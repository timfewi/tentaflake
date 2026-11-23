import { createHash } from 'node:crypto';
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

async function reserveRunBudget(operation, request, maxBytes) {
  const { run, grant } = operation;
  let size = 0;
  const chunks = [];
  if (!request.body) throw new Refusal('Missing provider request body');
  for await (const chunk of request.body) {
    size += chunk.byteLength;
    if (size > maxBytes) throw new Refusal('Provider request exceeds the broker body limit');
    chunks.push(chunk);
  }
  const encoded = Buffer.concat(chunks, size);
  const payload = JSON.parse(encoded);
  const completion = grant.protocol === 'openai-responses' ? payload.max_output_tokens :
    payload.max_completion_tokens ?? payload.max_tokens;
  if (!Number.isSafeInteger(completion) || completion < 1 || completion > grant.budget.max_output_tokens)
    throw new Refusal('Provider request has no approved completion limit');
  const tokens = BigInt(encoded.length) + BigInt(completion);
  const cost = (BigInt(encoded.length) * BigInt(grant.pricing.input_microusd_per_million) +
    BigInt(completion) * BigInt(grant.pricing.output_microusd_per_million) + 999999n) / 1000000n;
  const used = run.reserved ?? { tokens: 0n, cost: 0n };
  const limit = run.grant.run_budget;
  if (run.locallyRefused || used.tokens + tokens > BigInt(limit.max_tokens) || used.cost + cost > BigInt(limit.max_cost_microusd)) {
    throw new Refusal('Run budget cannot fund another provider request');
  }
  run.reserved = { tokens: used.tokens + tokens, cost: used.cost + cost };
  return encoded;
}

