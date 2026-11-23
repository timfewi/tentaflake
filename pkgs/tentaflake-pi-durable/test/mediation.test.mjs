import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync, chmodSync, symlinkSync, linkSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { modelBoundary, secret, worker, Research } from '../mediation.mjs';
import { startupStep } from '../ledger.mjs';

async function fixture(run) {
  const root = mkdtempSync(join(tmpdir(), 'pi-durable-mediation-'));
  const key = join(root, 'token'); writeFileSync(key, 'synthetic', { mode: 0o600 });
  const saved = globalThis.fetch;
  const events = []; let uncertain = false;
  globalThis.fetch = async () => { events.push('send'); return new Response('{}'); };
  const ledger = { get quarantined() { return uncertain; }, operation() { events.push('commit'); }, uncertain() { uncertain = true; } };
  const active = { id: 'run', taskId: 'task' };
  const model = { id: 'model', provider: 'openai' };
  let boundary;
  try {
    const create = models => (boundary = modelBoundary({ ledger, models, model,
      brokerBaseUrl: 'http://127.0.0.1:1234/v1', brokerTokenFile: key, active: () => active, allowLoopback: true }));
    await run({ ledger, active, model, events, create });
  } finally { boundary?.close(); globalThis.fetch = saved; rmSync(root, { recursive: true, force: true }); }
}
test('Research initialization requires the exact reviewed manifest for all tool selections', async () => {
  const expected = ['research_browser', 'research_fetch', 'research_job', 'research_read', 'research_search'];
  for (const [manifest, granted, accepted] of [
    [expected, expected, true],
    [[...expected, 'server_status'], expected, false],
    [expected, ['research_search'], true],
    [[...expected, 'server_status'], ['research_search'], false],
    [expected.slice(1), ['research_search'], false],
    [[...expected, 'research_search'], expected, false],
    [[...expected.map(name => ({ name })), { name: 3 }], expected, false],
  ]) {
    const methods = [];
    const research = Object.create(Research.prototype);
    research.granted = new Set(granted);
    research.child = { stdin: { write(line) {
      assert.equal(JSON.parse(line).method, 'notifications/initialized');
    } } };
    research.request = async method => {
      methods.push(method);
      return method === 'initialize' ? { protocolVersion: '2025-11-25' } :
        { tools: manifest.map(value => typeof value === 'string' ? { name: value } : value) };
    };
    if (accepted) await research.initialize();
    else await assert.rejects(startupStep('Research handshake', () => research.initialize()),
      { message: 'Research handshake: Research capability mismatch (expected exactly the five reviewed tools)' });
    assert.deepEqual(methods, ['initialize', 'tools/list']);
  }
});
test('Research dispatch exposes only the configured grant even when the server advertises more', async () => {
  const research = Object.create(Research.prototype);
  research.closed = false; research.pending = new Map(); research.sequence = 0;
  research.granted = new Set(['research_search']);
  research.child = { stdin: { write() {} } };
  await assert.rejects(research.request('tools/call', { name: 'research_fetch', arguments: {} }), /not granted/);
  assert.equal(research.sequence, 0);
});
test('production rejects public, loopback, malformed and credentialed broker endpoints', () => {
  for (const brokerBaseUrl of ['http://9.9.9.9/v1', 'http://127.0.0.1/v1', 'http://10.999.0.1/v1',
    'http://010.0.0.1/v1', 'http://user@10.0.0.1/v1', 'https://10.0.0.1/v1', 'http://10.0.0.1:65536/v1'])
    assert.throws(() => modelBoundary({ brokerBaseUrl }), /private IPv4/);
});
test('provider capabilities and absent task correlation are denied before dispatch', async () => {
  await fixture(async ({ create, active, model, events }) => {
    const boundary = create({ streamSimple() { throw new Error('must not invoke provider'); } });
    for (const method of ['completeSimple','complete','stream','fetchDeferred','streamDeferred','cancelDeferred','generateImages','classify','login','refresh','getProvider'])
      assert.throws(() => boundary.models[method](), /unavailable/);
    active.taskId = undefined;
    assert.throws(() => boundary.models.streamSimple(model, {}, {}), /correlation/);
    await assert.rejects(fetch('http://127.0.0.1:1234/v1/responses', { method: 'POST' }), /denied/);
    assert.deepEqual(events, []);
  });
});
test('durable barrier precedes bytes and the same SDK attempt cannot retry', async () => {
  await fixture(async ({ create, model, events }) => {
    let result;
    const boundary = create({ streamSimple() {
      result = (async () => {
        await fetch('http://127.0.0.1:1234/v1/responses', { method: 'POST' });
        await assert.rejects(fetch('http://127.0.0.1:1234/v1/responses', { method: 'POST' }), /repeated/);
        return { stopReason: 'stop' };
      })();
      return { result: () => result };
    } });
    await boundary.models.streamSimple(model, {}, {}).result();
    assert.deepEqual(events, ['commit', 'send']);
  });
});
test('failed durable barrier and wrong-origin traffic never send bytes', async () => {
  await fixture(async ({ create, model, ledger, events }) => {
    ledger.operation = () => { throw new Error('fsync failure'); };
    const boundary = create({ streamSimple() {
      const result = fetch('http://127.0.0.1:1234/v1/responses', { method: 'POST' }).then(() => ({ stopReason: 'stop' }));
      return { result: () => result };
    } });
    await assert.rejects(boundary.models.streamSimple(model, {}, {}).result(), /fsync/);
    assert.deepEqual(events, []);
    await assert.rejects(fetch('https://example.com/', { method: 'POST' }), /denied/);
  });
});
test('each supported provider protocol uses its exact one-shot broker route and scoped credential', async t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-durable-providers-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const key = join(root, 'token'); writeFileSync(key, 'broker-scope', { mode: 0o600 });
  const original = globalThis.fetch; const sends = []; const operations = [];
  globalThis.fetch = async (input, options) => {
    sends.push({ url: String(input), authorization: new Headers(options.headers).get('authorization'),
      apiKey: new Headers(options.headers).get('x-api-key'), aborted: options.signal?.aborted ?? false });
    return new Response('data: done\n\n', { headers: { 'content-type': 'text/event-stream' } });
  };
  let boundary;
  try {
    const definitions = [
      ['openai', 'ordinary-response', 'openai-responses', '/v1/responses'],
      ['anthropic', 'ordinary-message', 'anthropic-messages', '/v1/messages'],
      ['opencode', 'ordinary-chat', 'opencode-go-chat-completions', '/v1/chat/completions'],
    ];
    const grantedModels = new Map(definitions.map(([provider, id, protocol, route], index) =>
      [`${provider}\0${id}`, { model: { provider, id }, grant: { revision: `grant-${index}`, provider, model: id,
        protocol, endpoint: `http://127.0.0.1:4321${route}`, tools: [], budget: { max_output_tokens: 64 } } }]));
    const active = { id: 'run', taskId: '1', controller: new AbortController() };
    const models = { streamSimple(model, _messages, options) {
      const result = options.fetch(grantedModels.get(`${model.provider}\0${model.id}`).grant.endpoint,
        { method: 'POST', headers: { 'x-api-key': 'capsule-placeholder' }, signal: options.signal })
        .then(() => ({ stopReason: 'stop', usage: { input: 2, output: 3 } }));
      return { result: () => result };
    } };
    boundary = modelBoundary({ ledger: { quarantined: false, operation(run, task) { operations.push([run, task]); }, uncertain() {} },
      models, grantedModels, brokerBaseUrl: 'http://127.0.0.1:4321/v1', brokerTokenFile: key,
      active: () => active, allowLoopback: true });
    for (const [provider, id, _protocol, route] of definitions) {
      active.grant = grantedModels.get(`${provider}\0${id}`).grant; active.taskId = id;
      await boundary.models.streamSimple({ provider, id }, [], {}).result();
      assert.equal(sends.at(-1).url, `http://127.0.0.1:4321${route}`);
    }
    assert.deepEqual(sends.map(send => [send.authorization, send.apiKey, send.aborted]),
      definitions.map(() => ['Bearer broker-scope', null, false]));
    assert.equal(operations.length, 3);
    assert.deepEqual(active.usage, { input: 2, output: 3 });
  } finally { boundary?.close(); globalThis.fetch = original; }
});
test('provider transport cancellation is attributable and cannot be resent', async t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-durable-cancel-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const key = join(root, 'token'); writeFileSync(key, 'broker-scope', { mode: 0o600 });
  const original = globalThis.fetch; let sends = 0, uncertain = 0;
  globalThis.fetch = (_input, options) => {
    sends++;
    return new Promise((_resolve, reject) => options.signal.addEventListener('abort',
      () => reject(options.signal.reason ?? new Error('aborted')), { once: true }));
  };
  let boundary;
  try {
    const grant = { revision: 'cancel-grant', provider: 'anthropic', model: 'ordinary-message',
      protocol: 'anthropic-messages', endpoint: 'http://127.0.0.1:4321/v1/messages', tools: [], budget: { max_output_tokens: 64 } };
    const model = { provider: grant.provider, id: grant.model };
    const active = { id: 'cancel-run', taskId: 'cancel-task', grant, controller: new AbortController() };
    const models = { streamSimple(_model, _messages, options) {
      const result = options.fetch(grant.endpoint, { method: 'POST', signal: options.signal })
        .then(() => ({ stopReason: 'stop' }));
      return { result: () => result };
    } };
    boundary = modelBoundary({ ledger: { quarantined: false, operation() {}, uncertain() { uncertain++; } },
      models, grantedModels: new Map([[`${grant.provider}\0${grant.model}`, { model, grant }]]),
      brokerBaseUrl: 'http://127.0.0.1:4321/v1', brokerTokenFile: key, active: () => active, allowLoopback: true });
    const result = boundary.models.streamSimple(model, [], {}).result();
    active.controller.abort();
    await assert.rejects(result);
    assert.equal(sends, 1); assert.ok(uncertain >= 1); assert.equal(active.taskId, undefined);
  } finally { boundary?.close(); globalThis.fetch = original; }
});
test('credential reads reject broad permissions, symlinks and hard links', t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-durable-secret-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const path = join(root, 'token'); writeFileSync(path, 'synthetic', { mode: 0o600 });
  assert.equal(secret(path), 'synthetic');
  chmodSync(path, 0o644); assert.throws(() => secret(path), /Unsafe/);
  chmodSync(path, 0o600);
  symlinkSync(path, join(root, 'symlink')); assert.throws(() => secret(join(root, 'symlink')));
  linkSync(path, join(root, 'hardlink')); assert.throws(() => secret(path), /Unsafe/);
});
test('offline worker refuses pre-cancelled, symlinked and duplicate publications', async t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-durable-worker-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const request = { workspace: root, id: `pi_durable_${'a'.repeat(32)}`, argv: ['true'], timeout_seconds: 1 };
  await assert.rejects(worker({ ...request, signal: AbortSignal.abort() }));
  assert.equal(existsSync(join(root, '.tentaflake-worker')), false);
  mkdirSync(join(root, '.tentaflake-worker'));
  mkdirSync(join(root, 'foreign'));
  symlinkSync(join(root, 'foreign'), join(root, '.tentaflake-worker/inbox'));
  await assert.rejects(worker(request));
  rmSync(join(root, '.tentaflake-worker/inbox'));
  mkdirSync(join(root, '.tentaflake-worker/inbox'));
  const controller = new AbortController();
  const pending = worker({ ...request, signal: controller.signal });
  controller.abort(); await assert.rejects(pending);
  const path = join(root, `.tentaflake-worker/inbox/${request.id}.json`);
  const original = readFileSync(path, 'utf8');
  await assert.rejects(worker({ ...request, argv: ['changed'] }), /EEXIST/);
  assert.equal(readFileSync(path, 'utf8'), original);
  assert.equal(JSON.parse(original).action_class, 'local-reversible');
});

