import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchApp, json, eventually } from './support/harness.mjs';
import { TOOLS_UNAVAILABLE, assertGate, closedRefusal } from './support/sandbox.mjs';
import { THREAD, TURN, CLIENT, turnDone } from './support/fixtures.mjs';

const windows = process.platform === 'win32';
// Quotes PowerShell literals without interpreting user paths.
const ps = value => `'${value.replaceAll("'", "''")}'`;

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });

// Starts a host chat bound to an agent that may read and write one real folder.
async function agentChat(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-host-tools-')));
  await mkdir(join(root, 'work'));
  await mkdir(join(root, 'secret'));
  await writeFile(join(root, 'work/notes.txt'), 'work-content');
  await writeFile(join(root, 'secret/key.txt'), 'secret-content');
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { agent } = await json(await post(app, '/api/agents',
    { name: 'Editor', instructions: 'Edit the work folder.', active: true }), 201);
  for (const action of ['read', 'write']) {
    await json(await post(app, `/api/permissions/agent/${agent.id}`,
      { target: { target_type: 'folder', path: join(root, 'work'), action }, effect: 'allow' }));
  }
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`,
    { text: 'Go', clientMessageId: CLIENT }), 202);
  return { app, root, agent };
}

// Sends one model tool call and returns Codex's view of the result.
async function call(app, id, args, decide) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: THREAD, turnId: TURN, callId: id, arguments: args } });
  if (decide) {
    const snapshot = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
      value => value.scopedApproval, 'approval popup');
    await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${snapshot.scopedApproval.id}`,
      { action: decide }));
  }
  const state = await eventually(() => app.inspect(),
    value => value.responses.some(reply => reply.id === id), `tool reply ${id}`, windows ? 15000 : 4000);
  return state.responses.find(reply => reply.id === id).result;
}
const text = result => result.contentItems.map(item => item.text).join('\n');

// While Windows cannot enforce the sandbox, a conversation is offered no sandboxed tool: a Codex
// chat's dynamic tools lack miniclaw_scoped_native, a call that names it anyway is refused with
// SANDBOX_UNAVAILABLE and no popup, and a Claude chat's MiniClaw server runs without the file tool.
// Only those reports select the Windows expectations, so every test checks the real tool once the
// gate opens. The timed unsandboxed shell at the end of this file works with the gate closed.
// A path as a sandboxed command names it: the Mac's own path; on Windows, where the bundled POSIX
// shell would take backslashes for escapes, the same path with forward slashes, quoted.
const shArg = path => windows ? `'${path.replaceAll('\\', '/').replaceAll("'", "'\\''")}'` : path;
// What the sandbox answers a read it refuses: Seatbelt's words on the Mac, the shell's on Windows.
// On Linux a path outside the grants is not in the sandbox's view at all.
const DENIED = windows ? /Permission denied/ : process.platform === 'linux' ? /No such file or directory/ : /Operation not permitted/;
async function gateClosed(app, probe) {
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  const names = start.params.dynamicTools.map(tool => tool.name);
  if (!windows) return false;
  if (!await assertGate(app, !names.includes('miniclaw_scoped_native'))) return false;
  // Every other tool stays: the MCP gateway, the question tool and, in a project, its memory.
  assert.deepEqual(names.filter(name => name !== 'miniclaw_project'), ['miniclaw_mcp', 'miniclaw_ask']);
  const refused = await call(app, `gate-closed-${probe.action}`, probe);
  assert.equal(refused.success, false);
  assert.equal(text(refused), await closedRefusal(app, TOOLS_UNAVAILABLE));
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.scopedApproval, null);
  assert.equal(snapshot.review ?? null, null);
  return true;
}

test('a host agent chat is offered the MiniClaw tool', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'list_folder', path: join(root, 'work') })) return;
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  // Requirement change 2026-09-21: every conversation on this Mac also gets the MCP gateway, which reaches nothing until a server is switched on.
  assert.deepEqual(start.params.dynamicTools.map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask']);
  assert.equal(start.params.sandbox, 'read-only');
});

test('granted folder files are read and written without a prompt', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'read_file', path: join(root, 'work/notes.txt') })) return;
  const read = await call(app, 'read-1', { action: 'read_file', path: join(root, 'work/notes.txt') });
  assert.equal(read.success, true);
  assert.equal(text(read), 'work-content');
  const write = await call(app, 'write-1',
    { action: 'write_file', path: join(root, 'work/notes.txt'), text: 'edited' });
  assert.equal(write.success, true);
  assert.equal(await readFile(join(root, 'work/notes.txt'), 'utf8'), 'edited');
});

test('a granted folder can be listed, searched and edited in place; another folder asks', async t => {
  const { app, root } = await agentChat(t);
  const work = join(root, 'work');
  if (await gateClosed(app, { action: 'list_folder', path: work })) return;
  const listed = await call(app, 'list-1', { action: 'list_folder', path: work });
  assert.equal(listed.success, true);
  assert.equal(text(listed), 'notes.txt\tfile\t12');
  const found = await call(app, 'search-1', { action: 'search', path: work, text: 'work-' });
  assert.equal(text(found), `${join(work, 'notes.txt')}:1: work-content`);
  const edited = await call(app, 'edit-1', { action: 'replace_in_file',
    path: join(work, 'notes.txt'), find: 'work-content', text: 'edited in place' });
  assert.equal(edited.success, true);
  assert.equal(await readFile(join(work, 'notes.txt'), 'utf8'), 'edited in place');
  // A passage that is not there changes nothing and says why.
  const missed = await call(app, 'edit-2', { action: 'replace_in_file',
    path: join(work, 'notes.txt'), find: 'absent', text: 'x' });
  assert.equal(missed.success, false);
  assert.match(text(missed), /not found/);
  // Listing a folder without a grant goes through the approval popup like a file.
  const refused = await call(app, 'list-secret', { action: 'list_folder', path: join(root, 'secret') }, 'reject');
  assert.equal(refused.success, false);
  assert.equal(JSON.stringify(refused).includes('key.txt'), false);
});

test('network is off until switched on, then hosts are approved with the command', async t => {
  const { app, root } = await agentChat(t);
  const work = join(root, 'work');
  const command = { action: 'run_shell', cwd: work, hosts: ['registry.npmjs.org'],
    text: 'echo "proxy:$HTTPS_PROXY" | grep -c 127.0.0.1' };
  if (await gateClosed(app, command)) return;
  // Off by default: the model is told why, and the user is not asked anything.
  const off = await call(app, 'net-off', command);
  assert.equal(off.success, false);
  assert.match(text(off), /Network is off for this conversation/);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
  const feature = { target_type: 'feature', name: 'network' };
  // Requirement change 2026-09-21: an agent's configuration says whether a capability is allowed at all; the conversation only switches it.
  const [editor] = (await json(await app.request('/api/agents'))).agents;
  await json(await post(app, `/api/permissions/agent/${editor.id}`, { target: feature, effect: 'allow' }));
  // Requirement change 2026-09-22: allowed is not on; the conversation switches it on itself.
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'network', on: true }));
  // One popup shows the command together with the hosts it wants.
  await app.emit({ id: 'net-on', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: THREAD, turnId: TURN, callId: 'net-on', arguments: command } });
  const asked = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
    value => value.scopedApproval, 'approval popup');
  assert.equal(asked.scopedApproval.target.target_type, 'command');
  assert.deepEqual(asked.scopedApproval.hosts, ['registry.npmjs.org']);
  assert.deepEqual(asked.scopedApproval.domains, ['npmjs.org']);
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${asked.scopedApproval.id}`,
    { action: 'approve_once' }));
  const ran = (await eventually(() => app.inspect(),
    value => value.responses.some(reply => reply.id === 'net-on'), 'tool reply'))
    .responses.find(reply => reply.id === 'net-on').result;
  assert.equal(ran.success, true);
  assert.match(text(ran), /stdout:\n1\n/, 'the command saw its proxy');
  // A conversation switches network off again by itself; off is the absence of its own allow (2026-09-22).
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: feature, effect: 'block' }));
  const saved = await json(await app.request(`/api/permissions/conversation/${THREAD}`));
  const network = saved.effective.find(entry => entry.target.target_type === 'feature');
  assert.equal(network.decision, 'ask');
});

test('a file outside the grants asks, and rejection returns nothing', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'read_file', path: join(root, 'secret/key.txt') })) return;
  const result = await call(app, 'read-secret',
    { action: 'read_file', path: join(root, 'secret/key.txt') }, 'reject');
  assert.equal(result.success, false);
  assert.equal(JSON.stringify(result).includes('secret-content'), false);
});

test('an approved shell command runs inside the Seatbelt profile', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'run_shell', text: 'cat notes.txt', cwd: join(root, 'work') })) return;
  const script = `cat notes.txt; cat ${shArg(join(root, 'secret/key.txt'))}; echo made > made.txt`;
  const result = await call(app, 'shell-1',
    { action: 'run_shell', text: script, cwd: join(root, 'work') }, 'approve_once');
  assert.equal(result.success, true);
  assert.match(text(result), /work-content/);
  assert.match(text(result), DENIED);
  assert.equal(text(result).includes('secret-content'), false);
  assert.equal(await readFile(join(root, 'work/made.txt'), 'utf8'), 'made\n');
});

test('AppleScript is refused on the host before any prompt', async t => {
  const { app, root } = await agentChat(t);
  const result = await call(app, 'script-1',
    { action: 'run_applescript', text: 'return 1', cwd: join(root, 'work') });
  assert.equal(result.success, false);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
});

// The same tool, policy and sandbox through the Claude provider's MCP shim.
const claudeBin = fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url));
const MODEL = 'claude-sonnet-5';

// Starts a Claude chat bound to an agent with one readable folder.
async function claudeAgentChat(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-claude-tools-')));
  await mkdir(join(root, 'work'));
  await mkdir(join(root, 'secret'));
  await writeFile(join(root, 'work/notes.txt'), 'work-content');
  await writeFile(join(root, 'secret/key.txt'), 'secret-content');
  const app = await launchApp({}, { env: { CLAUDE_BIN: claudeBin, MC_CLAUDE_LOG: join(root, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { agent } = await json(await post(app, '/api/agents',
    { name: 'Reader', instructions: 'Read the work folder.', active: true, model: MODEL }), 201);
  await json(await post(app, `/api/permissions/agent/${agent.id}`,
    { target: { target_type: 'folder', path: join(root, 'work'), action: 'read' }, effect: 'allow' }));
  const { thread } = await json(await post(app, '/api/threads',
    { environment: { kind: 'host' }, agentId: agent.id, model: MODEL }), 201);
  return { app, root, thread };
}

// Sends one prompt, answers any approval, and returns the assistant's final text.
async function ask(app, thread, prompt, decide) {
  await json(await post(app, `/api/threads/${thread.id}/messages`,
    { text: prompt, clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  let decided = false;
  const snapshot = await eventually(async () => {
    const value = await json(await app.request(`/api/threads/${thread.id}`));
    if (decide && !decided && value.scopedApproval) {
      decided = true;
      await json(await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`,
        { action: decide }));
    }
    return value;
  }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant',
  'Claude tool reply', 10000);
  return snapshot.messages.at(-1).text;
}

// The Claude side of the closed gate: its MiniClaw server is started without the file tool and the
// model is not allowed to call it, so the fake model's first tool is another one.
async function claudeGateClosed(root, answer) {
  const args = (await readFile(join(root, 'calls.jsonl'), 'utf8')).trim().split('\n').map(l => JSON.parse(l)).at(-1).args;
  const servers = JSON.parse(args[args.indexOf('--mcp-config') + 1]).mcpServers;
  if (!windows || servers.miniclaw.env.MINICLAW_NO_FILE_TOOL !== '1') return false;
  assert.equal(args.includes('mcp__miniclaw__miniclaw_scoped_native'), false);
  if (answer !== undefined) assert.doesNotMatch(answer, /miniclaw_scoped_native/);
  return true;
}

test('a Claude agent chat reads a granted file through the MCP shim', async t => {
  const { app, root, thread } = await claudeAgentChat(t);
  const answer = await ask(app, thread,
    `tool ${JSON.stringify({ action: 'read_file', path: join(root, 'work/notes.txt') })}`);
  if (await claudeGateClosed(root, answer)) return;
  assert.equal(answer, 'miniclaw_scoped_native error=false: work-content');
  const [call] = (await readFile(join(root, 'calls.jsonl'), 'utf8')).trim().split('\n').map(l => JSON.parse(l));
  assert.ok(call.args.includes('--strict-mcp-config'));
  // Requirement change 2026-09-21: web search is on by default, so it is named beside MiniClaw's tool.
  const after = call.args.slice(call.args.indexOf('--allowedTools') + 1);
  const allowed = after.slice(0, after.findIndex(arg => arg.startsWith('--')));
  assert.deepEqual(allowed.filter(name => !name.startsWith('mcp__miniclaw__miniclaw_mcp')).sort(), ['WebSearch', 'mcp__miniclaw__miniclaw_ask', 'mcp__miniclaw__miniclaw_scoped_native']);
  assert.equal(call.args[call.args.indexOf('--tools') + 1], 'WebSearch');
  // Safe mode would disable the MCP server, so settings files are excluded instead.
  assert.equal(call.args.includes('--safe-mode'), false);
  assert.equal(call.args[call.args.indexOf('--setting-sources') + 1], '');
});

test('a Claude shell command is approved by the user and confined by Seatbelt', async t => {
  const { app, root, thread } = await claudeAgentChat(t);
  const answer = await ask(app, thread, `tool ${JSON.stringify({ action: 'run_shell',
    text: `cat notes.txt; cat ${shArg(join(root, 'secret/key.txt'))}`, cwd: join(root, 'work') })}`, 'approve_once');
  if (await claudeGateClosed(root, answer)) return;
  assert.match(answer, /error=false/);
  assert.match(answer, /work-content/);
  assert.match(answer, DENIED);
  assert.equal(answer.includes('secret-content'), false);
});

test('a rejected Claude tool call returns an error and no content', async t => {
  const { app, root, thread } = await claudeAgentChat(t);
  const answer = await ask(app, thread,
    `tool ${JSON.stringify({ action: 'read_file', path: join(root, 'secret/key.txt') })}`, 'reject');
  if (await claudeGateClosed(root, answer)) return;
  assert.match(answer, /error=true/);
  assert.equal(answer.includes('secret-content'), false);
});

test('a plain Claude chat carries tools but still needs permission', async t => {
  const { app, root } = await claudeAgentChat(t);
  const { thread } = await json(await post(app, '/api/threads',
    { environment: { kind: 'host' }, model: MODEL }), 201);
  await ask(app, thread, 'Hello');
  const calls = (await readFile(join(root, 'calls.jsonl'), 'utf8')).trim().split('\n').map(l => JSON.parse(l));
  // Main-agent chats now carry the shell tool so an explicit timed grant can activate it.
  const args = calls.at(-1).args;
  const servers = JSON.parse(args[args.indexOf('--mcp-config') + 1]).mcpServers;
  assert.deepEqual(Object.keys(servers), ['miniclaw']);
  if (await claudeGateClosed(root)) return;
  assert.notEqual(servers.miniclaw.env.MINICLAW_NO_FILE_TOOL, '1');
  assert.equal(args.includes('mcp__miniclaw__miniclaw_scoped_native'), true);
});

// A project's workspace folder is the agent's home: granted without a rule or a prompt.
async function projectChat(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-project-tools-')));
  await mkdir(join(root, 'workspace'));
  await mkdir(join(root, 'elsewhere'));
  await writeFile(join(root, 'workspace/readme.txt'), 'project-content');
  await writeFile(join(root, 'elsewhere/private.txt'), 'private-content');
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { project } = await json(await post(app, '/api/projects', { name: 'Site',
    environment: { kind: 'host' }, workspace: join(root, 'workspace') }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  return { app, root, project };
}

test('a project chat reads and writes its workspace without rules or prompts', async t => {
  const { app, root } = await projectChat(t);
  if (await gateClosed(app, { action: 'read_file', path: join(root, 'workspace/readme.txt') })) return;
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  assert.ok(start.params.baseInstructions.includes(join(root, 'workspace')), 'the briefing names the working folder');
  const read = await call(app, 'p-read', { action: 'read_file', path: join(root, 'workspace/readme.txt') });
  assert.equal(text(read), 'project-content');
  const write = await call(app, 'p-write',
    { action: 'write_file', path: join(root, 'workspace/readme.txt'), text: 'updated' });
  assert.equal(write.success, true);
  const outside = await call(app, 'p-outside',
    { action: 'read_file', path: join(root, 'elsewhere/private.txt') }, 'reject');
  assert.equal(outside.success, false);
});

test('a sandboxed command can use its workspace and a private temp directory', async t => {
  const { app, root } = await projectChat(t);
  if (await gateClosed(app, { action: 'run_shell', text: 'echo built > out.txt', cwd: join(root, 'workspace') })) return;
  const script = [`echo built > out.txt`, `cat <<'END' > "$TMPDIR/scratch"`, 'temp-content', 'END',
    `cat "$TMPDIR/scratch"`, `cat ${shArg(join(root, 'elsewhere/private.txt'))}`, 'echo "tmp=$TMPDIR"'].join('\n');
  const result = await call(app, 'p-shell',
    { action: 'run_shell', text: script, cwd: join(root, 'workspace') }, 'approve_once');
  assert.equal(result.success, true);
  assert.match(text(result), /temp-content/);
  assert.match(text(result), DENIED);
  assert.equal(text(result).includes('private-content'), false);
  assert.equal(await readFile(join(root, 'workspace/out.txt'), 'utf8'), 'built\n');
  // Requirement change 2026-09-22: a project conversation's commands run in its shared shell, whose
  // temp folder lives as long as the shell and goes with it. It is private to the shell either way.
  const temp = /tmp=(\S+)/.exec(text(result))[1];
  assert.equal(await readFile(join(temp, 'scratch'), 'utf8'), 'temp-content\n');
  await json(await post(app, `/api/threads/${THREAD}/terminal/close`));
  // On Windows the closed shell's sandbox is taken apart by the cleanup worker just after the close answers.
  if (windows) await eventually(() => readFile(join(temp, 'scratch')).then(() => 'still there', error => error.code), code => code === 'ENOENT', 'the temp directory goes with the shell', 30000);
  else await assert.rejects(readFile(join(temp, 'scratch')), { code: 'ENOENT' }, 'the temp directory goes with the shell');
});

test('a conversation block inside the workspace still wins', async t => {
  const { app, root } = await projectChat(t);
  if (await gateClosed(app, { action: 'read_file', path: join(root, 'workspace/readme.txt') })) return;
  await json(await post(app, `/api/permissions/conversation/${THREAD}`,
    { target: { target_type: 'file', path: join(root, 'workspace/readme.txt'), action: 'read' }, effect: 'block' }));
  const read = await call(app, 'p-blocked', { action: 'read_file', path: join(root, 'workspace/readme.txt') });
  assert.equal(read.success, false);
});

test('a workspace must be an absolute existing folder on this Mac', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  for (const workspace of ['relative/path', '/definitely/not/here', '/tmp/../etc']) {
    assert.equal((await post(app, '/api/projects',
      { name: 'Bad', environment: { kind: 'host' }, workspace })).status, 400, workspace);
  }
});

// User report 2026-10-05 (Windows, Claude, project chat): the model sent its provider's own working
// directory, MiniClaw's chat folder, as cwd; the sandbox could not enter it and the command failed at
// `cd`; a retry without cwd was refused as unreadable. Now a command without cwd runs in the project's
// folder, and on Windows a folder the sandbox may not enter is refused before anything starts, naming
// the folder to use. The model and the activity row get the same reason.
test('a project command without cwd runs in the project folder, and the chat folder is not taken for it', async t => {
  const { app, root } = await projectChat(t);
  const workspace = join(root, 'workspace');
  if (await gateClosed(app, { action: 'run_shell', text: 'pwd', cwd: workspace })) return;
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  const bare = await call(app, 'p-no-cwd', { action: 'run_shell', text: 'pwd; echo hi > hi.txt' });
  assert.equal(bare.success, true, text(bare));
  assert.equal(await readFile(join(workspace, 'hi.txt'), 'utf8'), 'hi\n');
  const named = await call(app, 'p-cwd', { action: 'run_shell', text: 'echo again > again.txt', cwd: workspace });
  assert.equal(named.success, true, text(named));
  assert.equal(await readFile(join(workspace, 'again.txt'), 'utf8'), 'again\n');
  const chatFolder = await realpath(app.peer.cwd);
  const wrong = await call(app, 'p-chat-folder', { action: 'run_shell', text: 'echo x > x.txt', cwd: chatFolder });
  await assert.rejects(readFile(join(chatFolder, 'x.txt')), { code: 'ENOENT' });
  const { activities } = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(activities.filter(a => a.id === 'p-no-cwd').map(a => [a.kind, a.title, a.detail, a.status]),
    [['command', 'pwd; echo hi > hi.txt', workspace, 'ok']]);
  if (windows) {
    assert.equal(wrong.success, false);
    assert.match(text(wrong), /outside the folders this conversation may use/);
    assert.ok(text(wrong).includes(workspace), text(wrong));
    const row = activities.find(a => a.id === 'p-chat-folder');
    assert.deepEqual([row.status, row.output], ['refused', text(wrong)]);
  }
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'nothing was asked');
});

