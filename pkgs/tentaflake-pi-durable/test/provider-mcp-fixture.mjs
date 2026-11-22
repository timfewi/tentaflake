import assert from 'node:assert/strict';
import { createInterface } from 'node:readline';

const tools = [
  'research_browser',
  'research_fetch',
  'research_job',
  'research_read',
  'research_search',
  'research_leads',
  'research_enrich',
];
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.id === undefined) continue;
  let result;
  if (request.method === 'initialize') result = { protocolVersion: '2025-11-25' };
  else if (request.method === 'tools/list') result = { tools: tools.map((name) => ({ name })) };
  else {
    assert.equal(request.method, 'tools/call');
    assert.deepEqual(request.params, { name: 'research_search', arguments: { q: 'local' } });
    result = { isError: false, content: [{ type: 'text', text: 'fixed fixture observation' }] };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
}
