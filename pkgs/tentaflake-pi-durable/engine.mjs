import { BACKGROUND_CONTEXT } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { openaiProvider } from '@earendil-works/pi-ai/providers/openai';
import { opencodeGoProvider } from '@earendil-works/pi-ai/providers/opencode-go';
import { configuredGrants, isLegacySingleton } from './grants.mjs';
import { anthropicProvider } from '@earendil-works/pi-ai/providers/anthropic';
import { Harness, createRegistry, defineExtension, defineTool, hook, GenerationTask, CompactionTask, AssistantEntry } from '@earendil-works/pi-durable';
import { openNodeSqliteStorage } from '@earendil-works/pi-durable/storage/sqlite/node';
import { Type } from 'typebox';
import { join } from 'node:path';
import { Refusal, stateFile } from './ledger.mjs';
import { modelBoundary, Research, worker } from './mediation.mjs';

const context = BACKGROUND_CONTEXT;
export const supportedTools = Object.freeze([
  'research_job', 'research_search', 'research_fetch', 'research_browser', 'research_read', 'offline_worker',
]);
export function selectTools(tools = supportedTools) {
  if (!Array.isArray(tools) || tools.some(tool => !supportedTools.includes(tool)) || new Set(tools).size !== tools.length)
    throw new Refusal('Tools must be a unique subset of the reviewed stable tool IDs');
  return Object.freeze(supportedTools.filter(tool => tools.includes(tool)));
}
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
  constructor(config, ledger) {
    const tools = selectTools(config.tools);
    const grants = configuredGrants(config, tools);
    for (const grant of grants)
      if (grant.tools.some(tool => !tools.includes(tool))) throw new Refusal('Grant tools exceed immutable runner policy');
    this.config = Object.freeze({ ...config, grants, tools: selectTools([...new Set(grants.flatMap(grant => grant.tools))]) });
    this.opening = new Map();
    this.ledger = ledger; this.active = undefined; this.stopping = false;
  }
  async open() {
    if (this.ledger.quarantined) return; // MUST precede writable SDK open.
    const raw = createModels(); raw.setProvider(openaiProvider()); raw.setProvider(anthropicProvider()); raw.setProvider(opencodeGoProvider());
    const models = new Map();
    for (const grant of this.config.grants) {
      const model = raw.getModel(grant.provider, grant.model);
      const expected = grant.protocol === 'openai-responses' ? 'openai-responses' :
        grant.protocol === 'anthropic-messages' ? 'anthropic-messages' : 'openai-completions';
      if (!model || model.api !== expected) throw new Refusal('Granted model/protocol is not in the pinned supported catalog');
      const key = `${grant.provider}\0${grant.model}`;
      const selected = models.get(key) ?? { model, grant, revisions: new Map() };
      selected.revisions.set(grant.revision, grant);
      models.set(key, selected);
    }
    this.boundary = modelBoundary({ ledger: this.ledger, models: raw, grantedModels: models,
      brokerBaseUrl: this.config.brokerBaseUrl, brokerTokenFile: this.config.brokerTokenFile,
      brokerMaxRequestBytes: this.config.brokerMaxRequestBytes, active: () => this.active });
    const registry = createRegistry();
    const tools = this.config.tools.map(name => name === 'offline_worker' ? defineTool({
      name, description: 'Submit one local-reversible command to the isolated offline worker. No host shell or network.',
      parameters: Type.Object({ argv: Type.Array(Type.String({ maxLength: 4096 }), { minItems: 1, maxItems: 128 }),
        timeout_seconds: Type.Integer({ minimum: 1, maximum: 3600 }) }, { additionalProperties: false }),
      replay: 'unsafe',
      execute: (args, api) => this.invokeTool(name, args, api),
    }) : defineTool({
      name, description: `Call the isolated ${name} capability. Supply its JSON arguments as a string.`,
      parameters: Type.Object({ arguments: Type.String({ maxLength: 60000 }) }, { additionalProperties: false }),
      replay: 'unsafe',
      execute: (args, api) => this.invokeTool(name, args, api),
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
    this.registry = registry;
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
    this.research = new Research(this.config.researchCommand,
      this.config.tools.filter(name => name.startsWith('research_')));
    await this.research.initialize();
    this.harness = await Harness.open(await openNodeSqliteStorage(path), options, context);
    // No unconditional resume. A clean ledger permits only a new explicit
    // submission. All interrupted admissions were quarantined by Ledger.open.
  }
  registeredTools() {
    return this.harness ? this.registry.snapshot().tools().map(({ tool }) => tool.name) : [];
  }
  async invokeTool(name, args, api) {
    // Also enforce selection at dispatch: stale/forged calls never reach a
    // Research request, worker publication or durable operation admission.
    if (!this.config.tools.includes(name)) throw new Refusal('Tool is not selected by immutable runner policy');
    const run = this.requireActive();
    if (!run.grant?.tools.includes(name)) throw new Refusal('Tool is not selected by the immutable run grant');
    const call = `${taskIdentity(api.taskId)}:${api.callId}`;
    if (typeof api.callId !== 'string' || !api.callId || run.toolCalls.has(call))
      throw new Refusal('Tool execution has no unique invocation');
    run.toolCalls.add(call);
    const invocation = run.toolCalls.size;
    this.ledger.event(run.id, 'run.tool_started', { tool: name, invocation });
    if (name === 'offline_worker') {
      const operation = this.ledger.operation(run.id, taskIdentity(api.taskId), 'worker');
      const id = `pi_durable_${operation.replaceAll('-', '')}`;
      try {
        const result = await worker({ ...args, id, workspace: this.config.workspace, signal: run.controller.signal });
        run.toolResults.add(call);
        this.ledger.event(run.id, 'run.tool_completed', { tool: name, invocation });
        return { output: [{ type: 'text', text: JSON.stringify(result) }] };
      }
      catch (error) { this.ledger.uncertain(run.id); throw error; }
    }
    const parameters = JSON.parse(args.arguments);
    if (!parameters || typeof parameters !== 'object' || Array.isArray(parameters)) throw new Refusal('Expected Research argument object');
    this.ledger.operation(run.id, taskIdentity(api.taskId), name);
    try {
      const response = await this.research.request('tools/call', { name, arguments: parameters }, run.controller.signal);
      if (response?.isError) throw new Refusal('Research outcome requires reconciliation');
      run.toolResults.add(call);
      this.ledger.event(run.id, 'run.tool_completed', { tool: name, invocation });
      return { output: [{ type: 'text', text: JSON.stringify(response) }] };
    } catch (error) { this.ledger.uncertain(run.id); throw error; }
  }
  requireActive() {
    if (!this.active || this.active.cancelled || this.stopping || this.ledger.quarantined)
      throw new Refusal('Instance is stopped or quarantined');
    return this.active;
  }
  async session(id, grant) {
    while (this.opening.has(id)) await this.opening.get(id);
    const selected = grant ?? this.config.grants.find(item => item.revision === this.ledger.sessionGrant(id))
      ?? (this.config.grants.length === 1 ? this.config.grants[0] : null);
    if (!selected) {
      if (!this.harness || this.ledger.quarantined || this.stopping) throw new Refusal('Instance unavailable');
      this.ledger.reserveSession(id);
      return;
    }
    const promise = this.openSession(id, selected);
    this.opening.set(id,promise);
    try { return await promise; } finally { if (this.opening.get(id) === promise) this.opening.delete(id); }
  }
  async openSession(id, grant) {
    if (!this.harness || this.stopping || this.ledger.quarantined) throw new Refusal('Instance unavailable');
    const existing = this.ledger.session(id);
    if (existing) {
      this.ledger.checkSessionGrant(id, grant.revision, isLegacySingleton(this.config, grant));
      const conversation = await this.harness.conversation(existing, context);
      if (!conversation) throw new Refusal('SDK session state is missing');
      if (this.ledger.sessionGrant(id) === null) {
        const { model } = await conversation.agent(context);
        if (model?.provider !== grant.provider || model.modelId !== grant.model)
          throw new Refusal('Session provider binding conflict');
      }
      return conversation;
    }
    // UI sessions are independent host-owned conversations, not the SDK's
    // singleton root or children whose lifetime belongs to a generation task.
    const tools = this.registry.snapshot().tools().map(entry => entry.tool).filter(tool => grant.tools.includes(tool.name));
    const conversation = await this.harness.createConversation({ ownership: { kind: 'ownerless' },
      agent: { model: { provider: grant.provider, modelId: grant.model }, tools } }, context);
    this.ledger.bindSession(id, conversation.id, grant.revision);
    return conversation;
  }
  async execute(row) {
    if (this.active || this.stopping || !this.harness) { this.ledger.uncertain(row.id); return; }
    const run = { id: row.id, sessionId: row.session_id, controller: new AbortController(), cancelled: false,
      generations: [], toolCalls: new Set(), toolResults: new Set() };
    this.active = run;
    try {
      run.grant = JSON.parse(row.grant) ?? (this.config.grants.length === 1 ? this.config.grants[0] : null);
      if (!run.grant) throw new Refusal('Missing run grant');
      run.conversation = await this.session(row.session_id, run.grant);
      const submission = await run.conversation.submit({ type: 'input', content: row.input, requestId: row.request_id }, context);
      this.ledger.bind(row.id, submission.id);
      const settled = await submission.wait(context);
      // This reads committed SDK state; it does not clear uncertainty on a
      // provider event, afterResponse callback, cancellation or transport EOF.
      await run.conversation.waitForIdle(context);
      const measured = await Promise.all(run.generations);
      if (run.cancelled || this.ledger.quarantined) { this.ledger.uncertain(row.id); return; }
      let output = '';
      if (settled.status === 'done' && settled.type === 'input') {
        const answer = await run.conversation.commit(tx => tx.entry(AssistantEntry, settled.answer), context);
        output = (answer?.model ?? []).flatMap(message => message.content ?? [])
          .filter(part => part.type === 'text').map(part => part.text).join('');
      }
      const complete = measured.length > 0 && measured.every(Boolean);
      const sends = measured.filter(item => item?.sent === true);
      if (!complete && run.grant.revision !== 'legacy-single-model') {
        this.ledger.uncertain(row.id); return;
      }
      const measurement = complete ? {
        provider: run.grant.provider, protocol: run.grant.protocol, model: run.grant.model,
        grant_revision: run.grant.revision, provider_requests: sends.length,
        input_tokens: measured.reduce((n, m) => n + m.input, 0),
        output_tokens: measured.reduce((n, m) => n + m.output, 0),
        streamed: sends.length > 0 && sends.every(m => m.streamed),
        tool_calls: run.toolCalls.size, tool_results: run.toolResults.size,
      } : null;
      this.ledger.finish(row.id, settled.status === 'done' && !run.locallyRefused ? 'completed' : 'failed', {
        output, settlement: settled, usage: run.usage, ...(measurement ? { measurement } : {}),
      });
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