// Requirement change 2026-10-05 (user decision): a conversation with no project and no agent gets its
// own folder in Documents/MiniClaw, named after its title with a short id, the first time it runs a
// command or writes a file; until then nothing exists. It is reused, kept when the conversation is
// renamed, made again when the user removed it, and only it is open to the sandbox.
test('a conversation without a project gets its own folder at its first command, and only that folder', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const documents = app.peer.documents;
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/name`, { name: 'Say hi: a/b' }));
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  if (!start.params.dynamicTools.some(tool => tool.name === 'miniclaw_scoped_native')) return;
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).folder, null);
  await assert.rejects(readFile(join(documents, 'MiniClaw')), { code: 'ENOENT' }, 'nothing exists before it is needed');
  // Others' files beside it: another conversation's folder and the rest of Documents.
  await mkdir(join(documents, 'MiniClaw', 'Other 00000000'), { recursive: true });
  await writeFile(join(documents, 'MiniClaw', 'Other 00000000', 'other.txt'), 'other-content');
  await writeFile(join(documents, 'private.txt'), 'private-content');
  // Asked about without full access; the user approves.
  const first = await call(app, 'own-1', { action: 'run_shell', text: 'pwd; echo hi > hi.txt' }, 'approve_once');
  assert.equal(first.success, true, text(first));
  const folder = (await json(await app.request(`/api/threads/${THREAD}`))).folder;
  assert.ok(folder, 'the folder is recorded');
  assert.match(folder, /Say hi a b [0-9a-f]{8}$/);
  assert.equal(await realpath(join(documents, 'MiniClaw')), await realpath(join(folder, '..')));
  assert.equal(await readFile(join(folder, 'hi.txt'), 'utf8'), 'hi\n');
  // Full access: no popup; reads outside are denied by the sandbox.
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  const outside = await call(app, 'own-2', { action: 'run_shell', text: `cat ${shArg(join(documents, 'MiniClaw', 'Other 00000000', 'other.txt'))}; cat ${shArg(join(documents, 'private.txt'))}; echo again > again.txt` });
  assert.equal(outside.success, true, text(outside));
  assert.equal(text(outside).includes('other-content') || text(outside).includes('private-content'), false);
  assert.match(text(outside), DENIED);
  assert.equal(await readFile(join(folder, 'again.txt'), 'utf8'), 'again\n');
  // A rename keeps the folder; a removed folder is made again in the same place.
  await json(await post(app, `/api/threads/${THREAD}/name`, { name: 'Renamed' }));
  await rm(folder, { recursive: true, force: true });
  const again = await call(app, 'own-3', { action: 'run_shell', text: 'echo back > back.txt' });
  assert.equal(again.success, true, text(again));
  assert.equal(await readFile(join(folder, 'back.txt'), 'utf8'), 'back\n');
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).folder, folder);
});

// Deleting a conversation keeps its folder unless the user ticks the box; then the folder goes to the
// Trash, and only a real folder inside Documents/MiniClaw that this conversation owns.
test('deleting a conversation keeps its folder unless asked to move it to the Trash', async t => {
  const trash = await mkdtemp(join(tmpdir(), 'mc-trash-'));
  t.after(() => rm(trash, { recursive: true, force: true }));
  const app = await launchApp({}, { env: { MINICLAW_TRASH_DIR: trash } }); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  if (!start.params.dynamicTools.some(tool => tool.name === 'miniclaw_scoped_native')) return;
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  assert.equal((await call(app, 'del-1', { action: 'write_file', path: join(app.peer.documents, 'MiniClaw', 'x.txt'), text: 'x' })).success, false,
    'a write outside the conversation folder is not allowed under full access');
  const folder = (await json(await app.request(`/api/threads/${THREAD}`))).folder;
  assert.ok(folder);
  assert.equal((await call(app, 'del-2', { action: 'write_file', path: join(folder, 'keep.txt'), text: 'kept' })).success, true);
  await app.restart();
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`, { deleteFolder: true }));
  await assert.rejects(readFile(join(folder, 'keep.txt')), { code: 'ENOENT' });
  const { readdir } = await import('node:fs/promises');
  const moved = await readdir(trash);
  assert.equal(moved.length, 1);
  assert.equal(await readFile(join(trash, moved[0], 'keep.txt'), 'utf8'), 'kept');
});

// Review F-2: the conversation is deleted before its folder is moved, so a folder that could not be
// moved (here the Trash cannot be made; natively, a Trash on another volume whose copy failed) is
// kept, and the answer says so with its path; the conversation is gone either way.
test('a folder that could not be moved to the Trash is kept and named in the answer', async t => {
  const blocked = join(await mkdtemp(join(tmpdir(), 'mc-trash-')), 'not-a-folder');
  await writeFile(blocked, 'a file where the Trash should be');
  t.after(() => rm(join(blocked, '..'), { recursive: true, force: true }));
  const app = await launchApp({}, { env: { MINICLAW_TRASH_DIR: blocked } }); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  if (!start.params.dynamicTools.some(tool => tool.name === 'miniclaw_scoped_native')) return;
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  assert.equal((await call(app, 'kept-1', { action: 'write_file', path: join(app.peer.documents, 'MiniClaw', 'nowhere.txt'), text: 'x' })).success, false);
  const folder = (await json(await app.request(`/api/threads/${THREAD}`))).folder;
  assert.ok(folder);
  assert.equal((await call(app, 'kept-2', { action: 'write_file', path: join(folder, 'keep.txt'), text: 'kept' })).success, true);
  await app.restart();
  await app.set('thread/delete', { result: {} });
  const answer = await json(await post(app, `/api/threads/${THREAD}/delete`, { deleteFolder: true }));
  assert.deepEqual(answer, { folderKept: true, folder });
  assert.equal(await readFile(join(folder, 'keep.txt'), 'utf8'), 'kept');
  assert.equal((await app.request(`/api/threads/${THREAD}`)).status, 404, 'the conversation is deleted');
});

test('every tool call is recorded as activity for the user: commands with output, edits as diffs, refusals', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'replace_in_file', path: join(root, 'work/notes.txt'), find: 'work-', text: 'done-' })) return;
  const { events } = await import('./support/harness.mjs');
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  const file = join(root, 'work/notes.txt');
  const edited = await call(app, 'edit-a', { action: 'replace_in_file', path: file, find: 'work-', text: 'done-' });
  assert.equal(JSON.stringify(edited).includes('+done-content'), false, 'the model never receives the diff');
  await call(app, 'cmd-a', { action: 'run_shell', text: 'echo hello; exit 3', cwd: join(root, 'work') }, 'approve_once');
  await call(app, 'read-a', { action: 'read_file', path: join(root, 'secret/key.txt') }, 'reject');

  const live = stream.seen.filter(e => e.event === 'activity').map(e => e.data);
  assert.deepEqual(live.filter(a => a.id === 'edit-a').map(a => a.status), ['running', 'ok']);
  const { activities } = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(activities.map(a => [a.id, a.kind, a.status]),
    [['edit-a', 'edit', 'ok'], ['cmd-a', 'command', 'ok'], ['read-a', 'read', 'refused']]);
  const [edit, command, refused] = activities;
  assert.equal(edit.turnId, TURN);
  assert.equal(edit.title, file);
  assert.match(edit.diff, /^-work-content$/m);
  assert.match(edit.diff, /^\+done-content$/m);
  assert.equal(command.title, 'echo hello; exit 3');
  assert.equal(command.exitCode, 3);
  assert.match(command.output, /hello/);
  assert.equal(command.detail, join(root, 'work'));
  // A refusal says why (requirement 2026-10-05), and never shows the content.
  assert.equal(refused.output, 'The user declined this request, so nothing was run.');
  assert.equal(JSON.stringify(activities).includes('secret-content'), false);
  assert.ok(edit.endedAt >= edit.startedAt);

  await app.restart();
  const again = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(again.activities.map(a => a.id), ['edit-a', 'cmd-a', 'read-a'], 'activity survives a restart');
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
});

