// Checks how a conversation uses MCP servers: nothing is on until the user switches a server
// on, every tool asks first, and every call is shown in Activity.
import assert from 'node:assert/strict';
// Registers the gateway tool tests.
import { test } from 'node:test';
// Prepares a server folder outside MiniClaw's data.
import { copyFile, mkdtemp, readFile, realpath, rm } from 'node:fs/promises';
// Locates the temporary folder and the fake server.
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Locates the fake Claude CLI.
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
import { placeMcpServer } from './support/mcp-fixture.mjs';
import { MCP_GATE_CLOSED, assertGate, closedRefusal, sandboxReport } from './support/sandbox.mjs';
// Names the fake conversation and its turn.
import { THREAD, TURN, CLIENT, turnDone } from './support/fixtures.mjs';
// Names a second message.
import { randomUUID } from 'node:crypto';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const SECOND_TURN = '019a2222-2222-7222-8222-222222222223';
const text = result => result.contentItems.map(item => item.text).join('\n');

/** Starts a plain conversation on this Mac and saves one local server, switched on nowhere. */
async function fixture(t, extra = {}, mode) {
  const app = await launchApp();
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-')));
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: mcp.command,
    args: mcp.args(...(mode ? [mode] : [])), folder, ...extra }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  return { app, folder, server, mcp };
}

// Requirement change 2026-09-21: an agent's configuration says whether a server is allowed at all, so the
// plain conversations here get it from the main agent; a conversation only switches it off and back on.
const MAIN = '00000000-0000-0000-0000-000000000000';
/** Switches a server on or off for a scope. A rule names the server by its id, so a new server of the same name inherits nothing. */
const allow = async (app, server, kind, id, effect = 'allow') => {
  const saved = await json(await post(app, `/api/permissions/${kind}/${id}`, { target: { target_type: 'mcp_server', server: server.id }, effect }));
  // Requirement change 2026-09-22: an allowance makes a server available; the conversation switches it on itself.
  if (effect === 'allow' && kind !== 'conversation') await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  return saved;
};

