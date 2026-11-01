import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, chmodSync, rmSync, symlinkSync, mkdirSync, linkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Ledger, privateDirectory } from '../ledger.mjs';

const input = { request_id: 'stable-request', session_id: 'session', input: 'hello', context_mode: 'persistent' };
function fixture(t, close = () => {}) {
  const root = mkdtempSync(join(tmpdir(), 'pi-ledger-'));
  chmodSync(root, 0o700);
  t.after(() => { try { close(); } finally { rmSync(root, { recursive: true, force: true }); } });
  return root;
}
test('exclusive writer, stable duplicates and conflicting intent', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  assert.throws(() => new Ledger(root), /Another runner/);
  const first = ledger.admit(input);
  assert.equal(ledger.admit(input).row.id, first.row.id);
  assert.equal(ledger.admit(input).duplicate, true);
  assert.throws(() => ledger.admit({ ...input, input: 'different' }), /conflict/);
  assert.throws(() => ledger.admit({ ...input, request_id: 'second' }), /busy/);
  assert.throws(() => ledger.operation(first.row.id, undefined, 'model'), /denied/);
});
test('crash window is quarantined even before SDK submission or first dispatch', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  const { row } = ledger.admit(input); ledger.close();
  ledger = new Ledger(root);
  assert.equal(ledger.quarantined, true);
  assert.equal(ledger.get(row.id).state, 'uncertain');
  assert.equal(ledger.admit(input).row.id, row.id);
  assert.throws(() => ledger.admit({ ...input, request_id: 'new' }), /quarantined/);
  assert.throws(() => ledger.operation(row.id, 'task', 'research_fetch'), /denied/);
  assert.throws(() => ledger.finish(row.id, 'completed', { output: 'invented' }), /uncertain/);
});
test('explicit stop retains worker and model uncertainty across reopen', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  const { row } = ledger.admit(input);
  const first = ledger.operation(row.id, 'task1', 'model');
  const second = ledger.operation(row.id, 'task2', 'worker');
  assert.notEqual(first, second);
  ledger.stopping(row.id); ledger.uncertain(row.id); ledger.close();
  ledger = new Ledger(root);
  assert.equal(ledger.db.prepare('SELECT count(*) AS count FROM operations').get().count, 2);
  assert.equal(ledger.quarantined, true);
  assert.throws(() => ledger.finish(row.id, 'completed', {}), /uncertain/);
});
test('only durable successful settlement permits new work after restart', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  const { row } = ledger.admit(input);
  ledger.operation(row.id, 'task', 'model'); ledger.finish(row.id, 'completed', { output: 'answer' }); ledger.close();
  ledger = new Ledger(root);
  assert.equal(ledger.quarantined, false);
  assert.equal(JSON.parse(ledger.get(row.id).result).output, 'answer');
  assert.equal(ledger.admit(input).duplicate, true);
  assert.equal(ledger.admit({ ...input, request_id: 'next' }).duplicate, false);
});
test('state permission and symlink boundaries fail closed', t => {
  const root = fixture(t); chmodSync(root, 0o755);
  assert.throws(() => new Ledger(root), /private/); chmodSync(root, 0o700);
  const target = join(root, 'real'); mkdirSync(target, { mode: 0o700 });
  const link = join(root, 'link'); symlinkSync(target, link);
  assert.throws(() => new Ledger(link), /Unsafe/);
  symlinkSync(join(root, 'foreign'), join(root, 'owner.sqlite'));
  assert.throws(() => new Ledger(root), /Unsafe/);
});
test('global restore quarantine persists without a matching run receipt', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  ledger.quarantine(); ledger.close(); ledger = new Ledger(root);
  assert.equal(ledger.quarantined, true);
  assert.throws(() => ledger.admit(input), /quarantined/);
});
test('numeric SDK session IDs round trip without changing their type', t => {
  let ledger; const root = fixture(t, () => ledger?.close()); ledger = new Ledger(root);
  ledger.bindSession('session', 123);
  assert.equal(ledger.session('session'), 123);
  assert.throws(() => ledger.bindSession('bad', '123'), /Invalid SDK session/);
});
test('group traversal applies only to the exact control directory, never state', t => {
  const root = fixture(t); chmodSync(root, 0o710);
  privateDirectory(root, { groupTraverse: true });
  assert.throws(() => new Ledger(root), /private/);
  chmodSync(root, 0o750);
  assert.throws(() => privateDirectory(root, { groupTraverse: true }), /private/);
});
test('hard-linked SQLite ownership files are refused', t => {
  const root = fixture(t); const ledger = new Ledger(root); ledger.close();
  linkSync(join(root, 'owner.sqlite'), join(root, 'foreign'));
  assert.throws(() => new Ledger(root), /Unsafe/);
});
