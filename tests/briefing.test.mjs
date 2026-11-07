// Checks what a model is told about where it is: MiniClaw, this Mac, its tools, its working
// folder, and what is switched on. A model that knows these has no reason to ask the user for them.
import assert from 'node:assert/strict';
// Registers the briefing tests.
import { test } from 'node:test';
// Builds a real working folder with a git repository in it.
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
// Reads whether this backend's sandbox is available, and why not.
import { assertGate, sandboxReport } from './support/sandbox.mjs';
import { linkOrRefused, NO_LINKS, ownAsUser } from './support/windows-links.mjs';
// Names the fake conversation.
import { THREAD, CLIENT, started } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const MAIN = '00000000-0000-0000-0000-000000000000';
const last = async (app, method) => (await app.inspect()).requests.filter(request => request.method === method).at(-1).params;
const environment = async app => (await last(app, 'turn/start')).additionalContext.miniclaw_environment.value;

test('a plain conversation is told what MiniClaw is, what it can use, and to look before it asks', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const base = (await last(app, 'thread/start')).baseInstructions;
  assert.doesNotMatch(base, /No tools are available/);
  // A Mac host chat always carries the native tool schema (TESTING.md, 2026-09-26), and so does a
  // Windows host chat whose sandbox is available. Without the sandbox and without a shell allowance
  // a Windows chat has no such tool, and is told so with the reason the backend reports.
  const windows = process.platform === 'win32';
  const report = await sandboxReport(app);
  const closed = windows && !report.available;
  const native = closed
    ? [/You run on Windows, (aarch64|x86_64)/, /no file or command tool/i,
      /sandbox is not available on this Windows computer/, /timed Unsandboxed shell access from Tools/]
    : windows ? [/You run on Windows, (aarch64|x86_64)/, /miniclaw_scoped_native/, /bundled POSIX shell/, /live timed Unsandboxed shell grant/]
    : process.platform === 'linux' ? [/the user's Linux computer/, /(aarch64|x86_64)/, /miniclaw_scoped_native/, /bubblewrap sandbox/, /live timed Unsandboxed shell grant/, /access expires or is revoked/]
    : [/macOS \d+/, /(arm64|x86_64)/, /miniclaw_scoped_native/, /live timed Unsandboxed shell grant/, /access expires or is revoked/];
  for (const must of [/MiniClaw/, ...native, /miniclaw_ask/, /miniclaw_mcp/, /web search/i,
    /Settings/, /before you ask/i, /never ask .*permission/i]) assert.match(base, must);
  if (windows || process.platform === 'linux') assert.doesNotMatch(base, /macOS \d+|user's own Mac/);
  // Neither an agent nor a project brings the tool while the Windows sandbox is unavailable, and
  // the model is given the reason to pass on.
  if (closed) {
    assert.doesNotMatch(base, /miniclaw_scoped_native|starting the conversation with an agent/);
    assert.ok(base.includes(report.reason), 'the reason the sandbox is unavailable');
  } else if (windows) assert.doesNotMatch(base, /sandbox is not available/);
  // A tool is described exactly when it is offered.
  const tools = (await last(app, 'thread/start')).dynamicTools;
  assert.equal(tools.some(tool => tool.name === 'miniclaw_scoped_native'), !closed, 'the tool is offered exactly when described');
  assert.ok(base.length < 6000, `the briefing stays short: ${base.length}`);
});

test('each message carries the current state: date, machine, agent, and what is on, off or not allowed', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: process.execPath }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Hello', clientMessageId: CLIENT }), 202);
  const first = await environment(app);
  assert.match(first, new RegExp(new Date().toISOString().slice(0, 4)));
  const machine = { win32: /this Windows computer/, linux: /Machine: this computer/ }[process.platform] ?? /this Mac/;
  for (const must of [machine, /main agent/i, /Web search: on/, /Network for commands: not allowed/, /Asking the user questions: on/,
    /MCP servers: none switched on/]) assert.match(first, must);
  if (process.platform !== 'darwin') assert.doesNotMatch(first, /this Mac/);
  assert.doesNotMatch(first, /\d\d:\d\d/, 'no clock time, so the text only changes when the state does');
  // Allowed and switched on, it says so by name on the next message.
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'feature', name: 'network' }, effect: 'allow' }));
  for (const key of [`mcp:${server.id}`, 'network']) await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key, on: true }));
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: '019a2222-2222-7222-8222-222222222222', status: 'completed', items: [] } } });
  const { eventually } = await import('./support/harness.mjs');
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: crypto.randomUUID() }), 202);
  const second = await environment(app);
  assert.match(second, /Network for commands: on/);
  assert.match(second, /MCP servers: notes/);
});

test('a project conversation is told its folder, what is in it, its git state and the toolchain on this Mac', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-briefing-')));
  // On Windows the product's own view of a repository uses the private Git that setup installs.
  const app = await launchApp({}, { git: true });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  await mkdir(join(root, 'src'));
  await writeFile(join(root, 'package.json'), '{}');
  await writeFile(join(root, 'README.md'), '# Demo');
  const git = (...args) => execFileSync('git', args, { cwd: root, stdio: 'ignore',
    env: { ...process.env, GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@example.com', GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@example.com' } });
  git('init', '-b', 'feature/briefing'); git('add', '.'); git('commit', '-m', 'first');
  ownAsUser(root);
  await writeFile(join(root, 'notes.txt'), 'uncommitted');
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build things.', active: true }), 201);
  const { project } = await json(await post(app, '/api/projects', { name: 'Demo app', environment: { kind: 'host' }, workspace: root, agentIds: [agent.id] }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: agent.id }), 201);
  const start = await last(app, 'thread/start');
  const base = start.baseInstructions;
  // While Windows cannot enforce the sandbox, a host project conversation is given no file or
  // command tool, so its folder is not surveyed. Only the tools the backend offered select this.
  if (await assertGate(app, process.platform === 'win32' && !(start.dynamicTools ?? []).some(tool => tool.name === 'miniclaw_scoped_native'))) {
    assert.match(base, /You have no file or command tool in this conversation/);
    assert.equal(base.includes(root), false, 'no folder survey without the tool');
    assert.doesNotMatch(base, /branch feature\/briefing/);
  } else {
    assert.match(base, /miniclaw_scoped_native/);
    assert.ok(base.includes(root), 'the working folder');
    for (const name of ['package.json', 'README.md', 'src/', 'notes.txt']) assert.ok(base.includes(name), name);
    assert.match(base, /branch feature\/briefing/);
    assert.match(base, /uncommitted changes/);
  }
  assert.match(base, /git \d+\.\d+/, 'the toolchain found on this Mac');
  assert.match(base, /node \d+\./);
  assert.doesNotMatch(base, /Use only miniclaw_scoped_native/);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Hello', clientMessageId: CLIENT }), 202);
  const state = await environment(app);
  assert.match(state, /Agent: Builder/);
  assert.match(state, /Project: Demo app/);
});

test('without the private Git a Windows project conversation is not told that its folder has no repository',
  { skip: process.platform !== 'win32' && 'the Mac runs the system git inside its sandbox' }, async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-briefing-')));
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  await writeFile(join(root, 'README.md'), '# Demo');
  execFileSync('git', ['init', '-b', 'main'], { cwd: root, stdio: 'ignore' });
  const report = await sandboxReport(app);
  if (!report.available) return;
  assert.deepEqual([report.git.available, report.git.reason], [false, "MiniClaw's private Git is not installed. Run `miniclaw update` to install it."]);
  const { project } = await json(await post(app, '/api/projects', { name: 'Demo app', environment: { kind: 'host' }, workspace: root }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
  const base = (await last(app, 'thread/start')).baseInstructions;
  assert.ok(base.includes(root), 'the working folder');
  assert.match(base, /Git could not be checked for this folder/);
  assert.doesNotMatch(base, /not a git repository/);
});

test('the question tool tells the model what it must find out by itself', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const tool = (await last(app, 'thread/start')).dynamicTools.find(tool => tool.name === 'miniclaw_ask');
  for (const must of [/look first/i, /files?/i, /installed/i, /only the user/i]) assert.match(tool.description, must);
});

