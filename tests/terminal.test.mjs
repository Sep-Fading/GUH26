// Checks the shared terminal of a conversation: it opens in the working folder, streams what the
// shell prints, takes what the user types, is confined like the agent's commands, restarts, and
// goes with the conversation.
import assert from 'node:assert/strict';
// Registers the terminal tests.
import { test } from 'node:test';
// Prepares a working folder and a file outside it.
import { mkdtemp, realpath, rm, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts the backend against the fake Codex and reads its event streams.
import { launchApp, json, events, eventually } from './support/harness.mjs';
import { TERMINAL_UNAVAILABLE, closedRefusal } from './support/sandbox.mjs';
// Names the fake conversation.
import { THREAD } from './support/fixtures.mjs';

const windows = process.platform === 'win32';
const prompt = windows ? /sh> / : /%/;
// Quotes literal POSIX paths without expanding shell input.
const sh = value => `'${value.replaceAll('\\', '/').replaceAll("'", "'\\''")}'`;
const shellPath = value => windows ? value.replaceAll('\\', '/') : value;
// Matches native paths without treating separators as regular-expression escapes.
const escaped = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
// While Windows cannot enforce the sandbox, the terminal does not open: the backend answers
// 409 terminal_unavailable, and an agent's shell tool is not offered. Only those answers select the
// Windows expectations below, so every test checks the real terminal once the gate opens.
class TerminalGateClosed extends Error {}
const terminalTest = (name, body) => test(name, async t => {
  try { await body(t); } catch (error) { if (!(error instanceof TerminalGateClosed)) throw error; }
});

/** A project with a folder, one conversation in it, and a file outside the folder. */
async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-terminal-')));
  const site = join(root, 'site');
  await (await import('node:fs/promises')).mkdir(site);
  await writeFile(join(site, 'notes.txt'), 'inside the folder\n');
  await writeFile(join(root, 'outside.txt'), 'OUTSIDE-TEXT\n');
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { project } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
  return { app, site, root };
}

/** Opens the terminal and its stream; `screen()` is everything the shell printed so far. */
async function attach(app, thread = THREAD, after = 0) {
  const response = await post(app, `/api/threads/${thread}/terminal/open`, { cols: 100, rows: 30 });
  if (windows && response.status === 409) {
    const { error } = await response.clone().json();
    if (error.code === 'terminal_unavailable') {
      assert.equal(error.message, await closedRefusal(app, TERMINAL_UNAVAILABLE));
      throw new TerminalGateClosed();
    }
  }
  const opened = await json(response);
  const feed = await events(app, `/api/threads/${thread}/terminal/stream?after=${after}`);
  const screen = () => feed.seen.filter(event => event.event === 'output').map(event => Buffer.from(event.data.bytes, 'base64').toString()).join('');
  const shows = (pattern, what = String(pattern)) => eventually(screen, text => pattern.test(text), what, 15000);
  const type = async text => json(await post(app, `/api/threads/${thread}/terminal/input`, { text }));
  return { opened, feed, screen, shows, type };
}

