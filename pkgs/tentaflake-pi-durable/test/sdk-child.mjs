// Synthetic transport only; the real pinned SDK owns scheduling and SQLite.
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { openaiProvider } from '@earendil-works/pi-ai/providers/openai';
import { Harness, createRegistry, defineExtension, hook, GenerationTask } from '@earendil-works/pi-durable';
import { openNodeSqliteStorage } from '@earendil-works/pi-durable/storage/sqlite/node';
import { join } from 'node:path';
import { Ledger } from '../ledger.mjs';
import { modelBoundary } from '../mediation.mjs';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import childProcess from 'node:child_process';
import http from 'node:http';
import { syncBuiltinESMExports } from 'node:module';
import { createInterface } from 'node:readline';
import { serve } from '../runner.mjs';
const [directory, endpoint] = process.argv.slice(2);
process.umask(0o077);
if (endpoint === 'research-fixture') {
  // The real Research framing/child lifecycle, but no external capability.
  for await (const line of createInterface({ input: process.stdin })) {
    const request = JSON.parse(line);
    if (request.id === undefined) continue;
    assert.ok(['initialize', 'tools/list'].includes(request.method));
    const result = request.method === 'initialize' ? { protocolVersion: '2025-11-25' }
      : { tools: ['research_browser','research_fetch','research_job','research_read','research_search'].map(name => ({ name })) };
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
  }
} else if (endpoint?.startsWith('runner-')) {
  await runnerFixture(endpoint);
} else {
const ledger = new Ledger(directory);
const raw = createModels(); raw.setProvider(openaiProvider());
const model = raw.getModels('openai').find(m => m.api === 'openai-responses');
if (!model) throw new Error('Pinned provider catalog has no supported model');
const { row } = ledger.admit({ request_id: 'sdk-crash', session_id: 'session', input: 'synthetic', context_mode: 'persistent' });
const active = { id: row.id };
const boundary = modelBoundary({ ledger, models: raw, model, brokerBaseUrl: endpoint,
  brokerTokenFile: join(directory, 'virtual-key'), active: () => active, allowLoopback: true });
const registry = createRegistry();
registry.install(defineExtension({ name: 'barrier', hooks: [hook(GenerationTask, {
  beforeRequest: (_request, api) => {
    if (!Number.isSafeInteger(api.taskId) || api.taskId <= 0) throw new Error('Missing SDK task correlation');
    active.taskId = String(api.taskId);
  },
})] }));
const harness = await Harness.open(await openNodeSqliteStorage(join(directory, 'durable.sqlite')), {
  models: boundary.models, registry, settings: { retry: { enabled: false } },
}, context);
const root = await harness.root(context, { agent: { model: { provider: 'openai', modelId: model.id } } });
await root.submit({ type: 'input', content: row.input, requestId: row.request_id }, context);
process.on('message', async message => {
  if (message !== 'close') return;
  ledger.stopping(row.id); ledger.uncertain(row.id);
  await harness.close(context); boundary.close(); ledger.close(); process.exit(0);
});
}