/** Sends one gateway call as the model would and answers its approval, if one is expected. */
async function call(app, id, args, decide, turnId = TURN) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_mcp', threadId: THREAD, turnId, callId: id, arguments: args } });
  let approval;
  if (decide) {
    const snapshot = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'approval popup');
    approval = snapshot.scopedApproval;
    await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${approval.id}`, { action: decide }));
  }
  const state = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === id), `tool reply ${id}`, 15000);
  return { result: state.responses.find(reply => reply.id === id).result, approval };
}

// While Windows cannot enforce the sandbox, a confined local server is never started: its check
// answers 502 mcp_unreachable with GATE_CLOSED, list_tools names it unreachable with that reason, and
// a call is refused in those words without a popup. Only those reports select the Windows
// expectations, so each test checks the real server once the gate opens.
// Tells whether `server`, switched on in the conversation, meets the closed gate, and checks how it is refused.
async function gateClosed(app, server) {
  // The backend's own report, read without side effects: the conversation was offered no sandboxed
  // tool. With the gate open nothing extra runs, so the test takes exactly the Mac path.
  if (process.platform !== 'win32') return false;
  if (!await assertGate(app, !await sandboxOffered(app))) return false;
  const response = await app.request(`/api/mcp/servers/${server.id}/check`, { method: 'POST', body: {}, signal: AbortSignal.timeout(20000) });
  assert.equal(response.status, 502, 'a confined server is refused while the sandbox is unavailable');
  const GATE_CLOSED = await closedRefusal(app, MCP_GATE_CLOSED);
  const { error } = await response.json();
  assert.deepEqual([error.code, error.message], ['mcp_unreachable', GATE_CLOSED]);
  const listed = JSON.parse(text((await call(app, `gate-list-${server.name}`, { action: 'list_tools' })).result));
  assert.deepEqual(listed.servers.filter(entry => entry.server === server.name), [{ server: server.name, unreachable: GATE_CLOSED }]);
  const refused = (await call(app, `gate-call-${server.name}`, { action: 'call_tool', server: server.name, tool: 'echo', arguments: { text: 'hi' } })).result;
  assert.deepEqual([refused.success, text(refused)], [false, `The server "${server.name}" could not be reached. ${GATE_CLOSED}`]);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'a server that cannot start is never asked about');
  return true;
}
// Tells whether the conversation's thread/start offered MiniClaw's sandboxed file and command tool,
// which the backend does exactly when its sandbox gate is open (no shell allowance is set here).
async function sandboxOffered(app) {
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start');
  assert.ok(start, 'a conversation was started before the gate is read');
  return (start.params.dynamicTools ?? []).some(tool => tool.name === 'miniclaw_scoped_native');
}
// An absolute command that is never run: a stdio command must be an absolute path, and '/usr/bin/true' is not one on Windows.
const UNUSED_COMMAND = process.platform === 'win32' ? process.execPath : '/usr/bin/true';

test('every conversation on this Mac is offered the gateway tool, and nothing is reachable until a server is on', async t => {
  const { app } = await fixture(t);
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start');
  const tool = start.params.dynamicTools.find(tool => tool.name === 'miniclaw_mcp');
  assert.equal(tool.type, 'function');
  assert.deepEqual(tool.inputSchema.properties.action.enum, ['list_tools', 'describe_tool', 'call_tool']);
  const listed = await call(app, 'list-0', { action: 'list_tools' });
  assert.equal(listed.result.success, true);
  assert.match(text(listed.result), /no MCP server is switched on/i);
  const refused = await call(app, 'call-0', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'hi' } });
  assert.equal(refused.result.success, false);
  assert.match(text(refused.result), /not switched on/i);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'a server that is off is never asked about');
});

test('a server that is on lists its tools; each tool asks, once or for the session', async t => {
  const { app, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  const listed = await call(app, 'list-1', { action: 'list_tools' });
  const offered = JSON.parse(text(listed.result));
  assert.deepEqual(offered.servers.map(server => [server.server, server.tools.map(tool => tool.name)]),
    [['notes', ['probe', 'echo', 'write_note', 'fail', 'snapshot', 'dial', 'serve']]]);
  // Requirement change 2026-09-22: the list carries no schemas; describe_tool does.
  assert.equal(offered.servers[0].tools[1].inputSchema, undefined);
  const described = JSON.parse(text((await call(app, 'desc-1', { action: 'describe_tool', server: 'notes', tool: 'echo' })).result));
  assert.deepEqual([described.tool, described.inputSchema.required], ['echo', ['text']]);
  const first = await call(app, 'echo-1', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'hi' } }, 'approve_once');
  assert.deepEqual(first.approval.target, { target_type: 'mcp_tool', server: server.id, tool: 'echo' });
  assert.equal(first.approval.serverName, 'notes');
  assert.deepEqual(first.approval.arguments, { text: 'hi' }, 'the user sees what is sent before it is sent');
  assert.equal(first.approval.unrestricted, false);
  assert.deepEqual([first.result.success, text(first.result)], [true, 'echo: hi']);
  // "Once" is once: the same tool asks again. "For this session" stops it asking.
  await call(app, 'echo-2', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'again' } }, 'approve_session');
  const third = await call(app, 'echo-3', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'quiet' } });
  assert.equal(text(third.result), 'echo: quiet');
  // Another tool of the same server still asks, and a rejection is said in words.
  const rejected = await call(app, 'fail-1', { action: 'call_tool', server: 'notes', tool: 'fail', arguments: {} }, 'reject');
  assert.equal(rejected.result.success, false);
  assert.match(text(rejected.result), /user rejected/i);
  const failed = await call(app, 'fail-2', { action: 'call_tool', server: 'notes', tool: 'fail', arguments: {} }, 'approve_once');
  assert.deepEqual([failed.result.success, text(failed.result)], [false, 'It broke.']);
  const unknown = await call(app, 'nope-1', { action: 'call_tool', server: 'notes', tool: 'absent', arguments: {} });
  assert.match(text(unknown.result), /no tool named/i);
});

test('an agent or project switches a server on; the conversation has the last word', async t => {
  const app = await launchApp();
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-')));
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: mcp.command, args: mcp.args(), folder }), 201);
  const { agent } = await json(await post(app, '/api/agents', { name: 'Clerk', instructions: 'Keep notes.', active: true }), 201);
  await json(await post(app, `/api/permissions/agent/${agent.id}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  // Allowed is not on: the conversation switches the server on, and can do so during a turn.
  assert.match(text((await call(app, 'list-0', { action: 'list_tools' })).result), /no MCP server is switched on/i);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  if (await gateClosed(app, server)) return;
  assert.match(text((await call(app, 'list-a', { action: 'list_tools' })).result), /write_note/);
  // Switching it off during the turn ends nothing; the next call finds the server off, and on again after.
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: false }));
  assert.match(text((await call(app, 'list-b', { action: 'list_tools' })).result), /no MCP server is switched on/i);
  assert.ok((await json(await app.request(`/api/threads/${THREAD}`))).activeTurn, 'the turn is still running');
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  assert.match(text((await call(app, 'list-c', { action: 'list_tools' })).result), /write_note/);
  // The agent taking the server away ends the running turn, as every removal does, and the next turn finds it gone.
  await json(await post(app, `/api/permissions/agent/${agent.id}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'remove' }));
  await app.emit(turnDone('interrupted'));
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  const next = { id: SECOND_TURN, status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn: next }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: next } }] });
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: randomUUID() }), 202);
  assert.match(text((await call(app, 'list-d', { action: 'list_tools' }, undefined, SECOND_TURN)).result), /no MCP server is switched on/i);
  // A rule names a server by its id, never by a name a later server could reuse.
  assert.equal((await post(app, `/api/permissions/agent/${agent.id}`, { target: { target_type: 'mcp_server', server: 'notes' }, effect: 'allow' })).status, 400);
});

test('a sandboxed server cannot write outside its folder', async t => {
  const { app, folder, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  const outside = join(app.peer.dir, 'written-by-server.txt');
  const denied = await call(app, 'write-1', { action: 'call_tool', server: 'notes', tool: 'write_note', arguments: { path: outside, text: 'x' } }, 'approve_once');
  // Seatbelt refuses the write; the Linux sandbox has no such path in its view.
  assert.deepEqual([denied.result.success, text(denied.result)], [false, process.platform === 'linux' ? 'ENOENT' : 'EPERM']);
  const inside = await call(app, 'write-2', { action: 'call_tool', server: 'notes', tool: 'write_note', arguments: { path: join(folder, 'note.txt'), text: 'kept' } }, 'approve_once');
  assert.equal(inside.result.success, true);
  assert.equal(await readFile(join(folder, 'note.txt'), 'utf8'), 'kept');
});

test('an unrestricted server says so in its approval, and an edited server restarts', async t => {
  const { app, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  const before = await call(app, 'probe-1', { action: 'call_tool', server: 'notes', tool: 'probe', arguments: {} }, 'approve_once');
  assert.equal(before.approval.unrestricted, false);
  const { command, args, folder, name, kind } = server;
  await json(await post(app, `/api/mcp/servers/${server.id}`, { name, kind, command, args, folder, unrestricted: true, revision: 1 }));
  const after = await call(app, 'probe-2', { action: 'call_tool', server: 'notes', tool: 'probe', arguments: {} }, 'approve_once');
  assert.equal(after.approval.unrestricted, true);
  // Deleting the server ends its use at once.
  await json(await post(app, `/api/mcp/servers/${server.id}/delete`, { revision: 2 }));
  const gone = await call(app, 'probe-3', { action: 'call_tool', server: 'notes', tool: 'probe', arguments: {} });
  assert.equal(gone.result.success, false);
  // A new server of the same name starts switched off: the old server's rules went with it.
  await json(await post(app, '/api/mcp/servers', { name, kind, command, args, folder }), 201);
  assert.match(text((await call(app, 'list-z', { action: 'list_tools' })).result), /no MCP server is switched on/i);
});

test('every MCP call is shown in Activity, with what was sent and what came back', async t => {
  const { app, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  await call(app, 'list-x', { action: 'list_tools' });
  await call(app, 'echo-x', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'seen' } }, 'approve_once');
  await call(app, 'echo-y', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'no' } }, 'reject');
  await call(app, 'bad-x', { action: 'dance' });
  const { activities } = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(activities.map(entry => [entry.id, entry.kind, entry.status]),
    [['list-x', 'mcp', 'ok'], ['echo-x', 'mcp', 'ok'], ['echo-y', 'mcp', 'refused'], ['bad-x', 'other', 'refused']]);
  const shown = activities[1];
  assert.deepEqual([shown.title, shown.detail, shown.output], ['notes: echo', '{"text":"seen"}', 'echo: seen']);
});

test('a coding session asks about every MCP call, even one approved for the session', async t => {
  const { app, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  await call(app, 'echo-s', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'a' } }, 'approve_session');
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow' }));
  const asked = await call(app, 'echo-t', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'b' } }, 'approve_once');
  assert.equal(asked.approval.target.tool, 'echo');
  // With full access on as well, full access wins, however the two rules came to be on together.
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: { target_type: 'feature', name: 'full_access' }, effect: 'allow' }));
  assert.equal(text((await call(app, 'echo-u', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'c' } })).result), 'echo: c');
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
});

test('a long tool list stays short: a name and a line per tool, a query narrows it, describe_tool has the schema, and a wrong name suggests the right one', async t => {
  const { app, server } = await fixture(t, {}, 'many');
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  const listed = text((await call(app, 'list-m', { action: 'list_tools' })).result);
  assert.ok(listed.length < 24 * 1024, `the list is ${listed.length} bytes`);
  const offered = JSON.parse(listed);
  const play = offered.servers[0].tools.find(tool => tool.name === 'play_test_start');
  assert.deepEqual([offered.servers[0].tools.length, play.summary, play.inputSchema, play.description],
    [128, 'Starts a play test of the open place.', undefined, undefined]);
  assert.match(offered.note, /describe_tool/);
  // A query keeps the tools whose name or description carries the words.
  const narrowed = JSON.parse(text((await call(app, 'list-q', { action: 'list_tools', query: 'play test' })).result));
  assert.deepEqual(narrowed.servers[0].tools.map(tool => tool.name), ['play_test_start']);
  const none = JSON.parse(text((await call(app, 'list-n', { action: 'list_tools', query: 'teleport' })).result));
  assert.deepEqual(none.servers[0].tools, []);
  // One tool's full description and schema come on request.
  const described = JSON.parse(text((await call(app, 'desc-m', { action: 'describe_tool', server: 'notes', tool: 'play_test_start' })).result));
  assert.deepEqual([described.server, described.tool, described.inputSchema.required, described.description.includes('play_test_stop')],
    ['notes', 'play_test_start', ['mode'], true]);
  // A guessed name is refused with the names that are close, and asks nobody.
  const guessed = await call(app, 'guess-m', { action: 'call_tool', server: 'notes', tool: 'play_test', arguments: {} });
  assert.equal(guessed.result.success, false);
  assert.match(text(guessed.result), /no tool named "play_test".*play_test_start/s);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
  assert.match(text((await call(app, 'desc-x', { action: 'describe_tool', server: 'notes', tool: 'thing' })).result), /thing_0/);
});

test('a Claude conversation reaches the same servers through the same approval', async t => {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-')));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)),
    MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: mcp.command,
    args: mcp.args(), folder }), 201);
  const MODEL = 'claude-sonnet-5';
  // A plain Claude conversation has no file tool, and still gets the gateway.
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  const ask = async (prompt, decide) => {
    await json(await post(app, `/api/threads/${thread.id}/messages`, { text: prompt, clientMessageId: randomUUID(), model: MODEL }), 202);
    let decided = false;
    const snapshot = await eventually(async () => {
      const value = await json(await app.request(`/api/threads/${thread.id}`));
      if (decide && !decided && value.scopedApproval) {
        decided = true;
        await json(await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: decide }));
      }
      return value;
    }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
    return snapshot.messages.at(-1).text;
  };
  const lastRun = async () => JSON.parse((await readFile(join(folder, 'calls.jsonl'), 'utf8')).trim().split('\n').at(-1)).args;
  // With no server switched on, a Claude turn has no gateway. With web search and questions off too, a turn
  // with no tool at all stays in safe mode.
  // Expectation corrected 2026-10-04: since the 2026-09-26 main-agent timed-shell change (TESTING.md) every Mac
  // host chat carries the native tool schema, so its shim is configured and safe mode is off. Baseline 20595a9
  // already behaved this way and failed the earlier assertion. A Windows host chat carries the schema too
  // when its sandbox is available; without the sandbox and without a shell allowance it has no tool and
  // keeps safe mode. The backend's own report says which.
  for (const name of ['no_web_search', 'no_questions']) {
    await json(await post(app, `/api/permissions/conversation/${thread.id}`, { target: { target_type: 'feature', name }, effect: 'allow' }));
  }
  await ask('Hello');
  const locked = await lastRun();
  const schema = process.platform !== 'win32' || (await sandboxReport(app)).available;
  assert.deepEqual([locked.includes('--safe-mode'), locked.includes('--mcp-config')], [!schema, schema]);
  assert.equal(locked.includes('mcp__miniclaw__miniclaw_mcp'), false, 'no gateway while no server is switched on');
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, `/api/threads/${thread.id}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  // Asking stays switched off in this conversation, so the gateway is the only tool beside the
  // native schema a Mac host chat always carries.
  assert.equal(await ask('tools'), schema ? 'miniclaw_scoped_native error=false: miniclaw_scoped_native,miniclaw_mcp'
    : 'miniclaw_mcp error=false: miniclaw_mcp');
  const echoed = await ask(`mcp ${JSON.stringify({ action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'via Claude' } })}`, 'approve_once');
  // While Windows cannot enforce the sandbox, a confined stdio server is never started; the gateway
  // says so instead. Only that report selects this branch, so the call is checked once the gate opens.
  if (process.platform === 'win32' && echoed.startsWith(`miniclaw_mcp error=true: The server "notes" could not be reached. ${MCP_GATE_CLOSED}`)) {
    const refused = `miniclaw_mcp error=true: The server "notes" could not be reached. ${await closedRefusal(app, MCP_GATE_CLOSED)}`;
    assert.equal(echoed, refused);
    assert.equal(await ask(`mcp ${JSON.stringify({ action: 'describe_tool', server: 'notes', tool: 'echo' })}`), refused);
  } else {
    assert.equal(echoed, 'miniclaw_mcp error=false: echo: via Claude');
    const described = await ask(`mcp ${JSON.stringify({ action: 'describe_tool', server: 'notes', tool: 'echo' })}`);
    assert.deepEqual(JSON.parse(described.replace('miniclaw_mcp error=false: ', '')).inputSchema.required, ['text']);
  }
  // The CLI is allowed MiniClaw's gateway by name and is given no server of the user's.
  const last = await lastRun();
  assert.ok(last.includes('mcp__miniclaw__miniclaw_mcp'));
  assert.deepEqual(Object.keys(JSON.parse(last[last.indexOf('--mcp-config') + 1]).mcpServers), ['miniclaw']);
  assert.equal(JSON.stringify(last).includes(mcp.file), false);
});

