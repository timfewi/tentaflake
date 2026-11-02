import { BACKGROUND_CONTEXT } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { openaiProvider } from '@earendil-works/pi-ai/providers/openai';
import { Harness, createRegistry, defineExtension, defineTool, hook, GenerationTask, CompactionTask, AssistantEntry } from '@earendil-works/pi-durable';
import { openNodeSqliteStorage } from '@earendil-works/pi-durable/storage/sqlite/node';
import { Type } from 'typebox';
import { join } from 'node:path';
import { Refusal, stateFile } from './ledger.mjs';
import { modelBoundary, Research, worker } from './mediation.mjs';

const context = BACKGROUND_CONTEXT;
const taskIdentity = value => {
  if (!Number.isSafeInteger(value) || value <= 0) throw new Refusal('Invalid SDK task correlation');
  return String(value);
};

// Public read-only Harness inspection never recovers or schedules. Compare both
// directions so restoring an older receipt database cannot hide newer SDK work.
async function auditCheckpoint(storage, viewer, ledger) {
  const live = await viewer.inspect(context);
  if (live.tasks.length || live.submissions.length) return false;
  for (const row of ledger.db.prepare('SELECT id,conversation_id FROM sessions').iterate()) {
    if (!await viewer.conversation(ledger.session(row.id), context)) return false;
  }
  let count = 0, cursor;
  do {
    const page = await storage.scanSubmissions({}, 128, cursor, context);
    for (const submission of page.items) {
      const row = ledger.db.prepare('SELECT * FROM runs WHERE submission_id=?').get(String(submission.id));
      if (!row || submission.type !== 'input' || submission.requestId !== row.request_id ||
          submission.conversationId !== ledger.session(row.session_id) ||
          (row.state === 'completed' ? submission.status !== 'done' :
            row.state === 'failed' ? submission.status !== 'unanswered' : true)) return false;
      count++;
    }
    cursor = page.next;
  } while (cursor);
  return count === ledger.db.prepare('SELECT count(*) AS count FROM runs').get().count;
}
export class Engine {
  constructor(config, ledger) { this.config = config; this.ledger = ledger; this.active = undefined; this.stopping = false; }
  async open() {
    if (this.ledger.quarantined) return; // MUST precede writable SDK open.
    const raw = createModels(); raw.setProvider(openaiProvider());
    const model = raw.getModel('openai', this.config.model);
    if (!model || model.api !== 'openai-responses') throw new Refusal('Configured model is not in the pinned supported catalog');
    this.boundary = modelBoundary({ ledger: this.ledger, models: raw, model,
      brokerBaseUrl: this.config.brokerBaseUrl, brokerTokenFile: this.config.brokerTokenFile,
      active: () => this.active });
    const registry = createRegistry();
    const tools = ['research_job','research_search','research_fetch','research_browser','research_read'].map(name => defineTool({
      name, description: `Call the isolated ${name} capability. Supply its JSON arguments as a string.`,
      parameters: Type.Object({ arguments: Type.String({ maxLength: 60000 }) }, { additionalProperties: false }),
      replay: 'unsafe',
      execute: async (args, api) => {
        const run = this.requireActive();
        const parameters = JSON.parse(args.arguments);
        if (!parameters || typeof parameters !== 'object' || Array.isArray(parameters)) throw new Refusal('Expected Research argument object');
        this.ledger.operation(run.id, taskIdentity(api.taskId), name);
        try {
          const response = await this.research.request('tools/call', { name, arguments: parameters }, run.controller.signal);
          if (response?.isError) throw new Refusal('Research outcome requires reconciliation');
          return { output: [{ type: 'text', text: JSON.stringify(response) }] };
        } catch (error) { this.ledger.uncertain(run.id); throw error; }
      },
    }));
    tools.push(defineTool({
      name: 'offline_worker', description: 'Submit one local-reversible command to the isolated offline worker. No host shell or network.',
      parameters: Type.Object({ argv: Type.Array(Type.String({ maxLength: 4096 }), { minItems: 1, maxItems: 128 }),
        timeout_seconds: Type.Integer({ minimum: 1, maximum: 3600 }) }, { additionalProperties: false }),
      replay: 'unsafe',
      execute: async (args, api) => {
        const run = this.requireActive();
        const operation = this.ledger.operation(run.id, taskIdentity(api.taskId), 'worker');
        const id = `pi_durable_${operation.replaceAll('-', '')}`;
        try { return { output: [{ type: 'text', text: JSON.stringify(await worker({ ...args, id, workspace: this.config.workspace, signal: run.controller.signal })) }] }; }
        catch (error) { this.ledger.uncertain(run.id); throw error; }
      },
    }));
    registry.install(defineExtension({ name: 'tentaflake-boundary', tools, hooks: [
      hook(GenerationTask, { beforeRequest: (_request, api) => {
        const run = this.requireActive();
        // A failed hook does not stop upstream; the Models facade independently
        // requires this one-use correlation before dispatch.
        run.taskId = taskIdentity(api.taskId);
      } }),
      hook(CompactionTask, { beforeCompact: () => ({ decline: true }) }),
    ] }));
    const options = {
      models: this.boundary.models, registry,
      settings: { retry: { enabled: false }, toolExecution: 'sequential', afterFailedRun: 'withdraw',
        stream: { deferred: false }, compaction: { enabled: false, backgroundTokens: 0 }, restartLimit: 1 },
      identity: { attribution: null },
    };
    const path = join(this.config.stateDir, 'durable.sqlite');
    for (const suffix of ['', '-journal', '-wal', '-shm']) stateFile(path + suffix);
    const storage = await openNodeSqliteStorage(path);
    let viewer;
    try {
      viewer = await Harness.open(storage, { ...options, readOnly: true }, context);
      if (!await auditCheckpoint(storage, viewer, this.ledger)) this.ledger.quarantine();
    } catch (error) { this.ledger.quarantine(); throw error; }
    finally { if (viewer) await viewer.close(context); else await storage.close(context); }
    if (this.ledger.quarantined) return;
    this.research = new Research(this.config.researchCommand);
    await this.research.initialize();
    this.harness = await Harness.open(await openNodeSqliteStorage(path), options, context);
    // No unconditional resume. A clean ledger permits only a new explicit
    // submission. All interrupted admissions were quarantined by Ledger.open.
  }
  requireActive() {
    if (!this.active || this.active.cancelled || this.stopping || this.ledger.quarantined)
      throw new Refusal('Instance is stopped or quarantined');
    return this.active;
  }
  async session(id) {
    if (!this.harness || this.stopping || this.ledger.quarantined) throw new Refusal('Instance unavailable');
    const existing = this.ledger.session(id);
    if (existing) {
      const conversation = await this.harness.conversation(existing, context);
      if (!conversation) throw new Refusal('SDK session state is missing');
      return conversation;
    }
    // UI sessions are independent host-owned conversations, not the SDK's
    // singleton root or children whose lifetime belongs to a generation task.
    const conversation = await this.harness.createConversation({ ownership: { kind: 'ownerless' },
      agent: { model: { provider: 'openai', modelId: this.config.model } } }, context);
    this.ledger.bindSession(id, conversation.id);
    return conversation;
  }
  async execute(row) {
    if (this.active || this.stopping || !this.harness) { this.ledger.uncertain(row.id); return; }
    const run = { id: row.id, controller: new AbortController(), cancelled: false };
    this.active = run;
    try {
      run.conversation = await this.session(row.session_id);
      const submission = await run.conversation.submit({ type: 'input', content: row.input, requestId: row.request_id }, context);
      this.ledger.bind(row.id, submission.id);
      const settled = await submission.wait(context);
      // This reads committed SDK state; it does not clear uncertainty on a
      // provider event, afterResponse callback, cancellation or transport EOF.
      await run.conversation.waitForIdle(context);
      if (run.cancelled || this.ledger.quarantined) { this.ledger.uncertain(row.id); return; }
      let output = '';
      if (settled.status === 'done' && settled.type === 'input') {
        const answer = await run.conversation.commit(tx => tx.entry(AssistantEntry, settled.answer), context);
        output = (answer?.model ?? []).flatMap(message => message.content ?? [])
          .filter(part => part.type === 'text').map(part => part.text).join('');
      }
      this.ledger.finish(row.id, settled.status === 'done' ? 'completed' : 'failed', { output, settlement: settled });
    } catch { this.ledger.uncertain(row.id); }
    finally { this.active = undefined; }
  }
  async cancel(id) {
    const row = this.ledger.get(id);
    if (!row) throw new Refusal('Unknown run', 404);
    if (!['running','stopping'].includes(row.state)) return;
    // Persist quarantine before signalling anything; a crash during abort can
    // never make this admission runnable again. Cancel does not reconcile work.
    this.ledger.stopping(id); this.ledger.uncertain(id);
    const run = this.active;
    if (run?.id === id) {
      run.cancelled = true; run.controller.abort();
      await run.conversation?.abort(context, { background: true });
    }
  }
  async close() {
    this.stopping = true; this.ledger.stopped = true;
    // A failed abort/SDK close must not leave the remaining capabilities live.
    // Keep every failure and the durable uncertainty written by cancel().
    const errors = [];
    try { if (this.active) await this.cancel(this.active.id); } catch (error) { errors.push(error); }
    try { this.research?.close(); } catch (error) { errors.push(error); }
    try { await this.harness?.close(context); } catch (error) { errors.push(error); }
    try { this.boundary?.close(); } catch (error) { errors.push(error); }
    if (errors.length === 1) throw errors[0];
    if (errors.length) throw new AggregateError(errors, 'Engine shutdown failed');
  }
}
