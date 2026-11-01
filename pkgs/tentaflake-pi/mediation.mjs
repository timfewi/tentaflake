import { AsyncLocalStorage } from 'node:async_hooks';
import { spawn } from 'node:child_process';
import { openSync, readFileSync, closeSync, writeSync, fsyncSync, linkSync, unlinkSync, constants, fstatSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { StringDecoder } from 'node:string_decoder';
import { Refusal } from './ledger.mjs';

export function secret(path, { sharedApi = false } = {}) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = fstatSync(fd);
    const permissions = sharedApi
      ? stat.uid === 0 && stat.gid === process.getgid() && (stat.mode & 0o777) === 0o440
      : stat.uid === process.getuid() && !(stat.mode & 0o077);
    if (!stat.isFile() || stat.nlink !== 1 || stat.size > 4096 || !permissions)
      throw new Refusal('Unsafe runtime token file');
    const value = readFileSync(fd, 'utf8').trim();
    if (!value || value.length > 4096 || /[\r\n\0]/.test(value)) throw new Refusal('Invalid runtime token');
    return value;
  } finally { closeSync(fd); }
}

// All methods except fixed model lookup and one supported stream are absent as
// authority, rather than inherited from the provider registry. The SDK remains
// the only model loop. No CodingTools, execution environment or extension load.
export function modelBoundary({ ledger, models, model, brokerBaseUrl, brokerTokenFile, active, allowLoopback = false }) {
  const parsed = /^http:\/\/((?:[0-9]{1,3}\.){3}[0-9]{1,3})(?::([1-9][0-9]{0,4}))?\/v1\/?$/.exec(brokerBaseUrl);
  const octets = parsed?.[1].split('.').map(Number);
  if (!octets || octets.some((value, index) => value > 255 || String(value) !== parsed[1].split('.')[index]) ||
      (parsed[2] && Number(parsed[2]) > 65535) ||
      !(octets[0] === 10 || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
        (octets[0] === 192 && octets[1] === 168) || (allowLoopback && octets[0] === 127)))
    throw new Refusal('Expected fixed private IPv4 broker /v1 endpoint');
  const base = new URL(brokerBaseUrl);
  const target = `${base.origin}/v1/responses`;
  const context = new AsyncLocalStorage();
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, options = {}) => {
    const operation = context.getStore();
    const url = typeof input === 'string' || input instanceof URL ? String(input) : input.url;
    if (!operation || operation.sent || url !== target || (options.method ?? input.method ?? 'GET').toUpperCase() !== 'POST')
      throw new Refusal('Unmediated or repeated model transport denied');
    operation.sent = true;
    // SQLite FULL commit happens before the first byte may reach the broker.
    ledger.operation(operation.runId, operation.taskId, 'model');
    try { return await originalFetch(input, { ...options, redirect: 'error' }); }
    catch (error) { ledger.uncertain(operation.runId); throw error; }
  };
  const lookup = (provider, id) => provider === model.provider && id === model.id ? model : undefined;
  const facade = new Proxy({}, {
    get(_target, property) {
      if (property === 'getModel') return lookup;
      if (property === 'streamSimple') return (requested, messages, options) => {
        const run = active();
        if (!run || !run.taskId || run.cancelled || ledger.quarantined || requested.id !== model.id || requested.provider !== model.provider)
          throw new Refusal('Missing stable model operation correlation');
        const operation = { runId: run.id, taskId: run.taskId, sent: false };
        run.taskId = undefined;
        const stream = context.run(operation, () => models.streamSimple(
          { ...model, baseUrl: base.href.replace(/\/$/, '') }, messages,
          { ...options, apiKey: secret(brokerTokenFile), deferred: false, maxRetries: 0,
            transport: 'sse', fetch: globalThis.fetch },
        ));
        // A transport response is NOT a durable SDK checkpoint. Keep every
        // operation receipt until the submission + idle barrier is committed.
        stream.result().then(message => {
          if (['error', 'aborted'].includes(message.stopReason)) ledger.uncertain(operation.runId);
        }, () => ledger.uncertain(operation.runId));
        return stream;
      };
      return () => { throw new Refusal(`Model capability ${String(property)} is unavailable`); };
    },
  });
  return { models: facade, close() { globalThis.fetch = originalFetch; } };
}

