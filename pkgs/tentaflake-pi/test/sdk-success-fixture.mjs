// Real Engine, provider parser, Harness scheduling and SQLite; only transport is
// synthetic. No socket, subprocess capability, provider or alternate model loop.
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';
import { BACKGROUND_CONTEXT as context } from '@earendil-works/chord/context';
import { AssistantEntry } from '@earendil-works/pi-durable';
import { createModels } from '@earendil-works/pi-ai/models';
import { openaiProvider } from '@earendil-works/pi-ai/providers/openai';
import { Engine } from '../engine.mjs';
import { Ledger } from '../ledger.mjs';

process.umask(0o077);
const [directory] = process.argv.slice(2);
const originalFetch = globalThis.fetch, originalSpawn = childProcess.spawn;
const researchCommand = '/nix/store/synthetic-pi-success/bin/research-client';
const children = [], sends = [];
const inputs = ['isolated-first-input', 'isolated-second-input', 'uncertain-third-input'];
const outputs = ['first committed answer', 'second committed answer', 'not a committed answer'];
const raw = createModels(); raw.setProvider(openaiProvider());
const model = raw.getModels('openai').find(value => value.api === 'openai-responses');
assert.ok(model, 'use the pinned real provider catalog');
const config = { stateDir: directory, model: model.id,
  brokerBaseUrl: 'http://10.88.0.1:8080/v1', brokerTokenFile: join(directory, 'virtual-key'),
  researchCommand, workspace: directory };
let ledger, engine, pending;

