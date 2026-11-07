// A small MCP server over standard input and output, for tests. It reports what its sandbox
// let it do in a tool's description, so a test can see that without calling a tool.
import { readFileSync, writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { connect, createServer } from 'node:net';

const mode = process.argv[2] ?? 'ok';
if (mode === 'exit') process.exit(3);

/** Says whether a file can be read, without ever returning its content. */
const canRead = path => { try { readFileSync(path); return 'read'; } catch { return 'denied'; } };
const outside = process.env.PROBE_OUTSIDE ? canRead(process.env.PROBE_OUTSIDE) : 'unset';
const inside = process.env.PROBE_INSIDE ? canRead(process.env.PROBE_INSIDE) : 'unset';
const secret = (process.env.API_KEY ?? '').length;

const tools = [
  { name: 'probe', description: `outside:${outside} inside:${inside} secret:${secret}`, inputSchema: { type: 'object', properties: {} } },
  { name: 'echo', title: 'Echo', description: 'Returns its text.', inputSchema: { type: 'object',
    properties: { text: { type: 'string' } }, required: ['text'] } },
  { name: 'write_note', description: 'Writes a file.', inputSchema: { type: 'object',
    properties: { path: { type: 'string' }, text: { type: 'string' } }, required: ['path', 'text'] } },
  { name: 'fail', description: 'Always fails.', inputSchema: { type: 'object', properties: {} } },
  { name: 'snapshot', description: 'Returns a picture, as a viewport screenshot would.', inputSchema: { type: 'object', properties: {} } },
  { name: 'dial', description: 'Connects to a port on this Mac.', inputSchema: { type: 'object', properties: { port: { type: 'number' } }, required: ['port'] } },
  { name: 'serve', description: 'Listens on a port on this Mac, as a server an app connects to would.', inputSchema: { type: 'object', properties: { port: { type: 'number' } }, required: ['port'] } },
];

// `many` stands for a server such as Roblox Studio's, whose list runs to tens of kilobytes: the
// tools above, one play-test tool, and 120 generated ones (the client keeps 128 per server) with long descriptions and schemas.
if (mode === 'many') {
  const filler = 'Does one small thing in the app, described at length so the whole list is far bigger than a model should be handed. '.repeat(3);
  tools.push({ name: 'play_test_start', description: 'Starts a play test of the open place. Stops with play_test_stop.',
    inputSchema: { type: 'object', properties: { mode: { type: 'string', enum: ['play', 'run'] } }, required: ['mode'] } });
  for (let n = 0; n < 120; n += 1) {
    tools.push({ name: `thing_${n}`, description: `Thing ${n}. ${filler}`, inputSchema: { type: 'object', properties: Object.fromEntries(
      ['a', 'b', 'c', 'd'].map(key => [key, { type: 'string', description: `The ${key} of thing ${n}, ${filler.slice(0, 60)}` }])) } });
  }
}

const send = message => process.stdout.write(`${JSON.stringify(message)}\n`);
process.stderr.write('fake-mcp: started\n');

createInterface({ input: process.stdin }).on('line', line => {
  const { id, method, params } = JSON.parse(line);
  if (mode === 'silent') return;
  if (id === undefined) return;
  if (method === 'initialize') {
    return send({ jsonrpc: '2.0', id, result: { protocolVersion: params.protocolVersion,
      capabilities: { tools: {} }, serverInfo: { name: 'fake-mcp', version: '1.0.0' } } });
  }
  // `notools` stands for a server whose tools arrive only once its app connects.
  if (method === 'tools/list' && mode === 'notools') return send({ jsonrpc: '2.0', id, result: { tools: [] } });
  if (method === 'tools/list') {
    // Two pages, so the client's paging is exercised.
    return send({ jsonrpc: '2.0', id, result: params?.cursor === 'page-2'
      ? { tools: tools.slice(2) } : { tools: tools.slice(0, 2), nextCursor: 'page-2' } });
  }
  if (method === 'tools/call') {
    const { name, arguments: input = {} } = params;
    if (name === 'echo') return send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: `echo: ${input.text}` }] } });
    if (name === 'fail') return send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: 'It broke.' }] } });
    if (name === 'write_note') {
      try { writeFileSync(input.path, input.text); return send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: 'written' }] } }); }
      catch (error) { return send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: error.code }] } }); }
    }
    if (name === 'snapshot') {
      // A one-pixel PNG beside a line of text, and a kind of content MiniClaw does not pass on.
      const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
      return send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: 'Viewport, 1x1.' }, { type: 'image', data: png, mimeType: 'image/png' },
        { type: 'audio', data: 'AAAA', mimeType: 'audio/wav' }] } });
    }
    if (name === 'dial') {
      // `hang` never answers, which is how a test sees the tool timeout.
      if (input.hang) return;
      // Says whether the sandbox let this server reach a port on this Mac, as an app's own add-on would need.
      const socket = connect({ host: '127.0.0.1', port: input.port });
      const answer = text => { socket.destroy(); send({ jsonrpc: '2.0', id, result: { isError: text !== 'connected', content: [{ type: 'text', text }] } }); };
      socket.once('connect', () => answer('connected'));
      socket.once('error', error => answer(error.code));
      return;
    }
    if (name === 'serve') {
      // Says whether the sandbox let this server accept connections on a port, as Roblox Studio's needs.
      // After binding it dials its own port, as Roblox's proxy does with its internal client.
      const server = createServer(socket => { socket.on('error', () => {}); socket.end('served\n'); });
      server.once('error', error => send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: error.code }] } }));
      server.listen(input.port, '127.0.0.1', () => {
        const self = connect({ host: '127.0.0.1', port: input.port });
        self.on('error', () => {});
        self.once('connect', () => { self.resume(); send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: 'listening, self-connected' }] } }); });
        self.once('error', error => send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: `listening, self-connect ${error.code}` }] } }));
      });
      return;
    }
    if (name === 'probe') return send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: tools[0].description }] } });
    return send({ jsonrpc: '2.0', id, error: { code: -32602, message: `Unknown tool: ${name}` } });
  }
  send({ jsonrpc: '2.0', id, error: { code: -32601, message: 'Method not found' } });
});