// All methods except fixed model lookup and one supported stream are absent as
// authority, rather than inherited from the provider registry. The SDK remains
// the only model loop. No CodingTools, execution environment or extension load.
export function modelBoundary({ ledger, models, model, grantedModels, brokerBaseUrl, brokerTokenFile, active, allowLoopback = false, brokerMaxRequestBytes = 1024 * 1024 }) {
  if (!Number.isSafeInteger(brokerMaxRequestBytes) || brokerMaxRequestBytes < 1)
    throw new Refusal('Invalid broker request body limit');
  const parsed = /^http:\/\/((?:[0-9]{1,3}\.){3}[0-9]{1,3})(?::([1-9][0-9]{0,4}))?\/v1\/?$/.exec(brokerBaseUrl);
  const octets = parsed?.[1].split('.').map(Number);
  if (!octets || octets.some((value, index) => value > 255 || String(value) !== parsed[1].split('.')[index]) ||
      (parsed[2] && Number(parsed[2]) > 65535) ||
      !(octets[0] === 10 || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
        (octets[0] === 192 && octets[1] === 168) || (allowLoopback && octets[0] === 127)))
    throw new Refusal('Expected fixed private IPv4 broker /v1 endpoint');
  const base = new URL(brokerBaseUrl);
  if (!grantedModels && model) grantedModels = new Map([[`${model.provider}\0${model.id}`, { model, grant: {
    revision: 'legacy-single-model', provider: model.provider, model: model.id, protocol: 'openai-responses',
    endpoint: `${base.origin}/v1/responses`, budget: { max_output_tokens: 4096 }, tools: [],
  } }]]);
  const grants = [...grantedModels.values()].flatMap(selected =>
    selected.revisions ? [...selected.revisions.values()] : [selected.grant]);
  const targets = new Set(grants.map(grant => grant.endpoint));
  const routeFor = protocol => protocol === 'openai-responses' ? '/v1/responses' :
    protocol === 'anthropic-messages' ? '/v1/messages' : '/v1/chat/completions';
  if (grants.some(grant => { try { const url = new URL(grant.endpoint);
    return url.href !== base.origin + routeFor(grant.protocol);
  } catch { return true; } }))
    throw new Refusal('Grant endpoint is outside the fixed broker');
  const context = new AsyncLocalStorage();
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, options = {}) => {
    const operation = context.getStore();
    const url = typeof input === 'string' || input instanceof URL ? String(input) : input.url;
    // The pinned Anthropic client uses its beta messages resource with one fixed
    // query flag. Only that SDK spelling maps to the canonical broker route.
    const endpoint = operation?.grant.endpoint;
    const beta = operation?.grant.protocol === 'anthropic-messages' && url === endpoint + '?beta=true';
    if (!operation || operation.attempted || (!beta && url !== endpoint) || !targets.has(endpoint) || (options.method ?? input.method ?? 'GET').toUpperCase() !== 'POST')
      throw new Refusal('Unmediated or repeated model transport denied');
    operation.attempted = true;
    if (operation.run.grant?.run_budget) {
      try {
        const request = new Request(input, options);
        const body = await reserveRunBudget(operation, request, brokerMaxRequestBytes);
        options = { ...options, method: request.method, signal: request.signal, headers: request.headers, body };
        input = url;
      } catch (error) {
        operation.locallyRefused = true;
        operation.run.locallyRefused = true;
        throw error;
      }
    }
    operation.sent = true;
    // SQLite FULL commit happens before the first byte may reach the broker.
    ledger.operation(operation.runId, operation.taskId, 'model');
    const headers = new Headers(input instanceof Request ? input.headers : undefined);
    new Headers(options.headers).forEach((value, name) => headers.set(name, value));
    // Provider SDKs express virtual credentials in provider-native headers.
    // Replace all of them with the one broker-scoped bearer credential; none
    // of the capsule's header choices can select an upstream credential.
    headers.delete('x-api-key');
    headers.set('authorization', `Bearer ${operation.brokerToken}`);
    for (const name of ['x-tentaflake-grant','x-tentaflake-operation','x-opencode-session']) headers.delete(name);
    if (operation.grant.brokerBinding === true) {
      headers.set('x-tentaflake-grant', operation.grant.revision);
      headers.set('x-tentaflake-operation', createHash('sha256').update(JSON.stringify([operation.runId,operation.taskId])).digest('hex'));
      if (operation.grant.protocol === 'opencode-go-chat-completions')
        headers.set('x-opencode-session', createHash('sha256').update(operation.sessionId).digest('hex'));
    }
    const target = beta && input instanceof Request ? new Request(endpoint,input) : beta ? endpoint : input;
    try {
      const response = await originalFetch(target, { ...options, headers, redirect: 'error' });
      operation.sse = response.ok && response.headers.get('content-type')?.split(';')[0].trim() === 'text/event-stream';
      return response;
    }
    catch (error) { ledger.uncertain(operation.runId); throw error; }
  };
  const lookup = (provider, id) => grantedModels.get(`${provider}\0${id}`)?.model;
  const facade = new Proxy({}, {
    get(_target, property) {
      if (property === 'getModel') return lookup;
      if (property === 'streamSimple') return (requested, messages, options) => {
        const run = active();
        const selected = grantedModels.get(`${requested.provider}\0${requested.id}`);
        const runGrant = run?.grant ?? selected?.grant;
        if (!run || !run.taskId || run.cancelled || ledger.quarantined || !selected ||
            requested.id !== runGrant?.model || requested.provider !== runGrant?.provider ||
            !(selected.revisions ? selected.revisions.has(runGrant.revision) : selected.grant.revision === runGrant.revision))
          throw new Refusal('Missing stable model operation correlation');
        const { model } = selected;
        const grant = selected.revisions?.get(runGrant.revision) ?? selected.grant;
        const brokerToken = secret(brokerTokenFile);
        const operation = { run, runId: run.id, taskId: run.taskId, sessionId: run.sessionId, sent: false, grant, brokerToken };
        run.taskId = undefined;
        // Anthropic appends /v1/messages itself. Disable its automatic provider
        // fallback and deferred placeholder tool to keep the declared grant exact.
        const brokerModel = { ...model, baseUrl: base.href.replace(/\/$/,'') };
        if (grant.protocol === 'anthropic-messages') {
          brokerModel.baseUrl = base.origin;
          brokerModel.compat = { ...model.compat, allowedFallbackModels: [], supportsMidConvoToolChanges: false };
        }
        const signal = run.controller?.signal && options?.signal
          ? AbortSignal.any([run.controller.signal, options.signal]) : run.controller?.signal ?? options?.signal;
        const stream = context.run(operation, () => models.streamSimple(
          brokerModel,
          { ...messages, ...(messages?.tools ? { tools: messages.tools.filter(tool => grant.tools.includes(tool.name)) } : {}) },
          { ...options, apiKey: brokerToken, deferred: false, maxRetries: 0,
            maxTokens: Math.min(options?.maxTokens ?? grant.budget.max_output_tokens, grant.budget.max_output_tokens),
            ...(grant.brokerBinding ? {cacheRetention:'none'} : {}),
            transport: 'sse', fetch: globalThis.fetch, signal },
        ));
        // A transport response is NOT a durable SDK checkpoint. Keep every
        // operation receipt until the submission + idle barrier is committed.
        const noSend = { input: 0, output: 0, streamed: false, sent: false };
        const generation = stream.result().then(message => {
          if (operation.locallyRefused) return noSend;
          run.usage = message.usage ?? null;
          if (['error', 'aborted'].includes(message.stopReason)) { ledger.uncertain(operation.runId); return null; }
          const usage = message.usage;
          if (!operation.sent || !usage) return null;
          const counts = [usage.input,usage.cacheRead ?? 0,usage.cacheWrite ?? 0,usage.output];
          if (counts.some(value => !Number.isSafeInteger(value) || value < 0)) return null;
          return { input: counts[0]+counts[1]+counts[2], output: usage.output, streamed: operation.sse === true, sent: true };
        }, () => {
          if (operation.locallyRefused) return noSend;
          ledger.uncertain(operation.runId);
          return null;
        });
        run.generations?.push(generation);
        return {
          result: () => stream.result(),
          async *[Symbol.asyncIterator]() {
            for await (const event of stream) {
              if (event.type === 'text_delta' && event.delta && !ledger.quarantined) {
                const characters = Array.from(event.delta);
                for (let offset = 0; offset < characters.length; offset += 512) {
                  ledger.event(run.id, 'run.text_delta', {
                    generation: Number(operation.taskId),
                    text: characters.slice(offset, offset + 512).join(''),
                  });
                }
              }
              yield event;
            }
          },
        };
      };
      return () => { throw new Refusal(`Model capability ${String(property)} is unavailable`); };
    },
  });
  return { models: facade, close() { globalThis.fetch = originalFetch; } };
}

