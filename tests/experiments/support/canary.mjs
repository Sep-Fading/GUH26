// Parses requests from the test runtime.
import { createInterface } from 'node:readline';
// Writes only fixed disposable canary files.
import { writeFileSync, appendFileSync } from 'node:fs';
// Resolves predetermined files beneath the fixture.
import { join } from 'node:path';

const [directory, access, advertised] = process.argv.slice(2);
if (!directory || !['write', 'read-only'].includes(access)
  || !['write', 'read-only'].includes(advertised)) {
  throw new Error('Expected an experiment directory and access mode');
}

// Returns a canary response; the model cannot choose paths or grant itself access.
function handle(request) {
  if (request.method === 'initialize') return {
    protocolVersion: request.params.protocolVersion,
    capabilities: { tools: {} }, serverInfo: { name: 'canary', version: '0.1.0' },
  };
  if (request.method === 'tools/list') return { tools: [{
    name: 'write_marker', description: 'Write one marker in the disposable test directory.',
    inputSchema: { type: 'object', properties: { nonce: { type: 'string' } },
      required: ['nonce'], additionalProperties: false },
    annotations: { readOnlyHint: advertised === 'read-only', destructiveHint: false },
  }] };
  if (request.method === 'ping') return {};
  if (request.method !== 'tools/call' || request.params?.name !== 'write_marker') {
    throw new Error('Unsupported fixture method');
  }
  const args = request.params.arguments;
  if (!args || Object.keys(args).length !== 1 || args.nonce !== 'canary-request') {
    throw new Error('Invalid canary arguments');
  }
  const denied = access !== 'write';
  if (!denied) writeFileSync(join(directory, 'canary.txt'), args.nonce, { flag: 'wx' });
  appendFileSync(join(directory, 'journal.jsonl'), `${JSON.stringify({
    nonce: args.nonce, outcome: denied ? 'denied' : 'written',
  })}\n`);
  return { isError: denied, content: [{ type: 'text',
    text: denied ? 'CANARY_DENIED' : 'CANARY_WRITTEN' }] };
}

createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  let response;
  try { response = { result: handle(request) }; }
  catch (error) { response = { error: { code: -32602, message: error.message } }; }
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: request.id, ...response })}\n`);
});
