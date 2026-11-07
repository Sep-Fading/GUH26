// Checks the provider choice of an MCP server: who may use it, and the fields each provider's
// own configuration has, which MiniClaw honours itself because it is the MCP client.
import assert from 'node:assert/strict';
// Registers the provider tests.
import { test } from 'node:test';
// Serves a stand-in remote MCP server.
import { createServer } from 'node:http';
// Prepares a server folder outside MiniClaw's data.
import { copyFile, mkdtemp, readFile, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake providers.
import { launchApp, json, eventually } from './support/harness.mjs';
import { placeMcpServer } from './support/mcp-fixture.mjs';
import { MCP_GATE_CLOSED, assertGate, closedRefusal } from './support/sandbox.mjs';
// Names the fake Codex conversation and its turn.
import { THREAD, TURN, CLIENT } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const MAIN = '00000000-0000-0000-0000-000000000000';
const text = result => result.contentItems.map(item => item.text).join('\n');
const allow = async (app, server) => json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
/** Switches every allowed server on in one conversation: an allowance alone leaves a switch off (2026-09-22). */
const switchOn = async (app, thread = THREAD) => {
  for (const item of (await json(await app.request(`/api/capabilities?thread=${thread}`))).capabilities.filter(item => item.key.startsWith('mcp:') && item.allowed)) {
    await json(await post(app, `/api/threads/${thread}/capabilities`, { key: item.key, on: true }));
  }
};

/** A backend with both fake providers and a folder that holds the fake MCP server. */
async function fixture(t) {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-provider-')));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)),
    MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  const local = (name, extra = {}) => ({ name, kind: 'stdio', command: mcp.command, args: mcp.args(), folder, ...extra });
  return { app, folder, local };
}

/** Sends one gateway call as the fake Codex and returns what the model receives. */
async function codexCall(app, id, args, decide) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_mcp', threadId: THREAD, turnId: TURN, callId: id, arguments: args } });
  if (decide) {
    const waiting = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'approval');
    await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${waiting.scopedApproval.id}`, { action: decide }));
  }
  return (await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === id), `reply ${id}`, 15000))
    .responses.find(reply => reply.id === id).result;
}

// While Windows cannot enforce the sandbox, a confined local server is never started: its check
// answers 502 mcp_unreachable with GATE_CLOSED and a Codex call is refused in those words without a
// popup. Only those reports select the Windows expectations, so each test checks the real server
// once the gate opens.
// Tells whether the conversation's thread/start offered MiniClaw's sandboxed file and command tool,
// which the backend does exactly when its sandbox gate is open (no shell allowance is set here).
async function sandboxOffered(app) {
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start');
  assert.ok(start, 'a conversation was started before the gate is read');
  return (start.params.dynamicTools ?? []).some(tool => tool.name === 'miniclaw_scoped_native');
}
// Checks that a check answer is the closed-gate refusal.
async function assertRefusedCheck(app, response) {
  assert.equal(response.status, 502, 'a confined server is refused while the sandbox is unavailable');
  const { error } = await response.json();
  assert.deepEqual([error.code, error.message], ['mcp_unreachable', await closedRefusal(app, MCP_GATE_CLOSED)]);
}
// Tells whether the Codex conversation meets the closed gate (read from the backend's tool offer, without
// side effects) and then checks that `server` is refused and a call to it is refused without a popup.
async function gateClosed(app, server, tool, args = {}) {
  if (process.platform !== 'win32') return false;
  if (!await assertGate(app, !await sandboxOffered(app))) return false;
  await assertRefusedCheck(app, await app.request(`/api/mcp/servers/${server.id}/check`, { method: 'POST', body: {}, signal: AbortSignal.timeout(20000) }));
  const GATE_CLOSED = await closedRefusal(app, MCP_GATE_CLOSED);
  const refused = await codexCall(app, `gate-call-${server.name}`, { action: 'call_tool', server: server.name, tool, arguments: args });
  assert.deepEqual([refused.success, text(refused)], [false, `The server "${server.name}" could not be reached. ${GATE_CLOSED}`]);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'a server that cannot start is never asked about');
  return true;
}

test('a server is for both providers unless it says otherwise, and each provider sees only its own', async t => {
  const { app, folder, local } = await fixture(t);
  const saved = {};
  for (const [name, provider] of [['shared', undefined], ['codexonly', 'codex'], ['claudeonly', 'claude']]) {
    const { server } = await json(await post(app, '/api/mcp/servers', local(name, provider ? { provider } : {})), 201);
    assert.equal(server.provider, provider ?? 'both');
    await allow(app, server);
    saved[name] = server;
  }
  assert.equal((await post(app, '/api/mcp/servers', local('bad', { provider: 'gemini' }))).status, 400);

  // Codex: the shared and the Codex-only server, never Claude's.
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await switchOn(app);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const listed = JSON.parse(text(await codexCall(app, 'list-codex', { action: 'list_tools' })));
  assert.deepEqual(listed.servers.map(server => server.server), ['shared', 'codexonly']);
  const refused = await codexCall(app, 'call-wrong', { action: 'call_tool', server: 'claudeonly', tool: 'echo', arguments: { text: 'x' } });
  assert.equal(refused.success, false);
  assert.match(text(refused), /Claude conversations only/);
  const note = (await app.inspect()).requests.find(request => request.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(note, /MCP servers: shared, codexonly /);
  const capabilities = (await json(await app.request(`/api/capabilities?thread=${THREAD}`))).capabilities.filter(item => item.key.startsWith('mcp:'));
  assert.deepEqual(capabilities.map(item => [item.name, item.allowed]), [['shared', true], ['codexonly', true], ['claudeonly', false]]);

  // Claude: the shared and the Claude-only server.
  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  await switchOn(app, thread.id);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: `mcp ${JSON.stringify({ action: 'list_tools' })}`, clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  const seen = JSON.parse(done.messages.at(-1).text.replace(/^miniclaw_mcp error=false: /, ''));
  assert.deepEqual(seen.servers.map(server => server.server), ['shared', 'claudeonly']);
  const system = JSON.parse((await readFile(join(folder, 'calls.jsonl'), 'utf8')).trim().split('\n').at(-1)).args;
  assert.match(system[system.indexOf('--system-prompt') + 1], /MCP servers: shared, claudeonly /);
});

test('enabled and disabled tools, and the two timeouts, are honoured by MiniClaw itself', async t => {
  const { app, local } = await fixture(t);
  for (const bad of [{ startupSeconds: 0 }, { startupSeconds: 121 }, { toolSeconds: 0 }, { toolSeconds: 3601 }, { enabledTools: ['a b\n'] },
    { enabledTools: Array(129).fill('x') }, { disabledTools: [''] }]) {
    assert.equal((await post(app, '/api/mcp/servers', local('bad', bad))).status, 400, JSON.stringify(bad).slice(0, 60));
  }
  const { server } = await json(await post(app, '/api/mcp/servers', local('picky', { enabledTools: ['echo', 'fail', 'dial'], disabledTools: ['fail'],
    startupSeconds: 20, toolSeconds: 1 })), 201);
  assert.deepEqual([server.enabledTools, server.disabledTools, server.startupSeconds, server.toolSeconds], [['echo', 'fail', 'dial'], ['fail'], 20, 1]);
  await allow(app, server);
  // Check connection shows what the server has; the model is offered what is enabled and not disabled.
  const checkResponse = await app.request(`/api/mcp/servers/${server.id}/check`, { method: 'POST', body: {}, signal: AbortSignal.timeout(20000) });
  // With the Windows gate closed the server is never started: the check is refused and the conversation
  // lists it as unreachable. Only that answer selects this branch; an open gate takes the Mac path.
  if (process.platform === 'win32' && checkResponse.status === 502
    && (await checkResponse.clone().json()).error?.message?.startsWith(MCP_GATE_CLOSED)) {
    await assertRefusedCheck(app, checkResponse);
    const GATE_CLOSED = await closedRefusal(app, MCP_GATE_CLOSED);
    await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
    await switchOn(app);
    await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
    const unreachable = JSON.parse(text(await codexCall(app, 'list-1', { action: 'list_tools' })));
    assert.deepEqual(unreachable.servers, [{ server: 'picky', unreachable: GATE_CLOSED }]);
    return;
  }
  const checked = await json(checkResponse);
  assert.deepEqual(checked.tools.map(tool => [tool.name, tool.offered]), [['probe', false], ['echo', true], ['write_note', false], ['fail', false], ['snapshot', false], ['dial', true], ['serve', false]]);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await switchOn(app);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const listed = JSON.parse(text(await codexCall(app, 'list-1', { action: 'list_tools' })));
  assert.deepEqual(listed.servers[0].tools.map(tool => tool.name), ['echo', 'dial']);
  const hidden = await codexCall(app, 'call-hidden', { action: 'call_tool', server: 'picky', tool: 'fail', arguments: {} });
  assert.match(text(hidden), /no tool named/i);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'a tool that is not offered is never asked about');
  // A call that outlasts the tool timeout is ended and said in words. Port 9 on this Mac accepts nothing and
  // the sandbox refuses it at once, so the slow call is a connection to a port nothing answers on.
  const slow = await codexCall(app, 'call-slow', { action: 'call_tool', server: 'picky', tool: 'dial', arguments: { port: 1, hang: true } }, 'approve_once');
  assert.equal(slow.success, false);
  assert.match(text(slow), /did not answer in time/i);
});

test('draft tools follow the provider and Claude uses them on its first prompt', async t => {
  const { app, local } = await fixture(t);
  const { server } = await json(await post(app, '/api/mcp/servers', local('claudeonly', { provider: 'claude' })), 201);
  await allow(app, server);
  const key = `mcp:${server.id}`;
  for (const claude of [false, true]) {
    const { capabilities } = await json(await app.request(`/api/capabilities?claude=${claude}`));
    assert.equal(capabilities.find(item => item.key === key).allowed, claude);
  }
  const { capabilities } = await json(await app.request('/api/capabilities?vm=true'));
  assert.equal(capabilities.find(item => item.key === 'web_search').allowed, false);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { [key]: true } }), 403);
  const model = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model,
    capabilities: { [key]: true, web_search: false, questions: false } }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'mcp {"action":"list_tools"}',
    clientMessageId: crypto.randomUUID(), model }), 202);
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  const seen = JSON.parse(done.messages.at(-1).text.replace(/^miniclaw_mcp error=false: /, ''));
  assert.deepEqual(seen.servers.map(server => server.server), ['claudeonly']);
});

test('a remote server takes extra headers, whose values are write-only like a token', async t => {
  const seen = [];
  const remote = createServer((request, response) => {
    let body = '';
    request.on('data', piece => { body += piece; });
    request.on('end', () => {
      const message = body ? JSON.parse(body) : {};
      seen.push(request.headers);
      if (message.id === undefined) { response.writeHead(202).end(); return; }
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: message.method === 'initialize'
        ? { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'remote', version: '1' } } : { tools: [] } }));
    });
  });
  await new Promise(resolve => remote.listen(0, '127.0.0.1', resolve));
  const { app } = await fixture(t);
  t.after(() => remote.close());
  const url = `http://127.0.0.1:${remote.address().port}/mcp`;
  const SECRET = 'header-secret-551';
  for (const headers of [{ 'Bad Name': 'x' }, { Host: 'x' }, { 'Content-Length': '1' }, { 'Mcp-Session-Id': 'x' }, { 'X-Key': 'a\nb' }]) {
    assert.equal((await post(app, '/api/mcp/servers', { name: 'bad', kind: 'http', url, headers })).status, 400, JSON.stringify(headers));
  }
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'keyed', kind: 'http', url, provider: 'claude', headers: { 'X-Api-Key': SECRET, 'X-Team': 'blue' } }), 201);
  assert.deepEqual(server.headers, ['X-Api-Key', 'X-Team']);
  assert.equal(JSON.stringify([server, (await json(await app.request('/api/mcp/servers'))).servers]).includes(SECRET), false);
  await json(await app.request(`/api/mcp/servers/${server.id}/check`, { method: 'POST', body: {} }));
  assert.deepEqual([seen.at(-1)['x-api-key'], seen.at(-1)['x-team']], [SECRET, 'blue']);
  // Kept by name, removed by omission, as environment values are.
  const edited = await json(await post(app, `/api/mcp/servers/${server.id}`, { name: 'keyed', kind: 'http', url, provider: 'claude', revision: 1, headers: { 'X-Api-Key': null } }));
  assert.deepEqual(edited.server.headers, ['X-Api-Key']);
  assert.equal((await post(app, '/api/mcp/servers', { name: 'local', kind: 'stdio', command: process.execPath, headers: { 'X-Key': 'v' } })).status, 400);
});

