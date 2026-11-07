import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launchApp, json, eventually, command } from './support/harness.mjs';
import { THREAD, TURN, OTHER, CLIENT, turnDone } from './support/fixtures.mjs';
import { standInHelper } from './support/computer-helper.mjs';

// Fake Codex never executes this path; availability checks still see an executable.
process.env.MINICLAW_HELPER_BIN = standInHelper();
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
const path = `/api/threads/${THREAD}`;
const prompt = (changes = {}) => ({
  id: 'approval-wire-1', method: 'mcpServer/elicitation/request', params: {
    threadId: THREAD, turnId: TURN, serverName: 'miniclaw_computer',
    mode: 'form', message: 'Allow Computer Use to use Calculator?',
    requestedSchema: { type: 'object', properties: {} },
    _meta: { tool_params: { app: 'com.apple.calculator' } }, ...changes,
  },
});
async function start(t, computerUse = true) {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', {
    computerUse, environment: { kind: 'host' },
  }), 201);
  await json(await post(app, `${path}/messages`, {
    text: 'Use Calculator', clientMessageId: CLIENT,
  }), 202);
  return app;
}
async function waiting(app) {
  const snapshot = await eventually(() => app.request(path).then(json),
    value => !!value.approval, 'pending app approval');
  return snapshot.approval;
}
async function response(app, id = 'approval-wire-1') {
  const state = await eventually(() => app.inspect(),
    value => value.responses.some(reply => reply.id === id), 'approval response');
  return state.responses.find(reply => reply.id === id).result;
}

test('computer tools require an explicit chat', async t => {
  const app = await start(t);
  const snapshot = await json(await app.request(path));
  assert.equal(snapshot.thread.computerUse, true);
  const request = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  const helper = request.params.config['mcp_servers.miniclaw_computer'];
  assert.equal(helper.command, command[0]);
  assert.deepEqual(helper.args, ['--computer-helper', process.env.MINICLAW_HELPER_BIN]);
  assert.equal(request.params.config['orchestrator.mcp.enabled'], true);
  assert.equal(request.params.approvalPolicy, 'on-request');
  assert.equal(request.params.approvalsReviewer, 'user');
  assert.deepEqual(request.params.environments, []);
  assert.equal(request.params.sandbox, 'read-only');
});

test('normal chats decline computer requests', async t => {
  const app = await start(t, false);
  const request = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  assert.equal(request.params.config?.['mcp_servers.miniclaw_computer'], undefined);
  await app.emit(prompt());
  assert.equal((await response(app)).action, 'decline');
  assert.equal((await json(await app.request(path))).approval, null);
});

for (const action of ['accept', 'decline']) {
  test(`app approval can ${action}`, async t => {
    const app = await start(t);
    await app.emit(prompt());
    const approval = await waiting(app);
    assert.equal(approval.message, 'Allow Computer Use to use Calculator?');
    assert.notEqual(approval.id, 'approval-wire-1');
    assert.equal((await app.inspect()).responses.length, 0);
    await json(await post(app, `${path}/approvals/${approval.id}`, { action }));
    assert.deepEqual(await response(app), {
      action, content: action === 'accept' ? {} : null,
    });
    assert.equal((await post(app, `${path}/approvals/${approval.id}`, { action })).status, 409);
  });
}

test('approval cannot be answered from another chat', async t => {
  const app = await start(t);
  await app.emit(prompt());
  const approval = await waiting(app);
  assert.equal((await post(app,
    `/api/threads/${OTHER}/approvals/${approval.id}`, { action: 'accept' })).status, 404);
  assert.equal((await post(app, `${path}/approvals/${approval.id}`, {
    action: 'accept', content: { persist: 'always' },
  })).status, 400);
  assert.equal((await app.inspect()).responses.length, 0);
});

test('Stop declines pending and later approvals', async t => {
  const app = await start(t);
  await app.emit(prompt());
  const approval = await waiting(app);
  await json(await post(app, `${path}/interrupt`, { turnId: TURN }));
  assert.equal((await response(app)).action, 'decline');
  assert.equal((await json(await app.request(path))).approval, null);
  const expired = await post(app, `${path}/approvals/${approval.id}`, {
    action: 'accept',
  });
  assert.equal(expired.status, 409);
  assert.equal((await expired.json()).error.code, 'approval_expired');
  await app.emit({ ...prompt(), id: 'after-stop' });
  assert.equal((await response(app, 'after-stop')).action, 'decline');
});

for (const notification of [false, true]) {
  test(`account change stops computer work: notification=${notification}`, async t => {
    const app = await start(t);
    await app.emit(prompt());
    await waiting(app);
    if (notification) {
      await app.emit({ method: 'account/updated', params: { authMode: null } });
    } else {
      await app.set('account/read', { result: { account: null, requiresOpenaiAuth: true } });
      await json(await app.request('/api/account'));
    }
    assert.equal((await response(app)).action, 'decline');
    const state = await eventually(() => app.inspect(),
      value => value.requests.some(r => r.method === 'turn/interrupt'), 'account cancellation');
    assert.deepEqual(state.requests.find(r => r.method === 'turn/interrupt').params,
      { threadId: THREAD, turnId: TURN });
    await app.emit({ method: 'turn/started', params: {
      threadId: THREAD, turn: { id: 'late-turn' },
    } });
    await eventually(() => app.inspect(), value => value.requests.some(r =>
      r.method === 'turn/interrupt' && r.params.turnId === 'late-turn'), 'late turn cancellation');
  });
}