const researchCapabilities = Object.freeze([
  'research_browser', 'research_fetch', 'research_job', 'research_read', 'research_search',
]);

export class Research {
  constructor(command, granted = researchCapabilities) {
    if (!/^\/nix\/store\/[^/]+\/bin\/research-client$/.test(command)) throw new Refusal('Research command is not a pinned closure');
    if (!Array.isArray(granted) || new Set(granted).size !== granted.length ||
        granted.some(name => !researchCapabilities.includes(name)))
      throw new Refusal('Invalid Research capability grant');
    this.granted = new Set(granted);
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
    if (method === 'tools/call' && (!params || typeof params !== 'object' ||
        typeof params.name !== 'string' || !this.granted.has(params.name)))
      throw new Refusal('Research capability is not granted');
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
    const result = await this.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'tentaflake-pi-durable', version: '1' } });
    if (result?.protocolVersion !== '2025-11-25') throw new Refusal('Research protocol mismatch');
    this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    const resultTools = await this.request('tools/list', {});
    if (!Array.isArray(resultTools?.tools) || resultTools.tools.length !== researchCapabilities.length ||
        !researchCapabilities.every(name => resultTools.tools.some(tool => tool?.name === name)))
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
  if (!/^pi_durable_[a-f0-9]{32}$/.test(id)) throw new Refusal('Invalid worker operation identity');
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
