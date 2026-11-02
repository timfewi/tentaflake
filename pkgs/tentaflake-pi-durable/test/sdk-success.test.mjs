import test from 'node:test';
import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, chmodSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

// Kept separate from the registered recovery suite: invoke this file explicitly.
// The child isolates global fetch/builtin transport interception from other tests.
test('production Engine commits synthetic SDK success, reopens isolated sessions and never replays uncertain input',
  { timeout: 30000 }, async t => {
    const directory = mkdtempSync(join(tmpdir(), 'pi-durable-sdk-success-'));
    chmodSync(directory, 0o700);
    writeFileSync(join(directory, 'virtual-key'), 'synthetic-virtual-key', { mode: 0o600 });
    const child = fork(new URL('./sdk-success-fixture.mjs', import.meta.url), [directory],
      { stdio: ['ignore', 'ignore', 'pipe', 'ipc'], env: { PATH: '', LANG: 'C.UTF-8' } });
    let diagnostic = '', summary;
    child.stderr.on('data', bytes => {
      // Bound failures too; never inherit provider keys or print environment.
      diagnostic = (diagnostic + bytes).slice(-16384);
    });
    child.on('message', message => { summary = message; });
    const exited = once(child, 'exit');
    const abort = () => child.kill('SIGKILL');
    t.signal.addEventListener('abort', abort, { once: true });
    try {
      const [code, signal] = await exited;
      assert.equal(signal, null, diagnostic);
      assert.equal(code, 0, diagnostic);
      assert.deepEqual(summary, { completed: 2, uncertain: 1, sends: 3, isolated: true });
    } finally {
      t.signal.removeEventListener('abort', abort);
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      await exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });
