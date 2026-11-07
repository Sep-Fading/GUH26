import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { launchApp, json, events, eventually } from './support/harness.mjs';
import { sandboxReport } from './support/sandbox.mjs';
import { THREAD } from './support/fixtures.mjs';

const binary = fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url));
const MODEL = 'claude-sonnet-5';

// Each app gets a private call journal written by the fake CLI.
async function launchLogged(t, env = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'mc-claude-'));
  const journal = join(dir, 'calls.jsonl');
  const app = await launchApp({}, { env: { CLAUDE_BIN: binary, MC_CLAUDE_LOG: journal,
    ANTHROPIC_API_KEY: 'must-not-pass', ...env } });
  t.after(async () => { await app.close(); await rm(dir, { recursive: true, force: true }); });
  return Object.assign(app, { journal });
}
const calls = async app => (await readFile(app.journal, 'utf8')).trim().split('\n').map(l => JSON.parse(l));
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const create = async (app, body = {}) => (await json(await post(app, '/api/threads',
  { environment: { kind: 'host' }, model: MODEL, ...body }), 201)).thread;
const send = (app, id, text, extra = {}) => post(app, `/api/threads/${id}/messages`,
  { text, clientMessageId: crypto.randomUUID(), model: MODEL, ...extra });

test('catalog adds Claude models beside Codex models', async t => {
  const app = await launchLogged(t);
  const { models } = await json(await app.request('/api/models'));
  assert.deepEqual(models.filter(m => !m.provider).map(m => m.model), ['fixture-model', 'other-model']);
  const claude = models.filter(m => m.provider === 'claude');
  assert.ok(claude.some(m => m.model === MODEL));
  assert.equal(claude.some(m => m.isDefault), false);
});

test('Claude stays out of the catalog when its CLI is disabled', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { models } = await json(await app.request('/api/models'));
  assert.deepEqual(models.map(m => m.model), ['fixture-model', 'other-model']);
});

test('a Claude chat streams a reply without touching Codex', async t => {
  const app = await launchLogged(t);
  const thread = await create(app, { effort: 'high' });
  assert.equal(thread.provider, 'claude');
  assert.equal(thread.model, MODEL);
  assert.notEqual(thread.id, THREAD);
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  t.after(() => stream.close());
  const { turnId } = await json(await send(app, thread.id, 'Hello', { effort: 'high' }), 202);
  const done = await stream.next('turn.completed');
  assert.deepEqual(done.data, { turnId, status: 'completed', error: null });
  assert.deepEqual(stream.seen.filter(e => e.event === 'message.delta').map(e => e.data.delta),
    ['Hello ', 'from Claude']);
  assert.equal(stream.seen.find(e => e.event === 'message.completed').data.text, 'Hello from Claude');
  assert.equal(JSON.stringify(stream.seen).includes('private'), false);

  const [call] = await calls(app);
  assert.equal(call.prompt, 'Hello');
  assert.equal(call.cwd, app.peer.cwd);
  assert.equal(call.apiKey, null);
  const flag = name => call.args[call.args.indexOf(name) + 1];
  assert.equal(flag('--session-id'), thread.id);
  assert.equal(flag('--model'), MODEL);
  assert.equal(flag('--effort'), 'high');
  // Requirement change 2026-09-21: web search is on by default, and safe mode would remove it, so
  // a plain turn loads no settings files instead. With search off it runs in safe mode (tested below).
  assert.equal(flag('--tools'), 'WebSearch');
  assert.equal(flag('--setting-sources'), '');
  for (const name of ['-p', '--strict-mcp-config', '--disable-slash-commands']) {
    assert.ok(call.args.includes(name), name);
  }
  assert.match(flag('--system-prompt'), /MiniClaw/);
  const codex = (await app.inspect()).requests.map(r => r.method);
  assert.equal(codex.includes('thread/start') || codex.includes('turn/start'), false);
});