test('a project conversation is told the folder\'s AGENTS.md and CLAUDE.md and the project\'s own instructions', async t => {
  const { symlink } = await import('node:fs/promises');
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-rules-')));
  const outside = await realpath(await mkdtemp(join(tmpdir(), 'mc-rules-out-')));
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); await rm(outside, { recursive: true, force: true }); });
  await writeFile(join(root, 'AGENTS.md'), '# House rules\nRun `npm test` before you finish. RULE-AGENTS');
  await writeFile(join(root, 'CLAUDE.md'), 'Prefer small commits. RULE-CLAUDE');
  const project = async extra => (await json(await post(app, '/api/projects', { name: `P${crypto.randomUUID().slice(0, 6)}`, environment: { kind: 'host' }, ...extra }), 201)).project;
  const base = async id => {
    // The fake Codex names every conversation alike unless told otherwise.
    await app.set('thread/start', { result: started(app.peer.cwd, { id: crypto.randomUUID() }) });
    await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: id }), 201);
    return (await last(app, 'thread/start')).baseInstructions;
  };
  const both = await base((await project({ workspace: root, instructions: 'Always answer in British English. RULE-PROJECT' })).id);
  assert.match(both, /## Project instructions/);
  for (const rule of ['RULE-AGENTS', 'RULE-CLAUDE', 'RULE-PROJECT']) assert.ok(both.includes(rule), rule);
  assert.ok(both.indexOf('RULE-AGENTS') < both.indexOf('RULE-CLAUDE'), 'AGENTS.md comes first');

  // A CLAUDE.md that only points at AGENTS.md, or repeats it, is not told twice.
  await writeFile(join(root, 'CLAUDE.md'), '@AGENTS.md\n');
  assert.equal((await base((await project({ workspace: root })).id)).split('RULE-AGENTS').length, 2);
  // A link that leaves the folder is not followed, and a long file is cut with a line that says so.
  await rm(join(root, 'CLAUDE.md'));
  await writeFile(join(outside, 'secret.md'), 'OUTSIDE-TEXT');
  if (!await linkOrRefused(join(outside, 'secret.md'), join(root, 'CLAUDE.md'))) return t.skip(NO_LINKS);
  await writeFile(join(root, 'AGENTS.md'), `RULE-START ${'x'.repeat(40000)} RULE-END`);
  const cut = await base((await project({ workspace: root })).id);
  assert.ok(cut.includes('RULE-START') && !cut.includes('RULE-END') && !cut.includes('OUTSIDE-TEXT'));
  assert.match(cut, /cut at 32 KiB/);
  // Without a folder the project's own instructions still arrive; too long a text is refused.
  assert.ok((await base((await project({ instructions: 'RULE-ONLY' })).id)).includes('RULE-ONLY'));
  assert.equal((await post(app, '/api/projects', { name: 'Too long', environment: { kind: 'host' }, instructions: 'x'.repeat(16001) })).status, 400);
});

test('a Claude project conversation is told the same instructions', async t => {
  const { readFile } = await import('node:fs/promises');
  const { fileURLToPath } = await import('node:url');
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-rules-claude-')));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: join(root, '.calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  await writeFile(join(root, 'AGENTS.md'), 'RULE-AGENTS');
  const MODEL = 'claude-sonnet-5';
  const { project } = await json(await post(app, '/api/projects', { name: 'Claude rules', environment: { kind: 'host' }, workspace: root, instructions: 'RULE-PROJECT' }), 201);
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, model: MODEL }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Hello', clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const { eventually } = await import('./support/harness.mjs');
  await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)), value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  const run = (await readFile(join(root, '.calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line)).at(-1);
  const system = run.args[run.args.indexOf('--system-prompt') + 1];
  for (const rule of ['RULE-AGENTS', 'RULE-PROJECT']) assert.ok(system.includes(rule), rule);
});