async function runnerFixture(mode) {
  const tokenPath = join(directory, 'api-token');
  const tokenStat = fs.statSync(tokenPath);
  const originalStat = fs.fstatSync, originalSpawn = childProcess.spawn;
  // Only the synthetic API token's root uid is emulated for unprivileged package
  // checks. Keep all other file metadata checks and runtime admission unchanged.
  fs.fstatSync = (...args) => {
    const stat = originalStat(...args);
    return stat.dev === tokenStat.dev && stat.ino === tokenStat.ino
      ? Object.assign(Object.create(stat), { uid: 0 }) : stat;
  };
  const researchCommand = '/nix/store/synthetic-pi-durable-test/bin/research-client';
  childProcess.spawn = (command, args, options) => {
    assert.equal(command, researchCommand);
    assert.deepEqual(args, ['--socket', '/run/tentaflake-research/socket']);
    return originalSpawn(process.execPath, [new URL(import.meta.url).pathname, directory, 'research-fixture'], options);
  };
  syncBuiltinESMExports();
  // Even accidental provider use must fail locally, never send bytes.
  const forbiddenFetch = async () => { throw new Error('Runner attempted provider transport'); };
  globalThis.fetch = forbiddenFetch;
  const raw = createModels(); raw.setProvider(openaiProvider());
  const socketPath = join(directory, 'runtime', 'api.sock');
  const activeModel = raw.getModels('openai').find(model => model.api === 'openai-responses').id;
  const fixedTools = ['research_job', 'research_search', 'research_fetch', 'research_browser', 'research_read', 'offline_worker'];
  const selections = {
    'runner-tools-none': [],
    'runner-tools-search': ['research_search'],
    'runner-tools-all': [...fixedTools].reverse(),
  };
  const requestedTools = selections[mode];
  const expectedTools = fixedTools.filter(tool => requestedTools === undefined || requestedTools.includes(tool));
  const runtime = await serve({
    stateDir: join(directory, 'state'), socketPath, apiTokenFile: tokenPath,
    brokerBaseUrl: 'http://10.88.0.1:8080/v1', brokerTokenFile: join(directory, 'virtual-key'),
    model: activeModel,
    researchCommand, workspace: join(directory, 'workspace'),
    ...(requestedTools === undefined ? {} : { tools: requestedTools }),
  });
  assert.ok(runtime.engine.harness, 'exercise the actual writable SDK, not quarantine');
  if (mode === 'runner-capabilities' || mode.startsWith('runner-tools-')) {
    const capabilities = await new Promise((resolve, reject) => {
      const request = http.get({ socketPath, path: '/v1/capabilities',
        headers: { authorization: 'Bearer synthetic-api-token' } }, response => {
        let body = '';
        response.setEncoding('utf8');
        response.on('data', chunk => { body += chunk; });
        response.on('error', reject);
        response.on('end', () => {
          try {
            assert.equal(response.statusCode, 200);
            resolve(JSON.parse(body));
          } catch (error) { reject(error); }
        });
      });
      request.on('error', reject);
    });
    assert.equal(capabilities.runtime, 'pi-durable');
    assert.equal(capabilities.available, true);
    assert.equal(capabilities.model, activeModel);
    assert.equal(capabilities.model, runtime.engine.config.model);
    assert.equal(capabilities.features.tool_selection, true);
    const registered = runtime.engine.registry.snapshot().tools().map(({ tool }) => tool.name);
    assert.deepEqual(registered, expectedTools, 'the actual pinned SDK registry contains only the reviewed subset');
    assert.deepEqual(capabilities.tools, registered, 'capabilities report the actual registry, not desired source');
    assert.ok(Object.isFrozen(runtime.engine.config.tools));
    assert.throws(() => runtime.engine.config.tools.push('research_fetch'), TypeError);
    if (requestedTools) requestedTools.push('not-admitted');
    assert.deepEqual(runtime.engine.config.tools, expectedTools, 'caller mutation cannot expand accepted configuration');
    let calls = 0;
    const researchRequest = runtime.engine.research.request;
    runtime.engine.research.request = async () => { calls++; throw new Error('Unselected Research dispatch'); };
    try {
      for (const tool of [...fixedTools.filter(tool => !expectedTools.includes(tool)), 'not-admitted']) {
        await assert.rejects(runtime.engine.invokeTool(tool,
          tool === 'offline_worker' ? { argv: ['true'], timeout_seconds: 1 } : { arguments: '{}' },
          { taskId: 1 }), /not selected/);
      }
      assert.equal(calls, 0, 'unselected invokes never reach the admitted connection');
      assert.equal(runtime.ledger.db.prepare('SELECT count(*) AS count FROM operations').get().count, 0,
        'selection refusal precedes durable outbound admission and worker publication');
    } finally { runtime.engine.research.request = researchRequest; }
    assert.equal(runtime.ledger.db.prepare('SELECT count(*) AS count FROM runs').get().count, 0,
      'capabilities must not submit a model turn');
  }
  if (mode === 'runner-sessions') {
    const first = await runtime.engine.session('ui-session-one');
    const second = await runtime.engine.session('ui-session-two');
    assert.notEqual(first.id, second.id, 'distinct UI sessions must not share the SDK singleton root');
    assert.equal(runtime.ledger.session('ui-session-one'), first.id);
    assert.equal(runtime.ledger.session('ui-session-two'), second.id);
    assert.equal((await runtime.engine.session('ui-session-one')).id, first.id);
    assert.equal((await runtime.engine.session('ui-session-two')).id, second.id);
  }
  const sdkClose = runtime.engine.harness.close.bind(runtime.engine.harness);
  const ledgerClose = runtime.ledger.close.bind(runtime.ledger);
  const sdkError = new Error('synthetic SDK close failure');
  const storageError = new Error('synthetic ledger close failure');
  const cancelError = new Error('synthetic abort failure');
  const failSdk = ['runner-sdk-error', 'runner-both-errors', 'runner-cancel-sdk-error'].includes(mode);
  const failStorage = ['runner-storage-error', 'runner-both-errors'].includes(mode);
  const failCancel = ['runner-cancel-error', 'runner-cancel-sdk-error'].includes(mode);
  let sdkCloses = 0, ledgerCloses = 0, sdkClosed = false, ledgerClosed = false, interrupted;
  runtime.engine.harness.close = async (...args) => {
    sdkCloses++;
    if (failSdk) throw sdkError; // Fail BEFORE real cleanup, not after success.
    await sdkClose(...args); sdkClosed = true;
  };
  runtime.ledger.close = () => {
    ledgerCloses++;
    if (failStorage) throw storageError;
    ledgerClose(); ledgerClosed = true;
  };
  if (failCancel) {
    await runtime.engine.session('shutdown-session');
    interrupted = runtime.ledger.admit({ request_id: 'shutdown-cancel', session_id: 'shutdown-session',
      input: 'synthetic', context_mode: 'persistent' }).row;
    runtime.engine.active = { id: interrupted.id, controller: new AbortController(),
      conversation: { abort: async () => { throw cancelError; } } };
  }
  process.once('message', async message => {
    try {
      assert.equal(message, 'close');
      if (mode === 'runner-already-closed') {
        await new Promise((resolve, reject) => runtime.server.close(error => error ? reject(error) : resolve()));
        assert.equal(fs.existsSync(socketPath), false, 'Node already removed its Unix socket');
        fs.writeFileSync(socketPath, 'replacement endpoint must survive', { mode: 0o600 });
      }
      const closing = runtime.close();
      assert.equal(runtime.close(), closing, 'concurrent close shares completion and failure');
      const errors = [failCancel && cancelError, failSdk && sdkError, failStorage && storageError].filter(Boolean);
      const matches = error => errors.length === 1 ? error === errors[0]
        : error instanceof AggregateError && error.errors.length === errors.length
          && error.errors.every((cause, index) => cause === errors[index]);
      if (errors.length) {
        await assert.rejects(closing, matches);
        await assert.rejects(runtime.close(), matches);
      } else { await closing; await runtime.close(); }
      assert.equal(sdkCloses, 1); assert.equal(ledgerCloses, 1);
      assert.equal(runtime.engine.research.closed, true);
      assert.equal(globalThis.fetch, forbiddenFetch, 'model boundary revoked even when earlier cleanup fails');
      if (failCancel) {
        assert.equal(runtime.engine.active.cancelled, true);
        assert.equal(runtime.engine.active.controller.signal.aborted, true);
        const reopened = new Ledger(join(directory, 'state'));
        try {
          assert.equal(reopened.quarantined, true);
          assert.equal(reopened.get(interrupted.id).state, 'uncertain');
          assert.throws(() => reopened.admit({ request_id: 'retry', session_id: 'shutdown-session',
            input: 'synthetic', context_mode: 'persistent' }), /quarantined/);
        } finally { reopened.close(); }
      }
      assert.equal(runtime.server.listening, false);
      if (mode === 'runner-already-closed')
        assert.equal(fs.readFileSync(socketPath, 'utf8'), 'replacement endpoint must survive');
      else assert.equal(fs.existsSync(socketPath), false);
    } catch (error) {
      console.error(error); process.exitCode = 1;
    } finally {
      // Release deliberately failed resources only AFTER the assertions above.
      // This is fixture teardown, not evidence that runner cleanup succeeded.
      try {
        if (!sdkClosed) await sdkClose(context);
      } finally {
        try { if (!ledgerClosed) ledgerClose(); }
        finally { runtime.engine.research.close(); process.disconnect(); }
      }
    }
  });
  process.send('ready');
}