test('a picture a tool returns reaches both providers as a picture', async t => {
  const { app, local } = await fixture(t);
  const { server } = await json(await post(app, '/api/mcp/servers', local('camera')), 201);
  await allow(app, server);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await switchOn(app);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  if (await gateClosed(app, server, 'snapshot')) return;
  const result = await codexCall(app, 'shot-1', { action: 'call_tool', server: 'camera', tool: 'snapshot', arguments: {} }, 'approve_once');
  assert.equal(result.success, true);
  assert.deepEqual(result.contentItems.map(item => item.type), ['inputText', 'inputImage']);
  assert.match(result.contentItems[0].text, /Viewport, 1x1\.[\s\S]*audio content is not passed on/);
  assert.match(result.contentItems[1].imageUrl, /^data:image\/png;base64,iVBOR/);
  // Requirement change 2026-09-22: MiniClaw keeps the picture with the conversation, Activity links it,
  // and the model is told its id so it can hand it to another agent. The entry holds a link, never the bytes.
  const entry = (await json(await app.request(`/api/threads/${THREAD}`))).activities.find(item => item.id === 'shot-1');
  assert.match(entry.output, /1 image/);
  assert.equal(JSON.stringify(entry).includes('iVBOR'), false);
  assert.equal(entry.pictures.length, 1);
  const [kept] = entry.pictures;
  assert.equal(kept.name, 'camera-snapshot-1.png');
  assert.equal(kept.url, `/api/threads/${THREAD}/attachments/${kept.id}`);
  assert.match(result.contentItems[0].text, new RegExp(`attachment ${kept.id}`));
  const served = await app.request(kept.url);
  assert.equal(served.headers.get('content-type'), 'image/png');
  assert.equal(Buffer.from(await served.arrayBuffer()).subarray(1, 4).toString(), 'PNG');
  // A tool's picture belongs to no message of the user's, so it is not shown as an upload and cannot be discarded.
  const again = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(JSON.stringify(again.messages).includes(kept.id), false);
  assert.equal((await app.request(kept.url, { method: 'DELETE', body: {} })).status, 409);

  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  await switchOn(app, thread.id);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: `mcp ${JSON.stringify({ action: 'call_tool', server: 'camera', tool: 'snapshot', arguments: {} })}`,
    clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const done = await eventually(async () => {
    const value = await json(await app.request(`/api/threads/${thread.id}`));
    if (value.scopedApproval) await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: 'approve_once' });
    return value;
  }, value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  assert.match(done.messages.at(-1).text, /\[image image\/png \d+\]/);
  const claudeEntry = done.activities.find(item => item.kind === 'mcp');
  assert.equal(claudeEntry.pictures.length, 1);
  assert.equal((await app.request(claudeEntry.pictures[0].url)).headers.get('content-type'), 'image/png');
});