test('a Computer Use conversation on this Mac keeps the MCP gateway and the question tool, never the file tool', async t => {
  process.env.MINICLAW_HELPER_BIN = '/usr/bin/true';
  const app = await launchApp({}, { env: { MINICLAW_HELPER_BIN: '/usr/bin/true' } });
  t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'blender', kind: 'stdio', command: UNUSED_COMMAND }), 201);
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  // The expectation follows the backend's report: a Windows helper must be a configured
  // codex-computer-use.exe, which /usr/bin/true is not, so there Computer Use is unavailable and no
  // conversation starts. A machine with a helper checks the conversation's tools.
  const created = await post(app, '/api/threads', { environment: { kind: 'host' }, computerUse: true });
  if (process.platform === 'win32' && created.status === 503) {
    const { error } = await json(created, 503);
    assert.deepEqual([error.code, error.message], ['computer_unavailable', 'The installed Computer Use helper is unavailable.']);
    assert.equal((await app.inspect()).requests.some(request => request.method === 'thread/start'), false);
    return;
  }
  await json(created, 201);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start').params;
  assert.deepEqual((start.dynamicTools ?? []).map(tool => tool.name), ['miniclaw_mcp', 'miniclaw_ask']);
  assert.match(start.baseInstructions, /Computer Use is on/);
  assert.match(start.baseInstructions, /miniclaw_mcp/);
  // What each message says about servers matches the tools the conversation really has.
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const note = (await app.inspect()).requests.find(request => request.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(note, /MCP servers: blender \(switched on; reach them only through the miniclaw_mcp tool/);
});

