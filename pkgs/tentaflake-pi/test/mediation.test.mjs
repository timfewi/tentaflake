import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync, chmodSync, symlinkSync, linkSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { modelBoundary, secret, worker } from '../mediation.mjs';

async function fixture(run) {
  const root = mkdtempSync(join(tmpdir(), 'pi-mediation-'));
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
test('credential reads reject broad permissions, symlinks and hard links', t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-secret-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const path = join(root, 'token'); writeFileSync(path, 'synthetic', { mode: 0o600 });
  assert.equal(secret(path), 'synthetic');
  chmodSync(path, 0o644); assert.throws(() => secret(path), /Unsafe/);
  chmodSync(path, 0o600);
  symlinkSync(path, join(root, 'symlink')); assert.throws(() => secret(join(root, 'symlink')));
  linkSync(path, join(root, 'hardlink')); assert.throws(() => secret(path), /Unsafe/);
});
test('offline worker refuses pre-cancelled, symlinked and duplicate publications', async t => {
  const root = mkdtempSync(join(tmpdir(), 'pi-worker-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const request = { workspace: root, id: `pi_${'a'.repeat(32)}`, argv: ['true'], timeout_seconds: 1 };
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
