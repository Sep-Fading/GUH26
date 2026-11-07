import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';

// This MCP fixture has no computer access; its marker proves execution occurred.
createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  let result;
  if (request.method === 'initialize') {
    result = { protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'fake-computer', version: '0.1.0' } };
  } else if (request.method === 'tools/list') {
    result = { tools: [{ name: 'get_app_state', description: 'Return a test marker.',
      inputSchema: { type: 'object', properties: { nonce: { type: 'string' } },
        required: ['nonce'], additionalProperties: false },
      annotations: { readOnlyHint: true } }] };
  } else if (request.method === 'tools/call') {
    const nonce = request.params.arguments.nonce;
    appendFileSync(process.argv[2], `${nonce}\n`);
    result = { content: [{ type: 'text', text: `FAKE_COMPUTER_EXECUTED:${nonce}` }] };
  } else if (request.method === 'ping') result = {};
  else {
    process.stdout.write(`${JSON.stringify({ id: request.id, jsonrpc: '2.0',
      error: { code: -32601, message: 'Unsupported fixture method' } })}\n`);
    return;
  }
  process.stdout.write(`${JSON.stringify({ id: request.id, jsonrpc: '2.0', result })}\n`);
});