test('later turns resume the same Claude session', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  for (const text of ['One', 'Two']) {
    await json(await send(app, thread.id, text), 202);
    await eventually(async () => (await json(await app.request(`/api/threads/${thread.id}`))),
      s => !s.activeTurn && !s.pending && s.messages.length === (text === 'One' ? 2 : 4), `reply to ${text}`);
  }
  const [first, second] = await calls(app);
  assert.ok(first.args.includes('--session-id') && !first.args.includes('--resume'));
  assert.equal(second.args[second.args.indexOf('--resume') + 1], thread.id);
  assert.equal(second.args.includes('--session-id'), false);
});

test('Claude chats are listed and reopen with history after restart', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  await json(await send(app, thread.id, 'Remember me'), 202);
  await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    s => !s.activeTurn && s.messages.length === 2, 'first reply');
  await app.restart();
  const { threads } = await json(await app.request('/api/threads'));
  const listed = threads.find(task => task.id === thread.id);
  assert.equal(listed.provider, 'claude');
  assert.equal(listed.name, 'Remember me');
  assert.ok(threads.some(task => task.id === THREAD), 'Codex tasks remain listed');
  const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
  assert.deepEqual(snapshot.messages.map(m => [m.role, m.text]),
    [['user', 'Remember me'], ['assistant', 'Hello from Claude']]);
});

test('Stop interrupts the Claude process', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  t.after(() => stream.close());
  const { turnId } = await json(await send(app, thread.id, 'hang'), 202);
  await stream.next('message.delta');
  await json(await post(app, `/api/threads/${thread.id}/interrupt`, { turnId }));
  assert.equal((await stream.next('turn.completed')).data.status, 'interrupted');
});

test('a failed Claude run ends the turn as failed', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  t.after(() => stream.close());
  await json(await send(app, thread.id, 'fail'), 202);
  const done = await stream.next('turn.completed');
  assert.equal(done.data.status, 'failed');
  assert.match(done.data.error, /Claude/);
});

test('a Claude chat rejects Codex models and Computer Use', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  assert.equal((await send(app, thread.id, 'Hello', { model: 'fixture-model' })).status, 400);
  assert.equal((await post(app, '/api/threads',
    { environment: { kind: 'host' }, model: MODEL, computerUse: true })).status, 400);
  assert.deepEqual(await calls(app).catch(() => []), []);
});

test('Claude sign-in is required before a Claude chat is created', async t => {
  const signedOut = await launchLogged(t, { MC_CLAUDE_SIGNED_OUT: '1' });
  const response = await post(signedOut, '/api/threads', { environment: { kind: 'host' }, model: MODEL });
  assert.equal(response.status, 401);
  assert.equal((await response.json()).error.code, 'claude_sign_in_required');
});

test('deleting a Claude chat removes it from the list', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  await json(await post(app, `/api/threads/${thread.id}/delete`));
  const { threads } = await json(await app.request('/api/threads'));
  assert.equal(threads.some(task => task.id === thread.id), false);
});

test('deleting a Claude chat also removes the CLI session file for that chat only', async t => {
  const config = await mkdtemp(join(tmpdir(), 'mc-claude-config-'));
  t.after(() => rm(config, { recursive: true, force: true }));
  const app = await launchLogged(t, { CLAUDE_CONFIG_DIR: config });
  const thread = await create(app);
  // The CLI keeps one file per session under a folder named after the working directory.
  const sessions = join(config, 'projects', app.peer.cwd.replace(/[^A-Za-z0-9]/g, '-'));
  await mkdir(sessions, { recursive: true });
  await writeFile(join(sessions, `${thread.id}.jsonl`), '{}');
  await writeFile(join(sessions, 'someone-elses-session.jsonl'), '{}');
  await json(await post(app, `/api/threads/${thread.id}/delete`));
  await assert.rejects(readFile(join(sessions, `${thread.id}.jsonl`)), { code: 'ENOENT' });
  assert.equal(await readFile(join(sessions, 'someone-elses-session.jsonl'), 'utf8'), '{}');
});