// Emulate just Research's bounded initialization protocol in memory. Any actual
// Research tool call or unexpected subprocess fails rather than reaching a host.
childProcess.spawn = (command, args, options) => {
  assert.equal(command, researchCommand);
  assert.deepEqual(args, ['--socket', '/run/tentaflake-research/socket']);
  assert.deepEqual(options.env, { PATH: '', LANG: 'C.UTF-8' });
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.methods = []; child.killed = false;
  child.stdin = new Writable({ write(bytes, _encoding, callback) {
    try {
      const request = JSON.parse(bytes.toString());
      child.methods.push(request.method);
      assert.ok(['initialize', 'notifications/initialized', 'tools/list'].includes(request.method));
      if (request.id !== undefined) {
        const result = request.method === 'initialize' ? { protocolVersion: '2025-11-25' }
          : { tools: ['research_browser', 'research_fetch', 'research_job', 'research_read', 'research_search'].map(name => ({ name })) };
        queueMicrotask(() => child.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n'));
      }
      callback();
    } catch (error) { callback(error); }
  } });
  child.kill = () => {
    assert.equal(child.killed, false);
    child.killed = true; child.stdout.end(); child.stdin.end();
    queueMicrotask(() => child.emit('exit', 0, null));
    return true;
  };
  children.push(child);
  return child;
};
syncBuiltinESMExports();

function responseFrames(index, terminal) {
  const text = outputs[index], id = `resp_synthetic_${index}`, itemId = `msg_synthetic_${index}`;
  const part = { type: 'output_text', text, annotations: [], logprobs: [] };
  const item = { id: itemId, type: 'message', role: 'assistant', status: 'completed', content: [part] };
  const response = { id, object: 'response', created_at: 1, status: 'completed', error: null,
    incomplete_details: null, model: model.id, output: [item], metadata: {},
    usage: { input_tokens: 5, output_tokens: 4, total_tokens: 9,
      input_tokens_details: { cached_tokens: 0 }, output_tokens_details: { reasoning_tokens: 0 } } };
  const events = [
    { type: 'response.created', response: { ...response, status: 'in_progress', output: [], usage: null } },
    { type: 'response.output_item.added', output_index: 0, item: { ...item, status: 'in_progress', content: [] } },
    { type: 'response.content_part.added', item_id: itemId, output_index: 0, content_index: 0, part: { ...part, text: '' } },
    { type: 'response.output_text.delta', item_id: itemId, output_index: 0, content_index: 0, delta: text, logprobs: [] },
    { type: 'response.output_text.done', item_id: itemId, output_index: 0, content_index: 0, text, logprobs: [] },
    { type: 'response.content_part.done', item_id: itemId, output_index: 0, content_index: 0, part },
    { type: 'response.output_item.done', output_index: 0, item },
    ...(terminal ? [{ type: 'response.completed', response }] : []),
  ];
  const frames = events.map((event, sequence_number) => `event: ${event.type}\ndata: ${JSON.stringify({ ...event, sequence_number })}\n\n`).join('');
  assert.ok(Buffer.byteLength(frames) < 16384);
  return new TextEncoder().encode(frames);
}

// Install before Engine.open(): the production boundary captures this as its
// only underlying fetch, still enforcing the exact private URL and durable send
// receipt. This function never delegates to the original network transport.
const syntheticFetch = async (input, options) => {
  const request = new Request(input, options);
  assert.equal(request.url, 'http://10.88.0.1:8080/v1/responses');
  assert.equal(request.method, 'POST');
  assert.equal(request.headers.get('authorization'), 'Bearer synthetic-virtual-key');
  assert.equal(options.redirect, 'error');
  assert.ok(pending, 'only an explicitly admitted input may dispatch');
  const expected = pending;
  assert.equal(expected.sent, false, 'exactly one send per input; SDK retries are disabled');
  expected.sent = true;
  const body = await request.json();
  assert.equal(body.model, model.id);
  assert.equal(body.stream, true);
  const users = body.input.filter(message => message.role === 'user');
  assert.deepEqual(users.map(message => message.content.map(part => part.text).join('')), [inputs[expected.index]],
    'independent sessions must not send another session\'s input');
  const encoded = JSON.stringify(body.input);
  for (let index = 0; index < expected.index; index++) {
    assert.equal(encoded.includes(inputs[index]), false);
    assert.equal(encoded.includes(outputs[index]), false);
  }
  const operations = ledger.db.prepare('SELECT * FROM operations WHERE run_id=?').all(expected.row.id);
  assert.equal(operations.length, 1, 'FULL ledger operation commit precedes transport');
  assert.equal(operations[0].kind, 'model');
  assert.match(operations[0].task_id, /^[1-9][0-9]*$/);
  assert.equal(ledger.get(expected.row.id).state, 'running');
  sends.push({ input: inputs[expected.index], task: operations[0].task_id });
  const response = new Response(new ReadableStream({ start(controller) {
    expected.release = () => {
      controller.enqueue(responseFrames(expected.index, expected.index !== 2));
      controller.close();
    };
  } }), { status: 200, headers: { 'content-type': 'text/event-stream', 'x-request-id': `synthetic-${expected.index}` } });
  expected.dispatched.resolve();
  return response;
};
globalThis.fetch = syntheticFetch;

async function open() {
  ledger = new Ledger(directory);
  engine = new Engine(config, ledger);
  await engine.open();
}
async function close() {
  try { await engine?.close(); }
  finally { ledger?.close(); }
  assert.equal(globalThis.fetch, syntheticFetch, 'close revokes the production boundary');
  assert.equal(children.every(child => child.killed), true, 'Research initialization transports close');
}
async function execute(index) {
  const body = { request_id: `request-${index}`, session_id: `session-${index}`,
    input: inputs[index], context_mode: 'persistent' };
  const admitted = ledger.admit(body);
  assert.equal(admitted.duplicate, false);
  pending = { index, row: admitted.row, sent: false, dispatched: Promise.withResolvers() };
  const execution = engine.execute(admitted.row);
  await Promise.race([pending.dispatched.promise, execution.then(() => {
    throw new Error(`Engine returned before synthetic dispatch: ${JSON.stringify(ledger.get(admitted.row.id))}`);
  })]);
  assert.equal(ledger.get(admitted.row.id).state, 'running');
  assert.equal(ledger.get(admitted.row.id).result, null, 'transport dispatch alone is not a result receipt');
  pending.release();
  await execution;
  pending = undefined;
  assert.equal(engine.active, undefined);
  assert.equal(sends.length, index + 1);
  return { row: ledger.get(admitted.row.id), body };
}
async function assertCompleted(row, index) {
  assert.equal(row.state, 'completed');
  assert.equal(ledger.quarantined, false);
  const receipt = JSON.parse(row.result);
  assert.equal(receipt.output, outputs[index]);
  assert.equal(receipt.settlement.status, 'done');
  assert.equal(receipt.settlement.type, 'input');
  assert.equal(String(receipt.settlement.id), row.submission_id);
  const submission = await engine.harness.submission(receipt.settlement.id, context);
  assert.ok(submission);
  assert.deepEqual(await submission.status(context), receipt.settlement, 'receipt matches committed SDK settlement');
  const conversation = await engine.session(row.session_id);
  assert.equal(receipt.settlement.conversationId, conversation.id);
  await conversation.waitForIdle(context);
  const answer = await conversation.commit(tx => tx.entry(AssistantEntry, receipt.settlement.answer), context);
  assert.equal(answer.model.flatMap(message => message.content).filter(part => part.type === 'text').map(part => part.text).join(''), outputs[index]);
  const live = await engine.harness.inspect(context);
  assert.equal(live.tasks.length, 0, 'idle barrier includes task completion, not just answer settlement');
  assert.equal(live.submissions.length, 0);
  return conversation.id;
}

try {
  await open();
  assert.ok(engine.harness, 'actual writable SDK opens');
  const first = await execute(0);
  const firstConversation = await assertCompleted(first.row, 0);
  const firstReceipt = first.row.result;
  await close();

  await open();
  assert.equal(ledger.quarantined, false, 'completed SDK checkpoint passes production read-only audit');
  assert.ok(engine.harness);
  assert.equal(ledger.get(first.row.id).result, firstReceipt, 'receipt survives clean reopen');
  assert.equal((await engine.session(first.body.session_id)).id, firstConversation);
  assert.equal(await assertCompleted(ledger.get(first.row.id), 0), firstConversation);
  const duplicate = ledger.admit(first.body);
  assert.equal(duplicate.duplicate, true);
  assert.equal(duplicate.row.id, first.row.id);
  assert.equal(duplicate.row.result, firstReceipt);
  // Like runner admission, a duplicate returns its existing receipt and is not
  // passed back to execute(). It must not schedule another model operation.
  assert.equal(sends.length, 1);
  assert.throws(() => ledger.admit({ ...first.body, input: 'conflicting identity' }), /identity conflict/);
  const second = await execute(1);
  const secondConversation = await assertCompleted(second.row, 1);
  assert.notEqual(secondConversation, firstConversation);
  assert.notEqual(second.row.id, first.row.id);
  assert.notEqual(second.row.submission_id, first.row.submission_id);
  assert.notEqual(sends[0].task, sends[1].task);
  assert.equal(ledger.get(first.row.id).result, firstReceipt);
  await close();

  await open();
  assert.ok(engine.harness, 'two completed independent sessions pass checkpoint audit');
  assert.equal(await assertCompleted(ledger.get(second.row.id), 1), secondConversation);
  const interrupted = await execute(2); // Full text, but deliberately no terminal response.completed.
  assert.equal(interrupted.row.state, 'uncertain', 'EOF is not a successful provider settlement');
  assert.equal(interrupted.row.result, null, 'partial output never becomes a completed receipt');
  assert.equal(ledger.quarantined, true);
  await close();

  const initialized = children.length;
  await open();
  assert.equal(ledger.quarantined, true);
  assert.equal(engine.harness, undefined, 'uncertainty refuses writable SDK open and recovery');
  assert.equal(children.length, initialized, 'quarantine precedes even Research initialization');
  const retry = ledger.admit(interrupted.body);
  assert.equal(retry.duplicate, true);
  assert.equal(retry.row.state, 'uncertain');
  await assert.rejects(engine.session('session-2'), /unavailable/);
  assert.throws(() => ledger.admit({ ...interrupted.body, request_id: 'new-after-uncertain' }), /quarantined/);
  assert.deepEqual(sends.map(send => send.input), inputs, 'one send each; clean reopen and uncertain retry never replay');
  assert.equal(ledger.db.prepare('SELECT count(*) AS count FROM operations').get().count, 3);
  for (const child of children)
    assert.deepEqual(child.methods, ['initialize', 'notifications/initialized', 'tools/list']);
  await close();
  process.send?.({ completed: 2, uncertain: 1, sends: sends.length, isolated: true });
} finally {
  try { if (engine && !ledger?.closed) await close(); }
  finally {
    for (const child of children) if (!child.killed) child.kill();
    globalThis.fetch = originalFetch;
    childProcess.spawn = originalSpawn; syncBuiltinESMExports();
    process.disconnect?.();
  }
}