test('a Claude agent with recipients keeps its file tool and can delegate in the middle of a conversation', async t => {
  const { app, root, thread: plain } = await claudeAgentChat(t);
  // The fake model answers `tools` by calling the first tool it was given and naming them all.
  const listing = names => `${names[0]} error=false: ${names.join(',')}`;
  const first = await ask(app, plain, 'tools');
  // With the gate closed the file tool is missing and delegation still works.
  const file = await claudeGateClosed(root, first) ? [] : ['miniclaw_scoped_native'];
  assert.equal(first, listing([...file, 'miniclaw_ask']));
  const reviewer = (await json(await post(app, '/api/agents',
    { name: 'Reviewer', instructions: 'Review carefully.', active: true, model: MODEL }), 201)).agent;
  const lead = (await json(await post(app, '/api/agents', { name: 'Lead', instructions: 'Lead.', active: true,
    model: MODEL, delegatesTo: [reviewer.id] }), 201)).agent;
  const { thread } = await json(await post(app, '/api/threads',
    { environment: { kind: 'host' }, agentId: lead.id, model: MODEL }), 201);
  assert.equal(await ask(app, thread, 'tools'), listing([...file, 'miniclaw_delegate', 'miniclaw_ask']));
  // 2026-09-22: a plain Claude conversation may hand a task to any active agent too.
  const { thread: main } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  assert.match(await ask(app, main, 'tools'), /miniclaw_delegate/);

  // The user is asked, chooses to wait, and the Claude recipient's answer comes back named. A picture the
  // user attached goes with the task (2026-09-22).
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');
  const { attachment } = await json(await app.request(`/api/threads/${thread.id}/attachments?name=shot.png`,
    { method: 'POST', body: png, headers: { 'Content-Type': 'application/octet-stream', 'X-MiniClaw-Request': '1' } }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text:
    `delegate ${JSON.stringify({ agent: 'Reviewer', task: 'Say hello.', summary: 'Say hello', attachments: [attachment.id] })}`, clientMessageId: crypto.randomUUID(), model: MODEL,
    attachments: [attachment.id] }), 202);
  const asked = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => value.delegationPrompt, 'delegation prompt from a Claude conversation', 10000);
  assert.equal(asked.delegationPrompt.agent, 'Reviewer');
  assert.deepEqual(asked.delegationPrompt.attachments.map(item => item.name), ['shot.png']);
  await json(await post(app, `/api/threads/${thread.id}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'parent reply', 15000);
  assert.match(done.messages.at(-1).text, /miniclaw_delegate error=false: Answer from the agent Reviewer:/);
  assert.match(done.messages.at(-1).text, /Hello from Claude/);
  assert.deepEqual(done.delegations.map(run => [run.agent, run.mode, run.status, run.result]),
    [['Reviewer', 'wait', 'done', 'Hello from Claude']]);

  // 2026-09-23: in the background, the idle Claude parent carries on by itself with the result.
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text:
    `delegate ${JSON.stringify({ agent: 'Reviewer', task: 'Say hello again.', summary: 'Say hello again' })}`, clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const again = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => value.delegationPrompt, 'second delegation prompt', 10000);
  await json(await post(app, `/api/threads/${thread.id}/delegations/${again.delegationPrompt.id}`, { mode: 'background' }));
  const carried = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.some(m => m.role === 'user' && m.text.startsWith('[MiniClaw notice] Reviewer finished "Say hello again"'))
      && value.messages.at(-1).role === 'assistant', 'the Claude parent carries on', 20000);
  assert.equal(carried.delegations.at(-1).status, 'done');
  const runs = (await readFile(join(root, 'calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line)).filter(call => call.prompt);
  const notice = runs.find(call => call.prompt.includes('[MiniClaw notice] Reviewer finished'));
  assert.match(notice.prompt, /A background delegation has ENDED[\s\S]*Hello from Claude/);
});

// Switches the coding session on for the fixture conversation.
const codingOn = app => post(app, `/api/permissions/conversation/${THREAD}`,
  { target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow' });
// Waits for the open review of a proposed change.
const review = app => eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
  value => value.review, 'review popup');
const decide = (app, id, body) => post(app, `/api/threads/${THREAD}/reviews/${id}`, body);
// Sends a tool call without waiting for its reply.
const propose = (app, id, args) => app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
  threadId: THREAD, turnId: TURN, callId: id, arguments: args } });
const answerOf = async (app, id) => (await eventually(() => app.inspect(),
  value => value.responses.some(reply => reply.id === id), `tool reply ${id}`)).responses.find(reply => reply.id === id).result;

test('in a coding session every edit waits for review with its diff, and nothing is written before Apply', async t => {
  const { app, root } = await agentChat(t);
  const file = join(root, 'work/notes.txt');
  await json(await codingOn(app));
  if (await gateClosed(app, { action: 'replace_in_file', path: file, find: 'work-', text: 'reviewed-' })) return;
  await propose(app, 'e-1', { action: 'replace_in_file', path: file, find: 'work-', text: 'reviewed-' });
  const open = (await review(app)).review;
  assert.equal(open.path, file);
  assert.equal(open.kind, 'edit');
  assert.match(open.diff, /^-work-content$/m);
  assert.match(open.diff, /^\+reviewed-content$/m);
  assert.equal(open.proposed, 'reviewed-content');
  assert.equal(await readFile(file, 'utf8'), 'work-content', 'nothing is written while the review is open');
  assert.equal((await decide(app, open.id, { action: 'nonsense' })).status, 400);
  await json(await decide(app, open.id, { action: 'apply' }));
  assert.equal((await answerOf(app, 'e-1')).success, true);
  assert.equal(await readFile(file, 'utf8'), 'reviewed-content');
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).review ?? null, null);
});

test('a rejected change writes nothing and the note reaches the agent; an edited change is what gets written', async t => {
  const { app, root } = await agentChat(t);
  const file = join(root, 'work/notes.txt');
  await json(await codingOn(app));
  if (await gateClosed(app, { action: 'write_file', path: file, text: 'agent version' })) return;
  await propose(app, 'w-1', { action: 'write_file', path: file, text: 'agent version' });
  await json(await decide(app, (await review(app)).review.id, { action: 'reject', note: 'Keep the original wording.' }));
  const rejected = await answerOf(app, 'w-1');
  assert.equal(rejected.success, false);
  assert.match(text(rejected), /rejected/i);
  assert.match(text(rejected), /Keep the original wording\./);
  assert.equal(await readFile(file, 'utf8'), 'work-content');

  await propose(app, 'w-2', { action: 'write_file', path: file, text: 'agent version' });
  await json(await decide(app, (await review(app)).review.id, { action: 'apply', text: 'my own version' }));
  const edited = await answerOf(app, 'w-2');
  assert.equal(edited.success, true);
  assert.match(text(edited), /changed your proposed content/i);
  assert.equal(await readFile(file, 'utf8'), 'my own version');
  const { activities } = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(activities.map(entry => [entry.id, entry.status]), [['w-1', 'refused'], ['w-2', 'ok']]);
  assert.match(activities[1].diff, /^\+my own version$/m, 'the recorded diff is what was really written');
});

test('"apply all" covers the rest of the turn only, and a coding session asks about every command', async t => {
  const { app, root } = await agentChat(t);
  const file = join(root, 'work/notes.txt');
  await json(await codingOn(app));
  if (await gateClosed(app, { action: 'run_shell', text: 'cat notes.txt', cwd: join(root, 'work') })) return;
  // A saved rule would allow this command; the coding session asks anyway.
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { effect: 'allow',
    target: { target_type: 'command', kind: 'shell', text: 'cat notes.txt', cwd: join(root, 'work') } }));
  const ran = await call(app, 'c-1', { action: 'run_shell', text: 'cat notes.txt', cwd: join(root, 'work') }, 'approve_once');
  assert.match(text(ran), /work-content/);

  await propose(app, 'a-1', { action: 'write_file', path: file, text: 'one' });
  await json(await decide(app, (await review(app)).review.id, { action: 'apply_all' }));
  assert.equal((await answerOf(app, 'a-1')).success, true);
  const second = await call(app, 'a-2', { action: 'write_file', path: file, text: 'two' });
  assert.equal(second.success, true, 'the rest of the turn is applied without asking');
  assert.equal(await readFile(file, 'utf8'), 'two');
});

test('without a coding session an edit is applied as before, with no review', async t => {
  const { app, root } = await agentChat(t);
  if (await gateClosed(app, { action: 'write_file', path: join(root, 'work/notes.txt'), text: 'plain' })) return;
  const done = await call(app, 'n-1', { action: 'write_file', path: join(root, 'work/notes.txt'), text: 'plain' });
  assert.equal(done.success, true);
  assert.equal(await readFile(join(root, 'work/notes.txt'), 'utf8'), 'plain');
});

test('the snapshot says whether the conversation is a coding session, and a project can make it the default', async t => {
  const { app } = await agentChat(t);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).codingSession, false);
  await json(await codingOn(app));
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).codingSession, true);
  await json(await post(app, `/api/permissions/conversation/${THREAD}`,
    { target: { target_type: 'feature', name: 'coding_session' }, effect: 'block' }));
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).codingSession, false);
});

test('in a coding session a new file is reviewed before it exists, and a file that cannot be shown is never overwritten', async t => {
  const { app, root } = await agentChat(t);
  await json(await codingOn(app));
  if (await gateClosed(app, { action: 'write_file', path: join(root, 'work/fresh.txt'), text: 'brand new\n' })) return;
  const fresh = join(root, 'work/fresh.txt');
  await propose(app, 'n-1', { action: 'write_file', path: fresh, text: 'brand new\n' });
  const open = (await review(app)).review;
  assert.equal(open.kind, 'create');
  assert.match(open.diff, /^\+brand new$/m);
  await assert.rejects(readFile(fresh), { code: 'ENOENT' }, 'nothing exists while the review is open');
  await json(await decide(app, open.id, { action: 'apply' }));
  assert.match(text(await answerOf(app, 'n-1')), /created/i);
  assert.equal(await readFile(fresh, 'utf8'), 'brand new\n');

  // Bytes that are not text cannot be shown as a diff, so they are not replaced unseen.
  const binary = join(root, 'work/image.bin');
  await writeFile(binary, Buffer.from([0xff, 0xfe, 0x00, 0x80]));
  await propose(app, 'b-1', { action: 'write_file', path: binary, text: 'overwritten' });
  const refused = await answerOf(app, 'b-1');
  assert.equal(refused.success, false);
  assert.match(text(refused), /cannot be shown/i);
  assert.deepEqual([...await readFile(binary)], [0xff, 0xfe, 0x00, 0x80]);
});

test('the model is told that its own sandbox is read-only on purpose and that the tool is how it writes', async t => {
  const { app, root } = await agentChat(t);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  if (await gateClosed(app, { action: 'write_file', path: join(root, 'work/notes.txt'), text: 'x' })) {
    // Without a sandboxed tool the model is told it has none, and nothing about writing through it.
    assert.doesNotMatch(start.params.baseInstructions, /read-only on purpose|write_file creates/);
    return assert.match(start.params.baseInstructions, /no file or command tool/);
  }
  assert.match(start.params.baseInstructions, /read-only on purpose/);
  assert.match(start.params.baseInstructions, /write_file creates/);
});

// Harmless commands: a fixed read-only list runs without a popup; anything else still asks.
test('a read-only command runs without asking, anything with a write, a redirect or an unknown program still asks', async t => {
  const { app, root } = await agentChat(t);
  const cwd = join(root, 'work');
  if (await gateClosed(app, { action: 'run_shell', text: 'ls -la', cwd })) return;
  const quiet = async (id, text) => {
    await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: id,
      arguments: { action: 'run_shell', text, cwd } } });
    const state = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === id), `tool reply ${id}`, windows ? 15000 : 4000);
    assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, `${text} asked`);
    return state.responses.find(reply => reply.id === id).result;
  };
  assert.match(text(await quiet('q-1', 'ls -la')), /notes\.txt/);
  assert.match(text(await quiet('q-2', 'cat notes.txt | head -n 1')), /work-content/);
  assert.match(text(await quiet('q-3', 'git status --porcelain; pwd')), /work/);
  // The list is what runs, not the word: a write, a redirect, an unknown program or a substitution asks.
  for (const text of ['rm notes.txt', 'echo hi > out.txt', 'python3 -c 1', 'cat $(echo notes.txt)', 'ls; touch made', 'cat notes.txt || rm -rf .', 'sudo ls', 'ls `pwd`', 'git push']) {
    await app.emit({ id: `ask-${text.length}-${text[0]}`, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN,
      callId: `ask-${text.length}-${text[0]}`, arguments: { action: 'run_shell', text, cwd } } });
    const asked = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, `popup for ${text}`);
    assert.equal(asked.scopedApproval.target.text, text);
    await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${asked.scopedApproval.id}`, { action: 'reject' }));
    await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === `ask-${text.length}-${text[0]}`), 'refusal');
  }
  // The conversation can turn the quiet path off: "Ask about every command".
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'ask_commands', on: true }));
  await app.emit({ id: 'ask-all', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'ask-all',
    arguments: { action: 'run_shell', text: 'ls', cwd } } });
  const asked = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'popup with the switch on');
  assert.equal(asked.scopedApproval.target.text, 'ls');
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${asked.scopedApproval.id}`, { action: 'approve_once' }));
});

test('a Claude read-only command runs without asking too', async t => {
  const { app, root, thread } = await claudeAgentChat(t);
  const answer = await ask(app, thread, `tool ${JSON.stringify({ action: 'run_shell', text: 'cat notes.txt', cwd: join(root, 'work') })}`);
  if (await claudeGateClosed(root, answer)) return;
  assert.match(answer, /error=false/);
  assert.match(answer, /work-content/);
});

// Full access: for one conversation nothing the agent may do is asked about. A block still blocks,
// and a path or host no rule names is refused, not asked.
// Requirement 2026-09-22: with full access and a coding session both on, only file changes ask,
// each with its diff; commands, reads and the rest run as full access says.
test('full access with a coding session: commands run without asking, and a file change still waits for its review', async t => {
  const { app, root } = await agentChat(t);
  const cwd = join(root, 'work');
  if (await gateClosed(app, { action: 'run_shell', text: 'cat notes.txt', cwd })) return;
  for (const name of ['coding_session', 'full_access']) {
    await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: { target_type: 'feature', name }, effect: 'allow' }));
  }
  await app.emit({ id: 'both-1', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'both-1',
    arguments: { action: 'run_shell', text: 'grep -n "a\\|b" notes.txt | head', cwd } } });
  const state = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === 'both-1'), 'tool reply both-1');
  assert.equal(state.responses.find(reply => reply.id === 'both-1').result.success, true);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'no popup');
  await app.emit({ id: 'both-2', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'both-2',
    arguments: { action: 'replace_in_file', path: join(cwd, 'notes.txt'), find: 'work-', text: 'reviewed-' } } });
  const open = (await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.review, 'review popup')).review;
  assert.match(open.diff, /^\+reviewed-content$/m);
  assert.equal(await readFile(join(cwd, 'notes.txt'), 'utf8'), 'work-content', 'nothing is written while the review is open');
  await json(await post(app, `/api/threads/${THREAD}/reviews/${open.id}`, { action: 'apply' }));
  const written = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === 'both-2'), 'tool reply both-2');
  assert.equal(written.responses.find(reply => reply.id === 'both-2').result.success, true);
  assert.equal(await readFile(join(cwd, 'notes.txt'), 'utf8'), 'reviewed-content');
});

test('full access runs allowed commands and writes without asking, refuses what no rule reaches, and shows in the environment note', async t => {
  const { app, root } = await agentChat(t);
  const cwd = join(root, 'work');
  if (await gateClosed(app, { action: 'run_shell', text: 'cat notes.txt', cwd })) return;
  const run = async (id, args) => {
    await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: id, arguments: args } });
    const state = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === id), `tool reply ${id}`, windows ? 15000 : 4000);
    return state.responses.find(reply => reply.id === id).result;
  };
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  const wrote = await run('fa-1', { action: 'run_shell', text: 'echo autonomous > made.txt; cat made.txt', cwd });
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'no popup');
  assert.match(text(wrote), /autonomous/);
  const edited = await run('fa-2', { action: 'write_file', path: join(cwd, 'notes.txt'), text: 'rewritten' });
  assert.equal(edited.success, true);
  assert.equal(await readFile(join(cwd, 'notes.txt'), 'utf8'), 'rewritten');
  // No rule names the secret folder: refused outright, not asked.
  const refused = await run('fa-3', { action: 'read_file', path: join(root, 'secret/key.txt') });
  assert.equal(refused.success, false);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
  // A conversation block inside the allowed folder still wins.
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: { target_type: 'file', path: join(cwd, 'notes.txt'), action: 'read' }, effect: 'block' }));
  assert.equal((await run('fa-4', { action: 'read_file', path: join(cwd, 'notes.txt') })).success, false);
  // The model is told, and the switch reads as on. The fixture's first turn has to end first.
  await app.emit(turnDone());
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  const turn = { id: 'turn-fa', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Next', clientMessageId: crypto.randomUUID() }), 202);
  const starts = (await app.inspect()).requests.filter(request => request.method === 'turn/start');
  assert.match(starts.at(-1).params.additionalContext.miniclaw_environment.value, /Full access[^\n]*: on/);
  const listed = (await json(await app.request(`/api/capabilities?thread=${THREAD}`))).capabilities.find(item => item.key === 'full_access');
  assert.deepEqual([listed.name, listed.on], ['Full access', true]);
});

// Allowing a program: the popup offers to allow the command's programs for good, as a lasting
// rule on the agent, and any plain command line of allowed programs then runs without asking.
test('"Always allow" on a command\'s popup saves a program rule, and plain lines of allowed programs stop asking', async t => {
  const { app, root } = await agentChat(t);
  const cwd = join(root, 'work');
  if (await gateClosed(app, { action: 'run_shell', text: 'sed -n 1p notes.txt', cwd })) return;
  const run = (id, text, decide) => call(app, id, { action: 'run_shell', text, cwd }, decide);
  await app.emit({ id: 'pr-1', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'pr-1',
    arguments: { action: 'run_shell', text: 'sed -n 1p notes.txt; awk 1 notes.txt', cwd } } });
  const asked = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'popup');
  assert.deepEqual(asked.scopedApproval.programs, ['sed', 'awk']);
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${asked.scopedApproval.id}`, { action: 'approve_programs' }));
  const first = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === 'pr-1'), 'reply');
  assert.match(text(first.responses.find(reply => reply.id === 'pr-1').result), /work-content/);
  // The rule is the agent's, lasting and visible.
  const agent = (await json(await app.request('/api/agents'))).agents[0];
  const rules = (await json(await app.request(`/api/permissions/agent/${agent.id}`))).rules.filter(rule => rule.target.target_type === 'program');
  assert.deepEqual(rules.map(rule => [rule.target.name, rule.effect]).sort(), [['awk', 'allow'], ['sed', 'allow']]);
  // A different plain line of those programs runs unasked; one that adds an unknown program asks.
  assert.match(text(await run('pr-2', 'sed -n 1p notes.txt | awk 1', null)), /work-content/);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null);
  await app.emit({ id: 'pr-3', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'pr-3',
    arguments: { action: 'run_shell', text: 'sed -n 1p notes.txt; python3 -c 1', cwd } } });
  const again = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'popup again');
  assert.deepEqual(again.scopedApproval.programs, ['sed', 'python3']);
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${again.scopedApproval.id}`, { action: 'reject' }));
  // A redirect is not a plain line, so it asks even for an allowed program.
  await app.emit({ id: 'pr-4', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: 'pr-4',
    arguments: { action: 'run_shell', text: 'sed -n 1p notes.txt > copy.txt', cwd } } });
  const redirect = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'popup for a redirect');
  assert.equal(redirect.scopedApproval.programs, undefined, 'no program button for a line the shell would do more with');
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${redirect.scopedApproval.id}`, { action: 'reject' }));
  // Removing the rule brings the popup back.
  await json(await post(app, `/api/permissions/agent/${agent.id}`, { target: { target_type: 'program', name: 'sed' }, effect: 'remove' }));
  // A removed rule interrupts the running turn; the fake answers no interrupt, so the test ends it.
  await app.emit(turnDone('interrupted'));
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'the turn ended by the removal');
  const turn = { id: 'turn-after-removal', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: crypto.randomUUID() }), 202);
  await app.emit({ id: 'pr-5', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: turn.id, callId: 'pr-5',
    arguments: { action: 'run_shell', text: 'sed -n 1p notes.txt', cwd } } });
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'popup after removal');
});

