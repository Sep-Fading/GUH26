import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launchApp, json, events, eventually } from './support/harness.mjs';
import { THREAD, TURN, OTHER, CLIENT, signedOut, signedIn,
  delta, itemDone, turnDone, message } from './support/fixtures.mjs';

const post = (app, path, body = {}) =>
  app.request(path, { method: 'POST',
    body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });

test('conversation streams and completes', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  assert.equal((await json(await app.request('/api/account'))).status,
    'signed_in');
  assert.equal((await json(await post(app, '/api/threads'), 201))
    .thread.id, THREAD);
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  const body = { text: 'Hello', clientMessageId: CLIENT };
  const receipt = await json(await post(app,
    `/api/threads/${THREAD}/messages`, body), 202);
  assert.equal(receipt.turnId, TURN);
  await app.emit(delta('Hello'), delta(' there'), itemDone('Hello there'),
    turnDone());
  await stream.next('turn.completed');
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.activeTurn, null);
  assert.equal(snapshot.messages.at(-1).text, 'Hello there');
  assert.equal((await json(await app.request('/api/threads/working')))
    .replies[THREAD].turnId, TURN);
  assert.equal(stream.seen.filter(e => e.event === 'message.delta').length, 2);
  const requests = (await app.inspect()).requests;
  assert.deepEqual(requests.find(r => r.method === 'thread/start')
    .params.environments, []);
  assert.deepEqual(requests.find(r => r.method === 'turn/start')
    .params.environments, []);
  assert.equal(requests.some(r => r.method === 'account/login/start'), false);
});

test('early completion is not lost', async t => {
  const app = await launchApp({ 'turn/start': {
    result: { turn: { id: TURN, status: 'inProgress', items: [] } },
    before: [delta('Early'), itemDone('Early'), turnDone()],
  } });
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  }), 202);
  await eventually(async () => json(await app.request(
    `/api/threads/${THREAD}`)), value => value.activeTurn === null &&
      value.messages.at(-1)?.text === 'Early');
});

test('OAuth completes inside MiniClaw', async t => {
  const app = await launchApp({ 'account/read': { result: signedOut } });
  t.after(() => app.close());
  assert.equal((await json(await app.request('/api/account'))).status,
    'signed_out');
  const attempt = await json(await post(app, '/api/auth/start', {
    method: 'browser',
  }));
  assert.match(attempt.authUrl, /^https:\/\/auth\.openai\.com\//);
  await app.set('account/read', { result: signedIn });
  await app.emit({ method: 'account/login/completed', params: {
    loginId: 'login-fixture', success: true, error: null,
  } });
  await eventually(async () => json(await app.request('/api/auth/status')),
    value => value.status === 'succeeded');
  assert.equal((await json(await app.request('/api/account'))).status,
    'signed_in');
});

test('origin and unknown tasks are rejected', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  assert.equal((await app.request('/api/account', {
    headers: { Origin: 'https://evil.example' },
  })).status, 403);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  })).status, 404);
  assert.equal((await post(app, '/api/auth/logout')).status, 404);
});

test('Stop waits for completion', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  }), 202);
  await json(await post(app, `/api/threads/${THREAD}/interrupt`, {
    turnId: TURN,
  }));
  let snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.activeTurn.id, TURN);
  await app.emit(turnDone('interrupted'));
  snapshot = await eventually(async () => json(await app.request(
    `/api/threads/${THREAD}`)), value => value.activeTurn === null);
  assert.equal(snapshot.outcomeUnknown, false);
});

test('duplicate sends have one receipt', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  const body = { text: 'Send once', clientMessageId: CLIENT };
  for (let i = 0; i < 2; i++) {
    assert.equal((await json(await post(app,
      `/api/threads/${THREAD}/messages`, body), 202)).turnId, TURN);
  }
  const requests = (await app.inspect()).requests;
  assert.equal(requests.filter(r => r.method === 'turn/start').length, 1);
});

test('reconnecting replays missed text', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  }), 202);
  await app.emit(delta('Missed text'), itemDone('Missed text'), turnDone());
  const stream = await events(app, `/api/threads/${THREAD}/events`, {
    'Last-Event-ID': snapshot.eventCursor,
  });
  t.after(() => stream.close());
  await stream.next('turn.completed');
  assert.equal(stream.seen.find(e => e.event === 'message.delta')
    .data.delta, 'Missed text');
  const fresh = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => fresh.close());
  assert.equal((await fresh.next('snapshot')).data.messages.at(-1).text,
    'Missed text');
});

test('working list follows turn completion', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  assert.deepEqual((await json(await app.request('/api/threads/working'))).threads, []);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Work', clientMessageId: CLIENT,
  }), 202);
  assert.deepEqual((await json(await app.request('/api/threads/working'))).threads, [THREAD]);
  await app.emit(itemDone('Partial answer'), turnDone('failed'));
  await eventually(async () => (await json(await app.request('/api/threads/working'))).threads,
    threads => threads.length === 0);
  assert.deepEqual((await json(await app.request('/api/threads/working'))).replies, {});
});

