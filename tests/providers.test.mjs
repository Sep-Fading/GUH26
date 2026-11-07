import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { launchApp, json, eventually } from './support/harness.mjs';
import { signedOut, THREAD } from './support/fixtures.mjs';

const binary = fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url));
const MODEL = 'claude-sonnet-5';

// Starts an app with the fake Claude CLI; `options` choose Codex handlers or no Codex.
async function launch(t, { handlers = {}, env = {}, codex } = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'mc-providers-'));
  const app = await launchApp(handlers, { codex, env: { CLAUDE_BIN: binary,
    MC_CLAUDE_LOG: join(dir, 'calls.jsonl'), MC_CLAUDE_LOGIN_FILE: join(dir, 'login'), ...env } });
  t.after(async () => { await app.close(); await rm(dir, { recursive: true, force: true }); });
  return app;
}
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const provider = (account, id) => account.providers.find(entry => entry.id === id);

// Creates a Claude chat, sends one message and waits for the saved reply.
async function converse(app) {
  const { thread } = await json(await post(app, '/api/threads',
    { environment: { kind: 'host' }, model: MODEL }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`,
    { text: 'Hello', clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const snapshot = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    s => !s.activeTurn && !s.pending && s.messages.length === 2, 'Claude reply');
  assert.equal(snapshot.messages[1].text, 'Hello from Claude');
  return thread;
}

test('Claude works with no Codex runtime', async t => {
  const app = await launch(t, { codex: false });
  const health = await json(await app.request('/api/health'));
  assert.deepEqual([health.codex, health.claude], ['unavailable', 'connected']);
  const account = await json(await app.request('/api/account'));
  assert.equal(account.status, 'signed_in');
  assert.equal(account.email, 'claude-reader@example.test');
  assert.equal(provider(account, 'claude').status, 'signed_in');
  assert.equal(provider(account, 'codex').status, 'unavailable');
  const { models } = await json(await app.request('/api/models'));
  assert.ok(models.length && models.every(m => m.provider === 'claude'));
  assert.ok(models.some(m => m.isDefault), 'the only provider supplies the default model');
  const thread = await converse(app);
  const { threads } = await json(await app.request('/api/threads'));
  assert.deepEqual(threads.map(task => task.id), [thread.id]);
  await json(await post(app, `/api/threads/${thread.id}/delete`));
  assert.equal((await json(await app.request('/api/agents'))).agents.length, 0);
});

test('Claude works while ChatGPT is signed out', async t => {
  const app = await launch(t, { handlers: { 'account/read': { result: signedOut } } });
  const account = await json(await app.request('/api/account'));
  assert.equal(account.status, 'signed_in');
  assert.equal(provider(account, 'codex').status, 'signed_out');
  const { models } = await json(await app.request('/api/models'));
  assert.ok(models.every(m => m.provider === 'claude'));
  await converse(app);
  // A Codex model cannot start a chat while its provider is signed out.
  assert.equal((await post(app, '/api/threads',
    { environment: { kind: 'host' }, model: 'fixture-model' })).status, 401);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

test('Codex works while Claude is signed out', async t => {
  const app = await launch(t, { env: { MC_CLAUDE_SIGNED_OUT: '1' } });
  const account = await json(await app.request('/api/account'));
  assert.equal(account.status, 'signed_in');
  assert.equal(account.email, 'reader@example.test');
  assert.equal(provider(account, 'claude').status, 'signed_out');
  const { models } = await json(await app.request('/api/models'));
  assert.deepEqual(models.map(m => m.model), ['fixture-model', 'other-model']);
  const { threads } = await json(await app.request('/api/threads'));
  assert.ok(threads.some(task => task.id === THREAD));
});

test('no signed-in provider requires sign-in', async t => {
  const app = await launch(t, { handlers: { 'account/read': { result: signedOut } },
    env: { MC_CLAUDE_SIGNED_OUT: '1' } });
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
  assert.equal((await app.request('/api/models')).status, 401);
});

test('Claude sign-in starts from the app', async t => {
  const app = await launch(t, { codex: false, env: { MC_CLAUDE_SIGNED_OUT: '1' } });
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
  const login = await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  assert.equal(login.status, 'pending');
  assert.equal(login.provider, 'claude');
  await eventually(async () => json(await app.request('/api/account')),
    account => account.status === 'signed_in', 'Claude sign-in');
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'succeeded');
});

// Reads one JSON-lines record file, or none when nothing was written.
const records = async file => (await readFile(file, 'utf8').catch(() => '')).split('\n').filter(Boolean);
// Starts a signed-out Claude with log files in a fresh folder for one sign-in test.
async function signedOutClaude(t, env = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'mc-claude-login-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const files = { cli: join(dir, 'cli-browser.jsonl'), opened: join(dir, 'miniclaw-browser.txt'),
    codes: join(dir, 'codes.txt'), pid: join(dir, 'pid'), status: join(dir, 'status-calls.txt') };
  const app = await launch(t, { codex: false, env: { MC_CLAUDE_SIGNED_OUT: '1', MC_CLAUDE_BROWSER_LOG: files.cli,
    MC_CLAUDE_CODE_LOG: files.codes, MC_CLAUDE_LOGIN_PID: files.pid, MC_CLAUDE_STATUS_LOG: files.status,
    MINICLAW_TEST_BROWSER_LOG: files.opened, ...env } });
  return { app, files };
}
const SIGN_IN_PAGE = /^https:\/\/claude\.com\/cai\/oauth\/authorize\?code=true&.*redirect_uri=https%3A%2F%2Fplatform\.claude\.com%2Foauth%2Fcode%2Fcallback&.*state=fake-state-0123456789$/;

// User report 2026-10-07 (0.1.5 on Windows): "Sign in" waited and no page opened. The page now
// gets the sign-in address at once, and exactly one browser opener runs, for the automatic
// (localhost callback) page: the CLI's own on macOS (unchanged); on Windows the CLI runs
// MiniClaw's backend executable as its BROWSER, which opens the page outside the CLI's job
// (lead decision 2026-10-07: the automatic flow stays the normal path on Windows).
test('Claude sign-in returns its sign-in page and opens the browser once', async t => {
  const { app, files } = await signedOutClaude(t);
  const login = await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  assert.equal(login.status, 'pending');
  assert.equal(login.provider, 'claude');
  assert.match(login.authUrl, SIGN_IN_PAGE);
  assert.equal(login.codeEntry, true);
  await eventually(async () => json(await app.request('/api/account')),
    account => account.status === 'signed_in', 'Claude sign-in');
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'succeeded');
  const cli = (await records(files.cli)).map(line => JSON.parse(line));
  const opened = await records(files.opened);
  if (process.platform === 'win32') {
    assert.deepEqual(cli.map(entry => entry.opener), ['BROWSER'], 'the CLI hands its page to an opener');
    assert.match(cli[0].browser, /^[A-Za-z]:\\.*\\miniclaw-server\.exe$/i, 'the opener is MiniClaw\'s own executable');
    assert.equal(cli[0].status, 0);
    assert.equal(opened.length, 1, 'MiniClaw opens the page once');
    assert.match(opened[0], /redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback/, 'the automatic page, not the code page');
  } else {
    assert.deepEqual(cli.map(entry => entry.opener), ['cli'], 'the CLI opens the browser once, as before');
    assert.deepEqual(opened, [], 'MiniClaw does not open a second page');
  }
  assert.doesNotMatch(app.logs(), /fake-state|fake-challenge/, 'the sign-in address is never logged');
});

// While a sign-in waits, the page polls about every two seconds; each poll must not start
// `claude auth status` again (at most one per three seconds).
test('a waiting Claude sign-in reads the account at most once per three seconds', async t => {
  const { app, files } = await signedOutClaude(t, { MC_CLAUDE_LOGIN_MODE: 'paste' });
  await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  const before = (await records(files.status)).length;
  const started = Date.now();
  while (Date.now() - started < 3200) {
    await json(await app.request('/api/auth/status'));
    await json(await app.request('/api/account'));
    await new Promise(resolve => setTimeout(resolve, 150));
  }
  const calls = (await records(files.status)).length - before;
  assert.ok(calls <= 2, `${calls} auth status runs in 3.2 s`);
});

test('a code pasted in MiniClaw finishes the Claude sign-in', async t => {
  const { app, files } = await signedOutClaude(t, { MC_CLAUDE_LOGIN_MODE: 'paste' });
  const login = await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  assert.match(login.authUrl, SIGN_IN_PAGE);
  for (const code of ['not a code', 'abc', 'a#b#c', '']) {
    const refused = await post(app, '/api/auth/code', { code });
    assert.equal(refused.status, 400, code);
    assert.equal((await refused.json()).error.code, 'invalid_sign_in_code');
  }
  assert.equal((await post(app, '/api/auth/code', { code: 'x', extra: 1 })).status, 400);
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'pending');
  await json(await post(app, '/api/auth/code', { code: '  pasted-Code_1#fake-state-0123456789 \n' }));
  await eventually(async () => json(await app.request('/api/account')),
    account => account.status === 'signed_in', 'Claude sign-in by code');
  assert.deepEqual(await records(files.codes), ['pasted-Code_1#fake-state-0123456789']);
  assert.doesNotMatch(app.logs(), /pasted-Code_1|fake-state/, 'the code is never logged');
});

test('a Claude sign-in that the CLI ends with an error shows that error', async t => {
  const { app } = await signedOutClaude(t, { MC_CLAUDE_LOGIN_MODE: 'fail' });
  const start = await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  const failed = start.status === 'failed' ? start : await eventually(async () => json(await app.request('/api/auth/status')),
    status => status.status !== 'pending', 'Claude sign-in failure');
  assert.equal(failed.status, 'failed');
  assert.equal(failed.error, 'Login failed: Request failed with status code 403');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
});

test('a rejected pasted code shows the CLI error and allows another attempt', async t => {
  const { app } = await signedOutClaude(t, { MC_CLAUDE_LOGIN_MODE: 'paste' });
  await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  await json(await post(app, '/api/auth/code', { code: 'rejected#fake-state-0123456789' }));
  const failed = await eventually(async () => json(await app.request('/api/auth/status')),
    status => status.status !== 'pending', 'rejected code');
  assert.equal(failed.status, 'failed');
  assert.equal(failed.error, 'Login failed: Authentication failed: Invalid authorization code');
  const again = await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  assert.equal(again.status, 'pending');
  assert.match(again.authUrl, SIGN_IN_PAGE);
});

test('cancelling a Claude sign-in ends the CLI', async t => {
  const { app, files } = await signedOutClaude(t, { MC_CLAUDE_LOGIN_MODE: 'paste' });
  await json(await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' }));
  const pid = Number(await readFile(files.pid, 'utf8'));
  assert.ok(pid > 0);
  await json(await post(app, '/api/auth/cancel'));
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'cancelled');
  await eventually(async () => { try { process.kill(pid, 0); return true; } catch { return false; } },
    alive => !alive, 'the sign-in CLI to exit');
  assert.equal((await post(app, '/api/auth/code', { code: 'late#fake-state-0123456789' })).status, 409);
});

test('Claude sign-in is refused while Claude is signed in', async t => {
  const app = await launch(t, { codex: false });
  const response = await post(app, '/api/auth/start', { method: 'browser', provider: 'claude' });
  assert.equal(response.status, 409);
  assert.equal((await response.json()).error.code, 'already_signed_in');
  assert.equal((await post(app, '/api/auth/code', { code: 'a#b' })).status, 409);
});

test('a provider can be disabled and enabled without restart, and the choice persists', async t => {
  const app = await launch(t);
  const thread = await converse(app);
  const off = await json(await post(app, '/api/providers/claude', { enabled: false }));
  assert.equal(off.providers.find(entry => entry.id === 'claude').enabled, false);
  assert.equal((await json(await app.request('/api/health'))).claude, 'unavailable');
  const { models } = await json(await app.request('/api/models'));
  assert.deepEqual(models.map(m => m.model), ['fixture-model', 'other-model']);
  assert.equal((await app.request(`/api/threads/${thread.id}`)).status, 503);
  assert.equal((await app.request(`/api/threads/${THREAD}`)).status, 200);

  await app.restart();
  assert.equal((await json(await app.request('/api/health'))).claude, 'unavailable');
  await json(await post(app, '/api/providers/claude', { enabled: true }));
  const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
  assert.equal(snapshot.messages.length, 2);

  await json(await post(app, '/api/providers/codex', { enabled: false }));
  assert.equal((await json(await app.request('/api/health'))).codex, 'unavailable');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
  // The fake Codex binds a control socket; a real restart has no such leftover.
  await rm(app.peer.socket, { force: true });
  await json(await post(app, '/api/providers/codex', { enabled: true }));
  await eventually(async () => json(await app.request('/api/health')),
    health => health.codex === 'connected', 'Codex restart');
});

test('provider settings reject unknown providers and fields', async t => {
  const app = await launch(t);
  assert.equal((await post(app, '/api/providers/gemini', { enabled: true })).status, 404);
  assert.equal((await post(app, '/api/providers/claude', { enabled: 'yes' })).status, 400);
});

test('a crashed provider comes back without restarting MiniClaw', async t => {
  const app = await launch(t);
  // The fake Codex exits as if it had crashed; a real restart has no leftover control socket.
  await app.peer.call({ op: 'shutdown' }).catch(() => {});
  await eventually(async () => json(await app.request('/api/health')),
    health => health.codex === 'unavailable', 'Codex loss noticed');
  await rm(app.peer.socket, { force: true });
  await eventually(async () => json(await app.request('/api/health')),
    health => health.codex === 'connected', 'Codex restarted by itself', 10000);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  assert.equal((await json(await app.request('/api/health'))).claude, 'connected');
});

test('a disabled provider is not restarted behind the user\'s back', async t => {
  const app = await launch(t);
  await json(await post(app, '/api/providers/codex', { enabled: false }));
  await rm(app.peer.socket, { force: true });
  await new Promise(resolve => setTimeout(resolve, 2500));
  assert.equal((await json(await app.request('/api/health'))).codex, 'unavailable');
});