test('a Claude command of allowed programs runs without asking too', async t => {
  const { app, root, thread } = await claudeAgentChat(t);
  const agent = (await json(await app.request('/api/agents'))).agents[0];
  await json(await post(app, `/api/permissions/agent/${agent.id}`, { target: { target_type: 'program', name: 'sed' }, effect: 'allow' }));
  const answer = await ask(app, thread, `tool ${JSON.stringify({ action: 'run_shell', text: 'sed -n 1p notes.txt', cwd: join(root, 'work') })}`);
  if (await claudeGateClosed(root, answer)) return;
  assert.match(answer, /error=false/);
  assert.match(answer, /work-content/);
});

// Grants are explicit, bounded in time, and never restored after restart.
test('unsandboxed shell requires a timed grant', async t => {
  const { app, root, agent } = await agentChat(t);
  const toggle = body => post(app, `/api/threads/${THREAD}/capabilities`,
    { key: 'unsandboxed_shell', ...body });
  const state = async () => (await json(await app.request(`/api/capabilities?thread=${THREAD}`)))
    .capabilities.find(row => row.key === 'unsandboxed_shell');
  assert.equal((await state()).on, false);
  await json(await toggle({ on: true, durationSeconds: 60 }), 403);
  await json(await post(app, `/api/permissions/agent/${agent.id}`, {
    target: { target_type: 'feature', name: 'unsandboxed_shell' }, effect: 'allow' }));
  for (const durationSeconds of [undefined, 0, -1, 86401]) {
    await json(await toggle({ on: true, durationSeconds }), 400);
  }
  await json(await toggle({ on: true, durationSeconds: 60 }));
  assert.equal((await state()).on, true);
  assert.ok((await state()).expiresAt > Date.now());
  const result = await call(app, 'unrestricted-read', { action: 'run_shell',
    cwd: join(root, 'work'), text: windows ? `Get-Content -LiteralPath ${ps(join(root, 'secret/key.txt'))}` : `cat '${join(root, 'secret/key.txt')}'` }, 'approve_once');
  assert.match(text(result), /secret-content/);
  await app.restart();
  assert.equal((await state()).on, false);
});