test('the aggregate run allowance is reserved before each paid provider call', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'pi-run-budget-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const key = join(directory, 'token');
  writeFileSync(key, 'synthetic', { mode: 0o600 });
  const original = globalThis.fetch;
  const completion = 8;
  for (const [provider, protocol, route, limitField] of [
    ['openai', 'openai-responses', '/v1/responses', 'max_output_tokens'],
    ['anthropic', 'anthropic-messages', '/v1/messages', 'max_tokens'],
    ['opencode-go', 'opencode-go-chat-completions', '/v1/chat/completions', 'max_completion_tokens'],
  ]) {
    const endpoint = 'http://127.0.0.1:4321' + route;
    const body = JSON.stringify({ model: 'synthetic-model', input: 'x'.repeat(65536), [limitField]: completion, stream: true });
    const reserve = Buffer.byteLength(body) + completion;
    for (const limited of ['max_tokens', 'max_cost_microusd']) {
      const grant = { version: 1, revision: 'budget-grant', provider, protocol, model: 'synthetic-model',
        endpoint, tools: [], budget: { max_output_tokens: completion },
        pricing: { input_microusd_per_million: 1000000, output_microusd_per_million: 1000000 },
        run_budget: { max_tokens: 2 * reserve, max_cost_microusd: 2 * reserve, [limited]: reserve },
      };
      const model = { id: grant.model, provider };
      const sends = [], operations = [];
      globalThis.fetch = async (url, options) => {
        sends.push([url, Buffer.from(options.body).toString()]);
        return new Response('data: done\n\n', { headers: { 'content-type': 'text/event-stream' } });
      };
      const active = { id: 'run', taskId: '1', grant, generations: [] };
      let providerBody = body;
      const models = { streamSimple(_model, _messages, options) {
        const done = options.fetch(new Request(endpoint, { method: 'POST', body: providerBody })).then(() => ({
          stopReason: 'stop', usage: { input: 3, output: 2 },
        }));
        return { result: () => done };
      } };
      const ledger = { quarantined: false, operation(...args) { operations.push(args); },
        uncertain() { assert.fail('a local budget denial must not become uncertain'); } };
      const resource = modelBoundary({ ledger, models, active: () => active, allowLoopback: true,
        brokerBaseUrl: 'http://127.0.0.1:4321/v1', brokerTokenFile: key,
        grantedModels: new Map([[`${provider}\0${grant.model}`, { model, grant }]]),
      });
      try {
        await resource.models.streamSimple(model, {}, {}).result();
        active.taskId = '2';
        await assert.rejects(resource.models.streamSimple(model, {}, {}).result(), /Run budget/);
        const outcomes = await Promise.all(active.generations);
        assert.deepEqual(sends, [[endpoint, body]]);
        assert.equal(operations.length, 1);
        assert.deepEqual(outcomes.map(({ sent }) => sent), [true, false]);
        assert.equal(active.locallyRefused, true);
        const fresh = { ...active, taskId: '3', generations: [], reserved: undefined, locallyRefused: false };
        Object.assign(active, fresh);
        providerBody = 'x'.repeat(1024 * 1024 + 1);
        await assert.rejects(resource.models.streamSimple(model, {}, {}).result(), /broker body limit/);
        assert.equal((await active.generations[0]).sent, false);
        assert.equal(sends.length, 1);
      } finally {
        resource.close();
        globalThis.fetch = original;
      }
    }
  }
});
