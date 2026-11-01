import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { fork } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, chmodSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Ledger } from '../ledger.mjs';
import { Engine } from '../engine.mjs';
import { DatabaseSync } from 'node:sqlite';
import { createModels } from '@earendil-works/pi-ai/models';
import { openaiProvider } from '@earendil-works/pi-ai/providers/openai';

for (const interruption of ['crash', 'close', 'restored-crash', 'restored-close']) {
  test(`real SDK ${interruption} after broker dispatch never reopens uncertain work`, { timeout: 20000 }, async () => {
    const directory = mkdtempSync(join(tmpdir(), 'pi-sdk-')); chmodSync(directory, 0o700);
    writeFileSync(join(directory, 'virtual-key'), 'synthetic-virtual-key', { mode: 0o600 });
    let sends = 0, observed;
    const dispatched = new Promise(resolve => { observed = resolve; });
    const sockets = new Set();
    // The server never completes its response. This is the uncertain send
    // window, not a successful provider/model acceptance test.
    const server = http.createServer((request, response) => {
      sends++;
      assert.equal(request.url, '/v1/responses');
      assert.equal(request.headers.authorization, 'Bearer synthetic-virtual-key');
      request.resume(); observed();
    });
    server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    const child = fork(new URL('./sdk-child.mjs', import.meta.url), [directory, `http://127.0.0.1:${server.address().port}/v1`],
      { stdio: ['ignore', 'ignore', 'pipe', 'ipc'] });
    let diagnostic = ''; child.stderr.on('data', bytes => { diagnostic += bytes; });
    let ledger, engine;
    try {
      await Promise.race([dispatched, once(child, 'exit').then(() => { throw new Error(`SDK exited before dispatch: ${diagnostic}`); })]);
      const exited = once(child, 'exit');
      if (interruption.endsWith('crash')) child.kill('SIGKILL'); else child.send('close');
      await exited;
      const restored = interruption.startsWith('restored-');
      if (restored) {
        // Simulate an older receipts snapshot beside newer interrupted SDK data.
        const old = new DatabaseSync(join(directory, 'receipts.sqlite'));
        old.exec('DELETE FROM operations; DELETE FROM runs;'); old.close();
      }
      ledger = new Ledger(directory);
      assert.equal(ledger.quarantined, !restored);
      assert.equal(ledger.db.prepare('SELECT count(*) AS count FROM operations').get().count, restored ? 0 : 1);
      const raw = createModels(); raw.setProvider(openaiProvider());
      engine = new Engine({ stateDir: directory, model: raw.getModels('openai').find(m => m.api === 'openai-responses').id,
        brokerBaseUrl: 'http://10.88.0.1:8080/v1', brokerTokenFile: join(directory, 'virtual-key') }, ledger);
      await engine.open();
      assert.equal(ledger.quarantined, true, 'read-only inspection must quarantine mismatched restored state');
      assert.equal(engine.harness, undefined, 'quarantine must precede writable SDK open');
      assert.throws(() => ledger.admit({ request_id: 'retry', session_id: 'session', input: 'synthetic', context_mode: 'persistent' }), /quarantined/);
      assert.equal(sends, 1);
    } finally {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      await engine?.close();
      ledger?.close(); for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

// Lifecycle regression: actual runner, HTTP Unix socket, SDK SQLite and Research
// child close; synthetic Research/abort transport only, with no provider dispatch.
for (const mode of ['idle', 'sessions', 'already-closed', 'sdk-error', 'storage-error', 'both-errors', 'cancel-error', 'cancel-sdk-error']) {
  test(`real runner shutdown: ${mode}`, { timeout: 20000 }, async t => {
    const directory = mkdtempSync(join(tmpdir(), 'pi-stop-')); chmodSync(directory, 0o700);
    mkdirSync(join(directory, 'runtime'), { mode: 0o710 });
    chmodSync(join(directory, 'runtime'), 0o710);
    mkdirSync(join(directory, 'workspace'), { mode: 0o700 });
    mkdirSync(join(directory, 'state'), { mode: 0o700 });
    writeFileSync(join(directory, 'api-token'), 'synthetic-api-token', { mode: 0o440 });
    chmodSync(join(directory, 'api-token'), 0o440);
    writeFileSync(join(directory, 'virtual-key'), 'synthetic-virtual-key', { mode: 0o600 });
    const child = fork(new URL('./sdk-child.mjs', import.meta.url), [directory, `runner-${mode}`],
      { stdio: ['ignore', 'ignore', 'pipe', 'ipc'] });
    let diagnostic = ''; child.stderr.on('data', bytes => { diagnostic += bytes; });
    const exited = once(child, 'exit');
    try {
      const [message] = await Promise.race([
        once(child, 'message', { signal: t.signal }),
        exited.then(() => { throw new Error(`Runner exited before readiness: ${diagnostic}`); }),
      ]);
      assert.equal(message, 'ready');
      child.send('close');
      const [code, signal] = await once(child, 'exit', { signal: t.signal });
      assert.equal(signal, null, diagnostic);
      assert.equal(code, 0, diagnostic);
    } finally {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      await exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