// A failed Codex turn says why when the App Server gives a reason: `turn.error` on turn/completed,
// or a final (`willRetry: false`) `error` notification, each a TurnError { message, codexErrorInfo }.
test('a failed turn shows the provider reason, and the plain notice when there is none', async t => {
  const limit = 'You’ve hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Oct 11th, 2026 12:25 PM.';
  const cases = [
    ['usage limit on the turn', [], { message: limit, codexErrorInfo: 'usageLimitExceeded' },
      'Codex usage limit reached. Try again at Oct 11th, 2026 12:25 PM.'],
    ['usage limit without a retry time', [], { message: 'You’ve hit your usage limit.', codexErrorInfo: 'usageLimitExceeded' },
      'Codex usage limit reached.'],
    ['usage limit only in the error notification', [{ error: { message: limit, codexErrorInfo: 'usageLimitExceeded' }, willRetry: false }], undefined,
      'Codex usage limit reached. Try again at Oct 11th, 2026 12:25 PM.'],
    ['another provider reason', [], { message: 'The model is overloaded. Please try again later.', codexErrorInfo: 'serverOverloaded' },
      'Codex could not complete this response: The model is overloaded. Please try again later.'],
    ['a structured reason', [], { message: 'Stream disconnected before completion.', codexErrorInfo: { responseStreamDisconnected: { httpStatusCode: 502 } } },
      'Codex could not complete this response: Stream disconnected before completion.'],
    ['a reason naming an account', [], { message: 'Workspace owner person@example.test must approve this.', codexErrorInfo: 'unauthorized' },
      'Codex could not complete this response.'],
    ['a reason carrying an identifier', [], { message: 'Request req_8f3a9c2b7d6e5f4a3b2c1d0e failed.', codexErrorInfo: 'internalServerError' },
      'Codex could not complete this response.'],
    ['a retried error only', [{ error: { message: 'Reconnecting...', codexErrorInfo: { responseStreamDisconnected: { httpStatusCode: null } } }, willRetry: true }], undefined,
      'Codex could not complete this response.'],
    ['no reason at all', [], undefined, 'Codex could not complete this response.'],
  ];
  for (const [label, notices, error, expected] of cases) {
    const app = await launchApp();
    t.after(() => app.close());
    await json(await post(app, '/api/threads'), 201);
    const stream = await events(app, `/api/threads/${THREAD}/events`);
    t.after(() => stream.close());
    await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Hello', clientMessageId: CLIENT }), 202);
    for (const notice of notices) await app.emit({ method: 'error', params: { ...notice, threadId: THREAD, turnId: TURN } });
    await app.emit({ method: 'turn/completed', params: { threadId: THREAD,
      turn: { id: TURN, status: 'failed', items: [], ...(error ? { error } : {}) } } });
    const done = await stream.next('turn.completed');
    assert.deepEqual([done.data.status, done.data.error], ['failed', expected], label);
  }
});

// The page places a turn's Activity and delegation cards by the turn they belong to. A turn can end
// without a reply (Stop, a failure), so the user's message names the turn it started: on the live
// stream (`turn.started` carries the message id), in the snapshot, and in a fresh page's first snapshot.
test('a user message names the turn it started, so a turn without a reply keeps its place', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'First', clientMessageId: CLIENT }), 202);
  assert.equal((await stream.next('turn.started')).data.messageId, CLIENT);
  // One step, refused because it is malformed, then Stop before any reply.
  await app.emit({ id: 'ask-1', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD,
    turnId: TURN, callId: 'ask-1', arguments: {} } });
  const read = async () => json(await app.request(`/api/threads/${THREAD}`));
  await eventually(read, value => value.activities.some(entry => entry.id === 'ask-1'), 'activity');
  await app.emit(turnDone('interrupted'));
  await eventually(read, value => value.activeTurn === null, 'stopped');
  const next = { id: OTHER, status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn: next }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: next } }] });
  const second = crypto.randomUUID();
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Second', clientMessageId: second }), 202);
  await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: OTHER, item: message('Answer', 'assistant-2') } },
    { method: 'turn/completed', params: { threadId: THREAD, turn: { ...next, status: 'completed' } } });
  const snapshot = await eventually(read, value => value.activeTurn === null && value.messages.length === 3, 'second reply');
  const turns = [['user', 'First', TURN], ['user', 'Second', OTHER], ['assistant', 'Answer', OTHER]];
  assert.deepEqual(snapshot.messages.map(item => [item.role, item.text, item.turnId]), turns);
  assert.equal(snapshot.activities.find(entry => entry.id === 'ask-1').turnId, TURN);
  assert.equal(stream.seen.filter(event => event.event === 'turn.started')[1].data.messageId, second);
  const fresh = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => fresh.close());
  assert.deepEqual((await fresh.next('snapshot')).data.messages.map(item => [item.role, item.text, item.turnId]), turns);
});