test('completion expires an approval', async t => {
  const app = await start(t);
  await app.emit(prompt());
  const approval = await waiting(app);
  await app.emit(turnDone());
  assert.equal((await response(app)).action, 'decline');
  const expired = await post(app, `${path}/approvals/${approval.id}`, {
    action: 'accept',
  });
  assert.equal(expired.status, 409);
  const { error } = await expired.json();
  assert.equal(error.code, 'approval_expired');
  assert.equal(error.message,
    'This approval is no longer active. Retry the request to get a new prompt.');
  assert.equal(error.outcomeUnknown, false);
  assert.equal(error.retryable, false);
});

test('unsupported requests are refused', async t => {
  const app = await start(t);
  const variants = [
    { serverName: 'other_server' }, { turnId: OTHER },
    { mode: 'url', url: 'https://example.test' },
    { requestedSchema: { type: 'object', properties: { secret: { type: 'string' } } } },
  ];
  for (const [index, changes] of variants.entries()) {
    const id = `invalid-${index}`;
    await app.emit({ ...prompt(changes), id });
    assert.equal((await response(app, id)).action, 'decline');
  }
});

test('tool activity is visible without leaking tool results', async t => {
  const app = await start(t);
  const item = { id: 'tool-1', type: 'mcpToolCall', server: 'miniclaw_computer',
    tool: 'get_app_state', arguments: { app: 'com.apple.calculator' },
    status: 'inProgress' };
  await app.emit({ method: 'item/started', params: { threadId: THREAD,
    turnId: TURN, item } });
  await eventually(() => app.request(path).then(json),
    value => value.computerActivity?.[0]?.status === 'inProgress');
  await app.emit({ method: 'item/completed', params: { threadId: THREAD,
    turnId: TURN, item: { ...item, status: 'completed', result: {
      content: [{ type: 'text', text: 'private native output' }],
    } } } });
  const snapshot = await eventually(() => app.request(path).then(json),
    value => value.computerActivity?.[0]?.status === 'completed');
  assert.equal(JSON.stringify(snapshot).includes('private native output'), false);
});

test('a failed step says why, in the helper\'s words and in plain ones', async t => {
  const app = await start(t);
  const item = { id: 'tool-f', type: 'mcpToolCall', server: 'miniclaw_computer', tool: 'list_apps', arguments: {}, status: 'failed' };
  const failed = async extra => {
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN, item: { ...item, ...extra } } });
    return (await eventually(() => app.request(path).then(json), value => value.computerActivity?.some(entry => entry.id === extra.id))).computerActivity
      .find(entry => entry.id === extra.id);
  };
  // What the user's Mac really returned on 2026-09-21, as the text of a failed result.
  const gone = await failed({ id: 'tool-gone', result: { isError: true,
    content: [{ type: 'text', text: 'Computer Use server error -10005: codex app-server exited before returning a response' }] } });
  assert.match(gone.error, /-10005/);
  assert.match(gone.hint, /desktop app/i);
  const plain = await failed({ id: 'tool-plain', error: { message: 'x'.repeat(900) } });
  assert.equal(plain.error.length, 300);
  assert.equal(plain.hint, undefined);
  // Anything else a failed call returned may describe the screen, so it stays out.
  const quiet = await failed({ id: 'tool-quiet', result: { isError: true, content: [{ type: 'text', text: 'Window title: My bank' }] } });
  assert.equal(quiet.error, undefined);
  assert.equal(JSON.stringify(quiet).includes('bank'), false);
});

// A turn's end cannot leave an unreported tool looking active or successful.
for (const [status, expected] of [
  ['interrupted', 'interrupted'], ['failed', 'unknown'], ['completed', 'unknown'],
]) {
  test(`unfinished tool after ${status}`, async t => {
    const app = await start(t);
    const item = { id: 'tool-pending', type: 'mcpToolCall', server: 'miniclaw_computer',
      tool: 'get_app_state', arguments: { app: 'com.apple.Safari' }, status: 'inProgress' };
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN,
      item: { ...item, id: 'tool-done', status: 'completed' } } });
    await app.emit({ method: 'item/started', params: { threadId: THREAD, turnId: TURN, item } });
    await json(await post(app, `${path}/interrupt`, { turnId: TURN }));
    const pending = await json(await app.request(path));
    assert.equal(pending.computerActivity[1].status, 'inProgress');
    await app.emit(turnDone(status));
    const done = await eventually(() => app.request(path).then(json), value => !value.activeTurn);
    assert.equal(done.computerActivity[0].status, 'completed');
    assert.equal(done.computerActivity[1].status, expected);
    assert.equal(done.approval, null);
  });
}

test('Computer Use on the Mac never reaches MiniClaw, the provider apps, settings, passwords or a terminal', async t => {
  const app = await start(t);
  // None of these is ever offered to the user: there is nothing to approve by mistake.
  for (const [index, name] of ['com.openai.codex', 'ChatGPT', 'com.anthropic.claudefordesktop', 'Claude',
    'com.apple.systempreferences', 'System Settings', 'com.apple.keychainaccess', 'Passwords',
    'com.apple.Terminal', 'iTerm', 'com.apple.ScriptEditor2', 'MiniClaw'].entries()) {
    const id = `protected-${index}`;
    await app.emit({ ...prompt({ _meta: { tool_params: { app: name } }, message: `Allow Computer Use to use ${name}?` }), id });
    assert.equal((await response(app, id)).action, 'decline', name);
    assert.equal((await app.request(path).then(json)).approval ?? null, null, `${name} is never asked about`);
  }
  // An ordinary app is still asked about, and a browser comes with a warning.
  await app.emit({ ...prompt({ _meta: { tool_params: { app: 'com.apple.Safari' } }, message: 'Allow Computer Use to use Safari?' }), id: 'browser-1' });
  const asked = await waiting(app);
  assert.equal(asked.app, 'com.apple.Safari');
  assert.match(asked.warning, /MiniClaw/);
});
