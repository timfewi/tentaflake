import http from 'node:http';
import { readFileSync, lstatSync, chmodSync } from 'node:fs';
import { randomUUID, createHash, timingSafeEqual } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Ledger, Refusal, identity, privateDirectory } from './ledger.mjs';
import { secret } from './mediation.mjs';
import { Engine } from './engine.mjs';

export function projection(row) {
  if (!row) throw new Refusal('Unknown run', 404);
  return { id: row.id, request_id: row.request_id, session_id: row.session_id,
    input: row.input, context_mode: 'persistent', state: row.state === 'uncertain' ? 'interrupted' : row.state,
    uncertain: row.state === 'uncertain', output: row.result ? JSON.parse(row.result).output : '',
    error: row.state === 'uncertain' ? 'External outcome uncertain; instance quarantined' : null };
}
async function body(request) {
  let size = 0; const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 70000) throw new Refusal('Request exceeds limit', 413);
    chunks.push(chunk);
  }
  try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { throw new Refusal('Invalid JSON', 400); }
}
export async function serve(config) {
  const token = Buffer.from(secret(config.apiTokenFile, { sharedApi: true }));
  privateDirectory(dirname(config.socketPath), { groupTraverse: true });
  try { lstatSync(config.socketPath); throw new Refusal('Runtime endpoint already exists'); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  const ledger = new Ledger(config.stateDir);
  const engine = new Engine(config, ledger);
  try { await engine.open(); }
  catch (error) { try { await engine.close(); } finally { ledger.close(); } throw error; }
  let closing = false;
  const server = http.createServer(async (request, response) => {
    const json = (status, value) => { response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); response.end(JSON.stringify(value)); };
    try {
      const supplied = Buffer.from((request.headers.authorization ?? '').replace(/^Bearer /, ''));
      if (!request.headers.authorization?.startsWith('Bearer ') || supplied.length !== token.length || !timingSafeEqual(supplied, token))
        throw new Refusal('Unauthorized', 401);
      if (closing) throw new Refusal('Runner is stopping', 503);
      const url = new URL(request.url, 'http://localhost');
      if (request.method === 'GET' && url.pathname === '/v1/capabilities') {
        json(200, { runtime: 'pi', version: 1, available: !ledger.quarantined,
          quarantined: ledger.quarantined, features: { runs: !ledger.quarantined, persistent_sessions: true,
            live_inputs: false, session_import: false, streaming: false, cancellation: true } }); return;
      }
      if (request.method === 'POST' && url.pathname === '/api/sessions') {
        const value = await body(request);
        if (!value || typeof value.title !== 'string' || value.title.length > 200 ||
            Object.keys(value).some(k => !['title','request_id'].includes(k)) ||
            (value.request_id !== undefined && !identity(value.request_id))) throw new Refusal('Invalid session request', 400);
        const id = value.request_id ? createHash('sha256').update(`session:${value.request_id}`).digest('hex') : randomUUID();
        await engine.session(id); json(200, { session: { id } }); return;
      }
      if (request.method === 'POST' && url.pathname === '/v1/runs') {
        const value = await body(request);
        if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Refusal('Invalid run request', 400);
        if (!ledger.session(value.session_id)) throw new Refusal('Unknown session', 404);
        const admitted = ledger.admit(value);
        if (!admitted.duplicate) void engine.execute(admitted.row);
        json(200, { run_id: admitted.row.id }); return;
      }
      if (request.method === 'GET' && url.pathname === '/v1/runs') {
        const id = url.searchParams.get('request_id');
        if (!identity(id) || [...url.searchParams.keys()].some(k => k !== 'request_id')) throw new Refusal('Invalid lookup', 400);
        json(200, projection(ledger.byRequest(id))); return;
      }
      const match = /^\/v1\/runs\/([A-Za-z0-9-]{1,128})(?:\/(events|stop|result))?$/.exec(url.pathname);
      if (!match) throw new Refusal('Capability unavailable', 404);
      const [, id, operation] = match;
      if (request.method === 'POST' && operation === 'stop') {
        const value = await body(request);
        if (!value || Object.keys(value).length) throw new Refusal('Stop takes no parameters', 400);
        await engine.cancel(id); json(200, projection(ledger.get(id))); return;
      }
      if (request.method !== 'GET') throw new Refusal('Method unavailable', 405);
      if (!operation || operation === 'result') { json(200, projection(ledger.get(id))); return; }
      if (operation !== 'events') throw new Refusal('Capability unavailable', 404);
      projection(ledger.get(id));
      response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
      let timer;
      const deadline = Date.now() + 3900000;
      const emit = () => {
        if (Date.now() >= deadline || response.writableLength > 65536) { response.destroy(); return; }
        const row = projection(ledger.get(id));
        if (['running','stopping'].includes(row.state)) { response.write(': waiting\n\n'); return; }
        clearInterval(timer);
        const event = row.state === 'completed' ? 'run.completed' : 'run.failed';
        response.end(`data: ${JSON.stringify({ event, run_id: id, output: row.output, state: row.state, uncertain: row.uncertain, error: row.error })}\n\n`);
      };
      timer = setInterval(emit, 1000);
      response.on('close', () => clearInterval(timer)); emit();
    } catch (error) {
      if (!response.headersSent) json(error instanceof Refusal ? error.status : 503, { error: error instanceof Refusal ? error.message : 'Runtime unavailable' });
      else response.destroy();
    }
  });
  server.maxConnections = 8; server.requestTimeout = 15000; server.headersTimeout = 5000;
  // Do not unlink an existing endpoint; systemd owns the private runtime dir.
  try {
    await new Promise((yes, no) => { server.once('error', no); server.listen(config.socketPath, yes); });
    chmodSync(config.socketPath, 0o620);
  } catch (error) {
    server.close();
    try { await engine.close(); } finally { ledger.close(); }
    throw error;
  }
  let closePromise;
  const close = () => {
    if (closePromise) return closePromise;
    closing = true;
    closePromise = (async () => {
      const errors = [];
      // Node owns the bound Unix socket and removes it on server.close().
      // A second unlink is both an ENOENT on normal stop and unsafe if another
      // endpoint has since occupied the path. Wait for HTTP teardown instead.
      try {
        await new Promise((yes, no) => {
          server.close(error => error && error.code !== 'ERR_SERVER_NOT_RUNNING' ? no(error) : yes());
          server.closeAllConnections();
        });
      } catch (error) { errors.push(error); }
      try { await engine.close(); } catch (error) { errors.push(error); }
      try { ledger.close(); } catch (error) { errors.push(error); }
      token.fill(0);
      if (errors.length === 1) throw errors[0];
      if (errors.length) throw new AggregateError(errors, 'Runner shutdown failed');
    })();
    return closePromise;
  };
  return { server, ledger, engine, close };
}

