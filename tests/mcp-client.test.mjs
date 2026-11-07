// Checks that MiniClaw can reach a saved MCP server and list its tools: a local command inside
// the sandbox, the same command unrestricted, and a remote server over Streamable HTTP.
import assert from 'node:assert/strict';
// Registers the connection tests.
import { test } from 'node:test';
// Serves a stand-in remote MCP server.
import { createServer } from 'node:http';
// Prepares a server folder outside MiniClaw's data.
import { copyFile, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
// Locates the temporary folder and the fake server.
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
import { placeMcpServer } from './support/mcp-fixture.mjs';
import { MCP_GATE_CLOSED, closedRefusal } from './support/sandbox.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const TOKEN = 'remote-token-91c';
// While Windows cannot enforce the sandbox, a confined local server is not started: its check
// answers 502 mcp_unreachable with GATE_CLOSED, while an unrestricted or remote server is unaffected.
// Only that answer selects the Windows expectations, so each test checks the real server once the gate opens.
// Tells whether an error body is the closed-gate refusal, and checks its code when it is.
const gateClosed = async (app, error) => {
  if (process.platform !== 'win32' || !error?.message?.startsWith(MCP_GATE_CLOSED)) return false;
  assert.deepEqual([error.code, error.message], ['mcp_unreachable', await closedRefusal(app, MCP_GATE_CLOSED)]);
  return true;
};

/** Starts a backend and a folder that holds a copy of the fake server and one readable file. */
async function fixture(t, env = {}) {
  const app = await launchApp({}, { env });
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-')));
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  await writeFile(join(folder, 'inside.txt'), 'inside');
  // A file in the user's home stands for everything a sandboxed server must not read.
  const outside = join(app.peer.dir, 'outside.txt');
  await writeFile(outside, 'outside');
  const draft = (extra = {}) => ({ name: 'notes', kind: 'stdio', command: mcp.command, args: mcp.args(),
    folder, env: { PROBE_OUTSIDE: outside, PROBE_INSIDE: join(folder, 'inside.txt'), API_KEY: 'twelve-chars' }, ...extra });
  return { app, folder, draft, mcp };
}

/** Saves a server and asks MiniClaw to connect to it and list its tools; a closed gate answers `{ gateClosed: true }`. */
async function check(app, draft, status = 200) {
  const { server } = await json(await post(app, '/api/mcp/servers', draft), 201);
  const response = await app.request(`/api/mcp/servers/${server.id}/check`, { method: 'POST', body: {}, signal: AbortSignal.timeout(20000) });
  if (status !== 502 && response.status === 502 && await gateClosed(app, (await response.clone().json()).error)) return { gateClosed: true };
  return json(response, status);
}

test('a local server runs inside the sandbox and lists every page of its tools', async t => {
  const { app, draft } = await fixture(t);
  const checked = await check(app, draft());
  if (checked.gateClosed) return;
  const { tools, server } = checked;
  assert.deepEqual(tools.map(tool => tool.name), ['probe', 'echo', 'write_note', 'fail', 'snapshot', 'dial', 'serve']);
  assert.equal(tools[0].description, 'outside:denied inside:read secret:12');
  assert.deepEqual([tools[1].title, tools[1].inputSchema.required], ['Echo', ['text']]);
  assert.deepEqual(server, { name: 'fake-mcp', version: '1.0.0' });
});

test('an unrestricted server is not confined, and says so', async t => {
  const { app, draft } = await fixture(t);
  const { tools } = await check(app, draft({ unrestricted: true }));
  assert.equal(tools[0].description, 'outside:read inside:read secret:12');
});

test('a server that exits or never answers is reported, not waited for', async t => {
  const { app, draft, mcp } = await fixture(t, { MINICLAW_TEST_MCP_START_MS: '1500' });
  const gone = await check(app, draft({ args: mcp.args('exit') }), 502);
  assert.equal(gone.error.code, 'mcp_unreachable');
  const silent = await check(app, draft({ name: 'silent', args: mcp.args('silent') }), 502);
  // With the Windows gate closed neither server is started, so both say why instead.
  if (await gateClosed(app, silent.error)) assert.equal(gone.error.message, silent.error.message);
  else assert.match(silent.error.message, /did not answer/i);
  assert.equal((await post(app, '/api/mcp/servers/019a0000-0000-7000-8000-000000000000/check')).status, 404);
});

test('a server that reports no tools yet is said to, not shown as an empty success', async t => {
  const { app, draft, mcp } = await fixture(t);
  const checked = await check(app, draft({ name: 'quiet', args: mcp.args('notools') }));
  if (checked.gateClosed) return;
  assert.deepEqual(checked.tools, []);
  assert.match(checked.note, /no tools yet/);
  assert.match(checked.note, /Roblox Studio/);
  assert.equal((await check(app, draft())).note, undefined, 'a server with tools carries no note');
});

test('a folder MiniClaw protects cannot be a server folder', async t => {
  const { app, draft } = await fixture(t);
  for (const folder of [app.peer.dir, app.peer.codexHome, 'relative', '/']) {
    assert.equal((await post(app, '/api/mcp/servers', draft({ folder }))).status, 400, folder);
  }
  for (const hosts of [['http://a.example'], ['a.example/path'], ['127.0.0.1'], ['localhost'], ['*.example.com'], Array(17).fill('a.example')]) {
    assert.equal((await post(app, '/api/mcp/servers', draft({ hosts }))).status, 400, JSON.stringify(hosts));
  }
  await json(await post(app, '/api/mcp/servers', draft({ hosts: ['api.example.com'] })), 201);
});

test('a remote server is called over Streamable HTTP with its token and session', async t => {
  const seen = [];
  const remote = createServer((request, response) => {
    let text = '';
    request.on('data', piece => { text += piece; });
    request.on('end', () => {
      const message = text ? JSON.parse(text) : {};
      seen.push({ method: request.method, rpc: message.method, headers: request.headers });
      if (request.headers.authorization !== `Bearer ${TOKEN}`) { response.writeHead(401).end(); return; }
      if (request.method === 'DELETE') { response.writeHead(204).end(); return; }
      if (message.method === 'initialize') {
        response.writeHead(200, { 'Content-Type': 'application/json', 'Mcp-Session-Id': 'session-7' });
        response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2025-06-18',
          capabilities: { tools: {} }, serverInfo: { name: 'remote-fake', version: '2.0.0' } } }));
      } else if (message.id === undefined) {
        response.writeHead(202).end();
      } else {
        // The answer arrives as an event stream, after a notification the client has to skip.
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.write(`event: message\ndata: ${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/progress', params: {} })}\n\n`);
        response.end(`event: message\nid: 1\ndata: ${JSON.stringify({ jsonrpc: '2.0', id: message.id,
          result: { tools: [{ name: 'lookup', description: 'Looks up.', inputSchema: { type: 'object' } }] } })}\n\n`);
      }
    });
  });
  await new Promise(resolve => remote.listen(0, '127.0.0.1', resolve));
  const app = await launchApp();
  t.after(async () => { await app.close(); remote.close(); });
  const url = `http://127.0.0.1:${remote.address().port}/mcp`;
  const { tools, server } = await check(app, { name: 'tracker', kind: 'http', url, token: TOKEN });
  assert.deepEqual([tools.map(tool => tool.name), server.name], [['lookup'], 'remote-fake']);
  const posts = seen.filter(request => request.method === 'POST');
  assert.deepEqual(posts.map(request => request.rpc), ['initialize', 'notifications/initialized', 'tools/list']);
  for (const request of posts) {
    assert.match(request.headers.accept, /application\/json/);
    assert.match(request.headers.accept, /text\/event-stream/);
  }
  assert.deepEqual(posts.slice(1).map(request => [request.headers['mcp-session-id'], request.headers['mcp-protocol-version']]),
    [['session-7', '2025-06-18'], ['session-7', '2025-06-18']]);
  // A wrong token is said plainly.
  const refused = await check(app, { name: 'locked', kind: 'http', url, token: 'wrong' }, 502);
  assert.match(refused.error.message, /token/i);
});