terminalTest('the terminal opens in the working folder, streams the prompt, and runs what the user types', async t => {
  const { app, site } = await fixture(t);
  const term = await attach(app);
  assert.deepEqual([term.opened.cols, term.opened.rows, term.opened.status], [100, 30, 'live']);
  await term.shows(prompt, 'a prompt');
  await term.type('pwd; cat notes.txt\r');
  await term.shows(/inside the folder/);
  assert.match(term.screen(), new RegExp(escaped(shellPath(site))), 'pwd is the working folder');
  if (windows) {
    await term.type('mkdir -p src; cd src; pwd\r');
    await term.shows(new RegExp(escaped(shellPath(join(site, 'src')))));
    const system32 = join(process.env.SystemRoot, 'System32');
    // PATH keeps the Windows spelling of its folders (backslashes, any letter case), so it is compared folded.
    await term.type(`case ";$(printf %s "$PATH" | tr 'A-Z\\\\' 'a-z/');" in *";${shellPath(system32).toLowerCase()};"*) echo PATH_SYSTEM=True;; esac; [ "$HOME" != ${sh(process.env.USERPROFILE)} ] && echo HOME_PRIVATE=True; [ "$USERPROFILE" != ${sh(process.env.USERPROFILE)} ] && echo PROFILE_PRIVATE=True\r`);
    await term.shows(/\nPATH_SYSTEM=True\r?\n/);
    await term.shows(/\nHOME_PRIVATE=True\r?\n/);
    await term.shows(/\nPROFILE_PRIVATE=True\r?\n/);
  } else {
    // The zsh prompt names the folder relative to the working folder.
    await term.type('mkdir -p src; cd src\r');
    await term.shows(/site\/src\x1b\[39m %/);
    await term.type('echo "path=$PATH home=$HOME"\r');
    await term.shows(/path=\/usr\/bin:\/bin:\/usr\/sbin:\/sbin/);
    assert.doesNotMatch(term.screen(), new RegExp(`home=${escaped(process.env.HOME)}`));
  }
  await term.feed.close();
});

terminalTest('the terminal is confined like the agent\'s commands', async t => {
  const { app, root, site } = await fixture(t);
  const term = await attach(app);
  await term.shows(prompt, 'a prompt');
  if (windows) {
    for (const [path, marker] of [[join(root, 'outside.txt'), 'OUTSIDE_DENIED'],
      [join(app.peer.dir, 'secrets.json'), 'SECRET_DENIED']]) {
      await term.type(`if cat ${sh(path)}; then echo OUTSIDE_LEAK; else echo ${marker}; fi\r`);
      await term.shows(new RegExp(`\\n${marker}\\r?\\n`));
    }
    assert.doesNotMatch(term.screen(), /OUTSIDE-TEXT/);
    await term.type(`if printf tampered > ${sh(join(root, 'outside.txt'))}; then echo WRITE_ALLOWED; else echo WRITE_DENIED; fi\r`);
    await term.shows(/\nWRITE_DENIED\r?\n/);
    assert.equal(await readFile(join(root, 'outside.txt'), 'utf8'), 'OUTSIDE-TEXT\n');
    await term.type(`printf '%s' 'inside write' > ${sh(join(site, 'inside-write.txt'))}; cat ${sh(join(site, 'inside-write.txt'))}; echo\r`);
    await term.shows(/\ninside write\r?\n/);
    assert.equal(await readFile(join(site, 'inside-write.txt'), 'utf8'), 'inside write');
  } else {
    await term.type(`cat ${join(root, 'outside.txt')}; echo done1\r`);
    await term.shows(/done1/);
    assert.match(term.screen(), /Operation not permitted/);
    assert.doesNotMatch(term.screen(), /OUTSIDE-TEXT/);
    await term.type(`cat ${app.peer.dir}/secrets.json; echo done2\r`);
    await term.shows(/done2/);
    assert.doesNotMatch(term.screen(), /"/, 'nothing of the private file');
  }
  await term.type('echo made > made.txt; cat made.txt\r');
  await term.shows(/\nmade\r?\n/);
  await term.feed.close();
});

terminalTest('a late page gets the screen so far, a restart clears it, and the shell goes with the conversation', async t => {
  const { app } = await fixture(t);
  const first = await attach(app);
  await first.shows(prompt, 'a prompt');
  await first.type('echo MARK-ONE\r');
  await first.shows(/MARK-ONE\r?\n/);
  await first.feed.close();
  const late = await attach(app);
  await late.shows(/MARK-ONE/);
  const seq = late.feed.seen.filter(event => event.event === 'output').at(-1).id;
  assert.ok(Number(seq) > 0, 'events carry the ring offset as their id');
  await late.feed.close();
  // Reading after the last offset gives only what comes next.
  const caught = await attach(app, THREAD, Number(seq));
  await caught.type('echo MARK-TWO\r');
  await caught.shows(/MARK-TWO\r?\n/);
  assert.doesNotMatch(caught.screen(), /MARK-ONE\r?\n/);
  await caught.feed.close();
  // `exit` ends the shell; the page is told, and the next open starts a fresh one.
  const ending = await attach(app);
  await ending.type('exit\r');
  await ending.feed.next('exit');
  await ending.feed.close();
  const fresh = await attach(app);
  assert.equal(fresh.opened.status, 'live');
  await fresh.shows(prompt, 'a prompt');
  assert.doesNotMatch(fresh.screen(), /MARK-TWO/);
  await json(await post(app, `/api/threads/${THREAD}/terminal/restart`));
  const after = await attach(app);
  await after.shows(prompt, 'a prompt');
  await after.feed.close();
  await fresh.feed.close();
  for (const [text, status] of [['x'.repeat(4097), 400], ['', 400]]) assert.equal((await post(app, `/api/threads/${THREAD}/terminal/input`, { text })).status, status);
  assert.equal((await post(app, `/api/threads/${THREAD}/terminal/resize`, { cols: 10, rows: 3 })).status, 400);
  await json(await post(app, `/api/threads/${THREAD}/terminal/resize`, { cols: 120, rows: 40 }));
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  assert.equal((await post(app, `/api/threads/${THREAD}/terminal/open`, { cols: 80, rows: 24 })).status, 404);
});

test('a conversation without a working folder has no terminal', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const refused = await post(app, `/api/threads/${THREAD}/terminal/open`, { cols: 80, rows: 24 });
  assert.equal(refused.status, 409);
  const { error } = await refused.json();
  if (windows && error.code === 'terminal_unavailable') return assert.equal(error.message, await closedRefusal(app, TERMINAL_UNAVAILABLE));
  assert.equal(error.code, 'no_working_folder');
});

// Agent commands in the shared shell. The model's call is the same as before; what changes is
// where it runs: in the conversation's shell, in view, with its state kept.
const TURN = '019a2222-2222-7222-8222-222222222222';
async function agentCall(app, id, args, decide = 'approve_once') {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: TURN, callId: id, arguments: args } });
  if (decide) {
    const snapshot = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.scopedApproval, 'approval popup');
    await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${snapshot.scopedApproval.id}`, { action: decide }));
  }
  const state = await eventually(() => app.inspect(), value => value.responses.some(reply => reply.id === id), `tool reply ${id}`, 20000);
  return state.responses.find(reply => reply.id === id).result;
}
const said = result => result.contentItems.map(item => item.text).join('\n');

terminalTest('an approved agent command runs in the shared shell: in view, captured whole, state kept', async t => {
  const { app, site } = await fixture(t);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: crypto.randomUUID() }), 202);
  const term = await attach(app);
  await term.shows(prompt, 'a prompt');
  const result = await agentCall(app, 'cmd-1', { action: 'run_shell', text: 'cat notes.txt; echo AGENT-RAN; exit 3', cwd: site });
  assert.equal(result.success, true);
  assert.match(said(result), /Exit code: 3/);
  assert.match(said(result), /inside the folder/);
  assert.match(said(result), /AGENT-RAN/);
  assert.doesNotMatch(said(result), /7331/, 'the markers are not part of the result');
  // The user saw it run: the command line, dimmed, and its output.
  await term.shows(/AGENT-RAN/);
  assert.match(term.screen(), /\x1b\[2m.*cat notes\.txt; echo AGENT-RAN/);
  // `exit 3` inside the command file does end the shell, as `exit` would when typed; the tool says so.
  const entry = (await json(await app.request(`/api/threads/${THREAD}`))).activities.find(item => item.id === 'cmd-1');
  assert.deepEqual([entry.kind, entry.exitCode, entry.status], ['command', 3, 'ok']);
  assert.match(entry.output, /AGENT-RAN/);
  await term.feed.close();
});

terminalTest('the shell keeps state between the agent and the user, and a long output is capped only for the model', async t => {
  const { app, site } = await fixture(t);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: crypto.randomUUID() }), 202);
  const term = await attach(app);
  await term.shows(prompt, 'a prompt');
  await agentCall(app, 'cd-1', { action: 'run_shell', text: 'mkdir -p deep; cd deep; export MC_SHARED=yes', cwd: site });
  await term.type('echo "pwd=$PWD shared=$MC_SHARED"\r');
  await term.shows(/shared=yes/);
  assert.match(term.screen(), /pwd=.*\/site\/deep/);
  // A command's cwd is what was approved, whatever the shell was in.
  // `pwd` is on the read-only list, so it runs without a popup.
  const back = await agentCall(app, 'cd-2', { action: 'run_shell', text: 'pwd', cwd: site }, null);
  assert.match(said(back), new RegExp(`${escaped(shellPath(site))}\\n`), 'line endings are plain newlines for the model');
  // `head | tr; echo` would run unasked; the write to a file keeps it a command that asks.
  const big = await agentCall(app, 'big-1', { action: 'run_shell', text: 'head -c 100000 /dev/zero | tr "\\0" x > big.txt; cat big.txt; echo; echo END-OF-BIG', cwd: site });
  assert.equal(big.success, true);
  assert.match(said(big), /truncated at 60 KiB/);
  assert.doesNotMatch(said(big), /END-OF-BIG/);
  assert.match(said(big), /Exit code: 0/);
  await term.shows(/END-OF-BIG/);
  // A conversation whose shell nobody opened still runs its commands in a shell of its own.
  await term.feed.close();
});

terminalTest('a Claude agent command runs in the shared shell too', async t => {
  const { fileURLToPath } = await import('node:url');
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-terminal-claude-')));
  const site = join(root, 'site');
  await (await import('node:fs/promises')).mkdir(site);
  await writeFile(join(site, 'notes.txt'), 'inside the folder\n');
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: join(root, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { project } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site }), 201);
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, model: MODEL }), 201);
  const term = await attach(app, thread.id);
  await term.shows(prompt, 'a prompt');
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: `tool ${JSON.stringify({ action: 'run_shell', text: 'cat notes.txt; echo CLAUDE-RAN', cwd: site })}`,
    clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  let decided = false;
  const done = await eventually(async () => {
    const value = await json(await app.request(`/api/threads/${thread.id}`));
    if (!decided && value.scopedApproval) { decided = true; await json(await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: 'approve_once' })); }
    return value;
  }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 20000);
  assert.match(done.messages.at(-1).text, /error=false/);
  assert.match(done.messages.at(-1).text, /CLAUDE-RAN/);
  await term.shows(/CLAUDE-RAN/);
  await term.feed.close();
});

test('a command past the limit is reported as still running, and Activity finishes when it ends', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-terminal-slow-')));
  const site = join(root, 'site');
  await (await import('node:fs/promises')).mkdir(site);
  const app = await launchApp({}, { env: { MINICLAW_TEST_TERMINAL_TIMEOUT_MS: '1500' } });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { project } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: crypto.randomUUID() }), 202);
  const offered = ((await app.inspect()).requests.find(request => request.method === 'thread/start').params.dynamicTools ?? [])
    .some(tool => tool.name === 'miniclaw_scoped_native');
  if (windows && !offered) {
    // No shell tool was offered, so a call the model makes anyway is refused without a popup.
    const refused = await agentCall(app, 'slow-1', { action: 'run_shell', text: 'echo started', cwd: site }, null);
    assert.equal(refused.success, false);
    const activity = (await json(await app.request(`/api/threads/${THREAD}`))).activities.find(item => item.id === 'slow-1');
    return assert.equal(activity.status, 'refused');
  }
  const slow = await agentCall(app, 'slow-1', { action: 'run_shell', text: 'echo started; sleep 3; echo finished-late; false', cwd: site });
  assert.equal(slow.success, true);
  assert.match(said(slow), /started/);
  assert.match(said(slow), /still running in the terminal/);
  assert.doesNotMatch(said(slow), /finished-late/);
  const entry = await eventually(async () => (await json(await app.request(`/api/threads/${THREAD}`))).activities.find(item => item.id === 'slow-1'),
    item => item?.exitCode === 1, 'the late finish', 15000);
  assert.match(entry.output, /finished-late/);
});

terminalTest('a permission change restarts the shell before the next command, and the page is told', async t => {
  const { app, site, root } = await fixture(t);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: crypto.randomUUID() }), 202);
  const term = await attach(app);
  await term.shows(prompt, 'a prompt');
  // Both platforms run a POSIX shell; the variable must be visible before the restart can remove it.
  await term.type('export MC_BEFORE=1; echo "set=$MC_BEFORE"\r');
  await term.shows(/\nset=1\r?\n/, 'the variable is set in the old shell');
  const { project } = (await json(await app.request('/api/projects'))).projects[0] ? { project: (await json(await app.request('/api/projects'))).projects[0] } : {};
  await json(await post(app, `/api/permissions/project/${project.id}`, { target: { target_type: 'folder', path: root, action: 'read' }, effect: 'allow' }));
  await agentCall(app, 'after-1', { action: 'run_shell', text: 'echo "before=$MC_BEFORE"', cwd: site });
  await term.feed.next('notice');
  assert.match(term.feed.seen.find(event => event.event === 'notice').data.text, /permissions changed/);
  await term.feed.close();
  const fresh = await attach(app);
  await fresh.shows(/before=\r?\n/, 'the variable is gone with the old shell');
  await fresh.feed.close();
});