// Expiry kills a running command and needs a fresh explicit grant.
test('unsandboxed expiry stops commands until renewed', async t => {
  const { app, root, agent } = await agentChat(t);
  await json(await post(app, `/api/permissions/agent/${agent.id}`, {
    target: { target_type: 'feature', name: 'unsandboxed_shell' }, effect: 'allow' }));
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  const grant = () => post(app, `/api/threads/${THREAD}/capabilities`,
    { key: 'unsandboxed_shell', on: true, durationSeconds: windows ? 5 : 2 });
  await json(await grant());
  const escaped = join(root, 'secret/escaped');
  const childStarted = join(root, 'secret/child-started');
  const childCode = `[IO.File]::WriteAllText(${ps(childStarted)}, [string]$PID); Start-Sleep -Seconds 7; [IO.File]::WriteAllText(${ps(escaped)}, 'escaped')`;
  const encoded = Buffer.from(childCode, 'utf16le').toString('base64');
  const shell = join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
  const descendant = `$child=Start-Process -FilePath ${ps(shell)} -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand','${encoded}') -NoNewWindow -PassThru; $child.WaitForExit()`;
  const result = await call(app, 'expires-running', { action: 'run_shell', cwd: join(root, 'work'),
    text: windows ? descendant : `sleep 4; echo escaped > '${escaped}'` });
  assert.equal(result.success, false);
  assert.match(text(result), /expired|revoked/);
  const state = (await json(await app.request(`/api/capabilities?thread=${THREAD}`)))
    .capabilities.find(row => row.key === 'unsandboxed_shell');
  assert.equal(state.on, false);
  assert.equal(state.expired, true);
  if (windows) {
    const savedPid = await readFile(childStarted, 'utf8');
    assert.match(savedPid, /^[1-9]\d*$/, 'the descendant wrote outside grants before expiry');
    const pid = Number(savedPid);
    assert.ok(Number.isSafeInteger(pid));
    await eventually(() => {
      try { process.kill(pid, 0); return false; }
      catch (error) { assert.equal(error.code, 'ESRCH'); return true; }
    }, Boolean, 'expired descendant terminated before its sleep completes', 1000);
  }
  await new Promise(resolve => setTimeout(resolve, windows ? 7500 : 2300));
  await assert.rejects(readFile(escaped), { code: 'ENOENT' });
  await json(await grant());
  const renewed = await call(app, 'renewed', { action: 'run_shell', cwd: join(root, 'work'),
    text: windows ? `Get-Content -LiteralPath ${ps(join(root, 'secret/key.txt'))}` : `cat '${join(root, 'secret/key.txt')}'` });
  assert.match(text(renewed), /secret-content/);
});