export class Research {
  constructor(command) {
    if (!/^\/nix\/store\/[^/]+\/bin\/research-client$/.test(command)) throw new Refusal('Research command is not a pinned closure');
    this.child = spawn(command, ['--socket', '/run/tentaflake-research/socket'], {
      stdio: ['pipe', 'pipe', 'ignore'], env: { PATH: '', LANG: 'C.UTF-8' },
    });
    this.pending = new Map(); this.sequence = 0; this.buffer = ''; this.closed = false;
    const decoder = new StringDecoder('utf8');
    const fail = () => {
      this.closed = true;
      for (const { reject } of this.pending.values()) reject(new Refusal('Research transport uncertain'));
      this.pending.clear();
    };
    this.child.on('error', fail); this.child.on('exit', fail);
    this.child.stdin.on('error', fail);
    this.child.stdout.on('data', chunk => {
      this.buffer += decoder.write(chunk);
      if (Buffer.byteLength(this.buffer) > 1024 * 1024) { fail(); this.child.kill(); return; }
      let end;
      while ((end = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
        try {
          const row = JSON.parse(line); const pending = this.pending.get(row.id);
          if (!pending) continue;
          this.pending.delete(row.id);
          if (row.error) pending.reject(new Refusal('Research request failed'));
          else pending.resolve(row.result);
        } catch { fail(); this.child.kill(); }
      }
    });
  }
  async request(method, params, signal) {
    if (this.closed || signal?.aborted || this.pending.size >= 4) throw new Refusal('Research unavailable');
    const id = ++this.sequence;
    let timer, abort;
    try {
      return await new Promise((resolve, reject) => {
        abort = () => { this.pending.delete(id); reject(new Refusal('Research operation uncertain')); this.close(); };
        timer = setTimeout(abort, 120000);
        signal?.addEventListener('abort', abort, { once: true });
        this.pending.set(id, { resolve, reject });
        const frame = JSON.stringify({ jsonrpc: '2.0', id, method, params });
        if (Buffer.byteLength(frame) > 65536) { abort(); return; }
        this.child.stdin.write(frame + '\n');
      });
    } finally { clearTimeout(timer); signal?.removeEventListener('abort', abort); this.pending.delete(id); }
  }
  async initialize() {
    const result = await this.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'tentaflake-pi', version: '1' } });
    if (result?.protocolVersion !== '2025-11-25') throw new Refusal('Research protocol mismatch');
    this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    const resultTools = await this.request('tools/list', {});
    const expected = ['research_browser','research_fetch','research_job','research_read','research_search'];
    if (JSON.stringify(resultTools.tools?.map(t => t.name).sort()) !== JSON.stringify(expected))
      throw new Refusal('Research capability mismatch');
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    for (const { reject } of this.pending.values()) reject(new Refusal('Research closed'));
    this.pending.clear(); this.child.kill();
  }
}

function openDirectory(path) {
  if (!path.startsWith('/') || path.split('/').some(part => part === '.' || part === '..'))
    throw new Refusal('Unsafe directory path');
  let fd = openSync('/', constants.O_RDONLY | constants.O_DIRECTORY);
  try {
    for (const part of path.split('/').filter(Boolean)) {
      const next = openSync(`/proc/self/fd/${fd}/${part}`, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
      closeSync(fd); fd = next;
    }
    return fd;
  } catch (error) { closeSync(fd); throw error; }
}

export async function worker({ workspace, id, argv, timeout_seconds, signal }) {
  signal?.throwIfAborted();
  if (!Array.isArray(argv) || argv.length < 1 || argv.length > 128 ||
      argv.some(a => typeof a !== 'string' || a.length > 4096 || a.includes('\0')) ||
      !Number.isInteger(timeout_seconds) || timeout_seconds < 1 || timeout_seconds > 3600)
    throw new Refusal('Invalid offline worker request');
  if (!/^pi_[a-f0-9]{32}$/.test(id)) throw new Refusal('Invalid worker operation identity');
  const directory = openDirectory(join(workspace, '.tentaflake-worker/inbox'));
  try {
    const inbox = `/proc/self/fd/${directory}`;
    const temporary = join(inbox, `.${id}.tmp`);
    const final = join(inbox, `${id}.json`);
    const fd = openSync(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    try {
      const bytes = Buffer.from(JSON.stringify({ version: 1, id, action_class: 'local-reversible', argv, timeout_seconds }));
      if (bytes.length > 65536) throw new Refusal('Worker request exceeds limit');
      let offset = 0;
      while (offset < bytes.length) offset += writeSync(fd, bytes, offset);
      fsyncSync(fd);
    } finally { closeSync(fd); }
    // Exclusive publication: no overwrite of a request with an existing ID.
    linkSync(temporary, final); unlinkSync(temporary); fsyncSync(directory);
  } finally { closeSync(directory); }
  const deadline = Date.now() + (timeout_seconds + 120) * 1000;
  while (Date.now() < deadline) {
    signal?.throwIfAborted();
    try {
      const results = openDirectory(`/run/tentaflake-worker/results/${id}`);
      let bytes;
      try {
        const file = openSync(`/proc/self/fd/${results}/result.json`, constants.O_RDONLY | constants.O_NOFOLLOW);
        try {
          const stat = fstatSync(file);
          if (!stat.isFile() || stat.size > 65536) throw new Refusal('Unsafe worker result');
          bytes = readFileSync(file, 'utf8');
        } finally { closeSync(file); }
      } finally { closeSync(results); }
      const result = JSON.parse(bytes);
      if (result.version !== 1 || result.id !== id || result.action_class !== 'local-reversible')
        throw new Refusal('Worker result identity mismatch');
      if (!['succeeded','failed','denied','rejected'].includes(result.status)) throw new Refusal('Worker execution uncertain');
      return result;
    } catch (e) { if (e.code !== 'ENOENT') throw e; }
    await delay(250, undefined, { signal });
  }
  throw new Refusal('Worker result uncertain');
}