test('a sandboxed server reaches only the local ports it was given, and never MiniClaw itself', async t => {
  const { createServer } = await import('node:net');
  const listen = () => new Promise(resolve => { const server = createServer(socket => socket.end()); server.listen(0, '127.0.0.1', () => resolve(server)); });
  const [allowed, other] = [await listen(), await listen()];
  const { app, folder, mcp } = await fixture(t, { name: 'unused' });
  t.after(() => { allowed.close(); other.close(); });
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'dialer', kind: 'stdio', command: mcp.command,
    args: mcp.args(), folder, ports: [allowed.address().port] }), 201);
  assert.deepEqual(server.ports, [allowed.address().port]);
  await allow(app, server, 'agent', MAIN);
  const dial = (id, port) => call(app, id, { action: 'call_tool', server: 'dialer', tool: 'dial', arguments: { port } }, 'approve_once');
  // With the Windows gate closed the server never runs; what may be saved is still checked below.
  if (!await gateClosed(app, server)) {
    assert.equal(text((await dial('dial-1', allowed.address().port)).result), 'connected');
    // On Linux the server has its own network, where nothing listens on another port.
    assert.equal(text((await dial('dial-2', other.address().port)).result), process.platform === 'linux' ? 'ECONNREFUSED' : 'EPERM',
      'another port on this Mac stays closed');
  }
  // MiniClaw's own port, a privileged port and a remote server's fields are refused when saving.
  const own = Number(new URL(app.url).port);
  for (const bad of [{ ports: [own] }, { ports: [80] }, { ports: [70000] }, { ports: Array.from({ length: 9 }, (_, index) => 9000 + index) }]) {
    assert.equal((await post(app, '/api/mcp/servers', { name: 'bad', kind: 'stdio', command: UNUSED_COMMAND, ...bad })).status, 400, JSON.stringify(bad));
  }
  assert.equal((await post(app, '/api/mcp/servers', { name: 'far', kind: 'http', url: 'https://mcp.example.com/mcp', ports: [9876] })).status, 400);
});