// Revoking either level also interrupts commands that are already executing.
test('unsandboxed revocation interrupts a command', async t => {
  const { app, root, agent } = await agentChat(t);
  const target = { target_type: 'feature', name: 'unsandboxed_shell' };
  await json(await post(app, `/api/permissions/agent/${agent.id}`, { target, effect: 'allow' }));
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'full_access', on: true }));
  for (const mode of ['conversation', 'agent']) {
    await json(await post(app, `/api/threads/${THREAD}/capabilities`,
      { key: 'unsandboxed_shell', on: true, durationSeconds: 60 }));
    const marker = join(root, 'work', `started-${mode}`);
    const running = call(app, `revoke-${mode}`, { action: 'run_shell', cwd: join(root, 'work'),
      text: windows ? `[IO.File]::WriteAllText(${ps(marker)}, 'started'); Start-Sleep -Seconds 10` : `echo started > '${marker}'; sleep 10` });
    await eventually(() => readFile(marker, 'utf8').catch(() => ''), value => value.includes('started'), 'command started');
    if (mode === 'conversation') await json(await post(app, `/api/threads/${THREAD}/capabilities`,
      { key: 'unsandboxed_shell', on: false }));
    else await json(await post(app, `/api/permissions/agent/${agent.id}`, { target, effect: 'remove' }));
    assert.match(text(await running), /expired|revoked/);
    if (mode === 'agent') {
      await json(await post(app, `/api/permissions/agent/${agent.id}`, { target, effect: 'allow' }));
      const rows = (await json(await app.request(`/api/capabilities?thread=${THREAD}`))).capabilities;
      assert.equal(rows.find(row => row.key === 'unsandboxed_shell').on, false);
    }
  }
});

// A late approval cannot restore a grant that expired while the popup was open.
test('expired approval cannot start an unrestricted command', async t => {
  const { app, root, agent } = await agentChat(t);
  await json(await post(app, `/api/permissions/agent/${agent.id}`, {
    target: { target_type: 'feature', name: 'unsandboxed_shell' }, effect: 'allow' }));
  await json(await post(app, `/api/threads/${THREAD}/capabilities`,
    { key: 'unsandboxed_shell', on: true, durationSeconds: 1 }));
  const running = call(app, 'late-approval', { action: 'run_shell', cwd: join(root, 'work'),
    text: windows ? `[IO.File]::WriteAllText(${ps(join(root, 'secret/late'))}, 'escaped')` : `echo escaped > '${join(root, 'secret/late')}'` });
  const snapshot = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
    value => value.scopedApproval, 'unrestricted approval');
  assert.equal(snapshot.scopedApproval.unrestricted, true);
  await new Promise(resolve => setTimeout(resolve, 1100));
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${snapshot.scopedApproval.id}`,
    { action: 'approve_once' }));
  assert.match(text(await running), /expired|revoked/);
  await assert.rejects(readFile(join(root, 'secret/late')), { code: 'ENOENT' });
});

// The main agent needs its own allowance and a live timed grant, like named agents.
test('main agent can use a timed shell grant', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const MAIN = '00000000-0000-0000-0000-000000000000';
  await json(await post(app, `/api/permissions/agent/${MAIN}`, {
    target: { target_type: 'feature', name: 'unsandboxed_shell' }, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const row = (await json(await app.request(`/api/capabilities?thread=${THREAD}`)))
    .capabilities.find(row => row.key === 'unsandboxed_shell');
  assert.equal(row.allowed, true);
  assert.equal(row.on, false);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  assert.ok(start.params.dynamicTools.some(tool => tool.name === 'miniclaw_scoped_native'));
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  const args = { action: 'run_shell', cwd: process.platform === 'darwin' ? '/private/tmp' : tmpdir(),
    // A redirect keeps the line from running unasked in the sandbox, so the popup decides it.
    text: windows ? "Write-Output 'main-timed-shell'" : 'printf main-timed-shell 2>/dev/null' };
  // Requirement change 2026-10-05 (user decision): a conversation with no project and no agent runs
  // sandboxed commands too. Without the grant this one is a sandboxed command: asked about on the Mac
  // (and declined here), refused on Windows because its folder is outside what the sandbox may enter.
  // Neither runs it outside the sandbox.
  const before = await call(app, 'main-before', args, windows ? undefined : 'reject');
  assert.equal(before.success, false);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`,
    { key: 'unsandboxed_shell', on: true, durationSeconds: 60 }));
  const active = (await json(await app.request(`/api/capabilities?thread=${THREAD}`)))
    .capabilities.find(row => row.key === 'unsandboxed_shell');
  assert.equal(active.on, true);
  assert.match(text(await call(app, 'main-during', args, 'approve_once')), /main-timed-shell/);
  assert.equal((await call(app, 'main-file', { action: 'read_file', path: windows ? join(process.env.SystemRoot, 'System32/drivers/etc/hosts') : '/etc/hosts' }, 'reject')).success, false);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'unsandboxed_shell', on: false }));
  const revoked = (await json(await app.request(`/api/capabilities?thread=${THREAD}`)))
    .capabilities.find(row => row.key === 'unsandboxed_shell');
  assert.equal(revoked.allowed, true);
  assert.equal(revoked.on, false);
  assert.equal((await call(app, 'main-after', args, windows ? undefined : 'reject')).success, false);
});