function configuration(path) {
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.uid !== 0 || (stat.mode & 0o022) || stat.size > 16384)
    throw new Refusal('Unsafe runtime configuration');
  const config = JSON.parse(readFileSync(path, 'utf8'));
  const fields = ['stateDir','socketPath','apiTokenFile','brokerBaseUrl','brokerTokenFile','model','researchCommand','workspace'];
  if (Object.keys(config).length !== fields.length || fields.some(k => typeof config[k] !== 'string'))
    throw new Refusal('Invalid runtime configuration');
  for (const key of ['stateDir','socketPath','apiTokenFile','brokerTokenFile','researchCommand','workspace'])
    if (resolve(config[key]) !== config[key]) throw new Refusal('Runtime paths must be canonical absolute paths');
  if (config.workspace === config.stateDir || config.workspace.startsWith(config.stateDir + '/') || config.stateDir.startsWith(config.workspace + '/'))
    throw new Refusal('State and workspace must be disjoint');
  return config;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.umask(0o077);
  if (process.argv.length !== 3) { console.error('Usage: tentaflake-pi CONFIG'); process.exitCode = 2; }
  else {
    try {
      const runtime = await serve(configuration(process.argv[2]));
      const stop = () => { runtime.close().then(() => { process.exitCode = 0; }, () => { process.exitCode = 1; }); };
      process.once('SIGTERM', stop); process.once('SIGINT', stop);
    } catch { console.error('Pi runtime refused startup'); process.exitCode = 1; }
  }
}