test('saved activation defaults expose MCP tools on the first turn for each provider', async t => {
  const { app, local } = await fixture(t);
  const servers = {};
  for (const [name, provider] of [['blender', 'both'], ['claudeonly', 'claude']]) {
    const { server } = await json(await post(app, '/api/mcp/servers', local(name, { provider })), 201);
    servers[name] = server;
    await allow(app, server);
    await json(await post(app, `/api/agents/${MAIN}/capability-defaults`, { key: `mcp:${server.id}`, on: true }));
  }
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'List tools', clientMessageId: CLIENT }), 202);
  const listed = JSON.parse(text(await codexCall(app, 'default-list', { action: 'list_tools' })));
  assert.deepEqual(listed.servers.map(server => server.server), ['blender']);
  // With the Windows gate closed the call is refused without asking; the defaults are still checked for Claude below.
  if (!await gateClosed(app, servers.blender, 'echo', { text: 'Default active' })) {
    const called = await codexCall(app, 'default-echo', { action: 'call_tool', server: 'blender', tool: 'echo', arguments: { text: 'Default active' } }, 'approve_once');
    assert.equal(called.success, true);
    assert.match(text(called), /Default active/);
  }
  const model = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: `mcp ${JSON.stringify({ action: 'list_tools' })}`, clientMessageId: crypto.randomUUID(), model }), 202);
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude defaults', 15000);
  const seen = JSON.parse(done.messages.at(-1).text.replace(/^miniclaw_mcp error=false: /, ''));
  assert.deepEqual(seen.servers.map(server => server.server), ['blender', 'claudeonly']);
});