// Safe mode applies only to a turn with no tool at all. A Mac host chat always carries the
// native tool schema (TESTING.md, 2026-09-26 main-agent correction), so it never qualifies.
// A Windows host chat carries it too when its sandbox is available; without the sandbox and
// without a shell allowance it has no schema, and the turn uses safe mode.
test('a turn with everything switched off uses safe mode only without the host tool schema', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  const rule = async target => json(await post(app,
    `/api/permissions/conversation/${thread.id}`, { target, effect: 'allow' }));
  await rule({ target_type: 'feature', name: 'no_web_search' });
  await rule({ target_type: 'feature', name: 'no_questions' });
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  await json(await send(app, thread.id, 'Quiet'), 202);
  await stream.next('turn.completed');
  stream.close();
  const off = (await calls(app)).at(-1).args;
  assert.ok(off.includes('--tools'), `not a Claude turn: ${JSON.stringify(await calls(app)).slice(-1500)}`);
  assert.equal(off[off.indexOf('--tools') + 1], '');
  // The backend's own report says whether a Windows host chat carries the schema.
  const schemaOffered = process.platform !== 'win32' || (await sandboxReport(app)).available;
  assert.equal(off.includes('--safe-mode'), !schemaOffered);
  assert.equal(off.includes('--mcp-config'), schemaOffered, 'the shim is configured only with a schema');
  assert.equal(off.includes('--setting-sources'), schemaOffered, 'safe mode replaces the no-settings path');
});