test('a sandboxed server listens only on the ports it was given, and an app on this Mac can reach it', async t => {
  const { connect, createServer } = await import('node:net');
  const free = () => new Promise(resolve => { const probe = createServer(); probe.listen(0, '127.0.0.1', () => { const { port } = probe.address(); probe.close(() => resolve(port)); }); });
  const [given, other] = [await free(), await free()];
  const { app, folder, mcp } = await fixture(t, { name: 'unused' });
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'host', kind: 'stdio', command: mcp.command,
    args: mcp.args(), folder, listen: [given] }), 201);
  assert.deepEqual(server.listen, [given]);
  await allow(app, server, 'agent', MAIN);
  const serve = (id, port) => call(app, id, { action: 'call_tool', server: 'host', tool: 'serve', arguments: { port } }, 'approve_once');
  // With the Windows gate closed the server never runs; what may be saved is still checked below.
  if (process.platform === 'win32' && !await gateClosed(app, server)) {
    // Known gap on Windows (docs/windows/sandbox-review.md, "Declared listeners"): inbound loopback to a
    // sandboxed listener is dropped outside MiniClaw's filters, so a server that declares a listening
    // port is refused before it starts, in these words and without a popup. Nothing listens.
    const refused = (await call(app, 'serve-1', { action: 'call_tool', server: 'host', tool: 'serve', arguments: { port: given } })).result;
    assert.deepEqual([refused.success, text(refused)], [false, 'The server "host" could not be reached. Listening ports for sandboxed programs '
      + 'are not supported on Windows yet. Remove the listening ports, or run the program without the sandbox.']);
    await assert.rejects(new Promise((resolve, reject) => { const socket = connect({ host: '127.0.0.1', port: given }); socket.on('connect', resolve); socket.on('error', reject); }),
      { code: 'ECONNREFUSED' }, 'no listener was started');
  } else if (process.platform !== 'win32') {
    // Bound, and reachable by the server's own client too: Roblox's proxy dials its own host.
    assert.equal(text((await serve('serve-1', given)).result), 'listening, self-connected');
    // The app's side: a plain client on this Mac connects to what the server serves.
    const greeted = await new Promise((resolve, reject) => { const socket = connect({ host: '127.0.0.1', port: given }); let got = '';
      socket.on('data', piece => { got += piece; }); socket.on('end', () => resolve(got)); socket.on('error', reject); });
    assert.equal(greeted, 'served\n');
    if (process.platform === 'linux') {
      // The server may bind any port inside its own network, but only a declared one is bridged
      // to this machine: an app here cannot reach the other.
      await serve('serve-2', other);
      await assert.rejects(new Promise((resolve, reject) => { const socket = connect({ host: '127.0.0.1', port: other }); socket.on('connect', resolve); socket.on('error', reject); }),
        { code: 'ECONNREFUSED' }, 'an undeclared listener is not reachable');
    } else {
      assert.equal(text((await serve('serve-2', other)).result), 'EPERM', 'another port cannot be listened on');
    }
  }
  const own = Number(new URL(app.url).port);
  for (const bad of [{ listen: [own] }, { listen: [80] }, { listen: [given, given] }, { listen: Array.from({ length: 9 }, (_, index) => 9000 + index) },
    { listen: [given], ports: [given] }]) {
    assert.equal((await post(app, '/api/mcp/servers', { name: 'bad', kind: 'stdio', command: UNUSED_COMMAND, ...bad })).status, 400, JSON.stringify(bad));
  }
  assert.equal((await post(app, '/api/mcp/servers', { name: 'far', kind: 'http', url: 'https://mcp.example.com/mcp', listen: [9876] })).status, 400);
});

// Requirement change 2026-09-22: a coding session beside full access reviews file changes only, so an MCP tool still runs unasked.
test('with full access an MCP tool runs without its popup, with or without a coding session', async t => {
  const { app, server } = await fixture(t);
  await allow(app, server, 'agent', MAIN);
  if (await gateClosed(app, server)) return;
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  const quiet = await call(app, 'fa-echo', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'unasked' } });
  assert.equal(text(quiet.result), 'echo: unasked');
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'coding_session', on: true }));
  const still = await call(app, 'cs-echo', { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'reviewed' } });
  assert.equal(text(still.result), 'echo: reviewed');
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
});