test('fetch, subagents and task tracking switch independently', async t => {
  const app = await launchLogged(t);
  // This case requires the host schema even when native sandbox execution is unavailable.
  // An allowance offers timed-shell tools without granting unrestricted execution.
  await json(await post(app, '/api/permissions/agent/00000000-0000-0000-0000-000000000000', {
    target: { target_type: 'feature', name: 'unsandboxed_shell' }, effect: 'allow' }));
  const thread = await create(app);
  const rule = async (target, effect = 'allow') => json(await post(app,
    `/api/permissions/conversation/${thread.id}`, { target, effect }));
  const turn = async text => {
    const stream = await events(app, `/api/threads/${thread.id}/events`);
    await json(await send(app, thread.id, text), 202);
    await stream.next('turn.completed');
    stream.close();
    return (await calls(app)).at(-1).args;
  };
  const value = (args, name) => args[args.indexOf(name) + 1];
  // Requirement change 2026-09-22: each additional tool has its own allowance and switch.
  const first = await turn('First');
  assert.equal(value(first, '--tools'), 'WebSearch');
  assert.deepEqual(first.slice(first.indexOf('--allowedTools') + 1).filter(arg => !arg.startsWith('--') && arg.startsWith('Web')), ['WebSearch']);
  assert.equal(value(first, '--setting-sources'), '', 'built-in tools need the no-settings path, since safe mode removes them');
  // Built-in tools can be off while the host MCP schema remains available for timed shell grants.
  await rule({ target_type: 'feature', name: 'no_web_search' });
  await rule({ target_type: 'feature', name: 'no_questions' });
  const off = await turn('Quiet');
  assert.equal(value(off, '--tools'), '');
  assert.equal(off.includes('--safe-mode'), false, 'safe mode disables the host MCP schema');
  assert.equal(value(off, '--setting-sources'), '');
  await rule({ target_type: 'feature', name: 'no_web_search' }, 'remove');
  await rule({ target_type: 'feature', name: 'no_questions' }, 'remove');

  // Requirement change 2026-09-21: an agent's configuration says whether a capability is allowed at all; the conversation only switches it.
  const toggle = async (key, on) => json(await post(app, `/api/threads/${thread.id}/capabilities`, { key, on }));
  for (const key of ['web_fetch', 'subagents', 'task_tracking']) {
    await json(await post(app, '/api/permissions/agent/00000000-0000-0000-0000-000000000000',
      { target: { target_type: 'feature', name: key }, effect: 'allow' }));
  }
  await toggle('web_fetch', true);
  await rule({ target_type: 'host', host: 'docs.rs' });
  await rule({ target_type: 'domain', domain: 'npmjs.org' });
  await rule({ target_type: 'host', host: 'blocked.example.com' }, 'block');
  const on = await turn('Second');
  assert.equal(value(on, '--tools'), 'WebSearch,WebFetch');
  assert.equal(on.includes('--permission-mode'), false, 'plan mode is its own switch');
  // Shell and file tools are never named, and settings files are still not loaded.
  // Checked in the arguments that enable tools; the system prompt is prose and may well say "read".
  const enabling = [value(on, '--tools'), ...on.slice(on.indexOf('--allowedTools') + 1, on.indexOf('--allowedTools') + 1
    + on.slice(on.indexOf('--allowedTools') + 1).findIndex(arg => arg.startsWith('--')))].join(' ');
  for (const never of ['Bash', 'Read', 'Edit', 'Write']) assert.equal(new RegExp(`\\b${never}\\b`).test(enabling), false, never);
  assert.equal(value(on, '--setting-sources'), '');
  const allowed = on.slice(on.indexOf('--allowedTools') + 1).filter(arg => !arg.startsWith('--'));
  // A host rule is one exact rule; a domain rule also needs the CLI's wildcard form, because
  // the CLI matches hosts exactly (measured: docs/native-tools-probe).
  assert.deepEqual(allowed.filter(arg => arg.startsWith('WebFetch')).sort(),
    ['WebFetch(domain:*.npmjs.org)', 'WebFetch(domain:docs.rs)', 'WebFetch(domain:npmjs.org)']);
  assert.ok(allowed.includes('WebSearch'));
  for (const name of ['Task', 'TaskCreate', 'TaskGet', 'TaskList', 'TaskUpdate']) assert.ok(!allowed.includes(name), name);
  const denied = on.slice(on.indexOf('--disallowedTools') + 1);
  assert.equal(denied[0], 'WebFetch(domain:blocked.example.com)');
  assert.equal(allowed.includes('WebFetch'), false, 'fetch is never allowed without a domain');

  await toggle('web_fetch', false);
  await toggle('web_search', false);
  await toggle('subagents', true);
  const delegated = await turn('Delegate');
  assert.equal(value(delegated, '--tools'), 'Task');
  assert.ok(!delegated.some(arg => arg.startsWith('WebFetch')), 'fetch rules are absent when fetch is off');
  await toggle('subagents', false);
  await toggle('task_tracking', true);
  const tracked = await turn('Track');
  assert.equal(value(tracked, '--tools'), 'TaskCreate,TaskGet,TaskList,TaskUpdate');
  assert.equal(tracked.includes('--permission-mode'), false, 'task tracking does not enter plan mode');
  for (const name of ['TaskCreate', 'TaskGet', 'TaskList', 'TaskUpdate']) assert.ok(tracked.includes(name));
  await toggle('plan_mode', true);
  await toggle('web_fetch', true);
  await toggle('subagents', true);
  const combined = await turn('Plan with selected tools');
  assert.equal(value(combined, '--tools'), 'WebFetch,Task,TaskCreate,TaskGet,TaskList,TaskUpdate');
  assert.equal(value(combined, '--permission-mode'), 'plan');
});

test('plan mode adds no tool of its own', async t => {
  const app = await launchLogged(t);
  const thread = await create(app);
  await json(await post(app, `/api/permissions/conversation/${thread.id}`,
    { target: { target_type: 'feature', name: 'plan_mode' }, effect: 'allow' }));
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  t.after(() => stream.close());
  await json(await send(app, thread.id, 'Plan it'), 202);
  await stream.next('turn.completed');
  const { args } = (await calls(app)).at(-1);
  assert.equal(args[args.indexOf('--permission-mode') + 1], 'plan');
  // Requirement change 2026-09-21: web search is on by default; plan mode adds nothing to it.
  assert.equal(args[args.indexOf('--tools') + 1], 'WebSearch', 'plan mode alone enables no further tool');
});
