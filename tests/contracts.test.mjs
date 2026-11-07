import assert from 'node:assert/strict';
import { before, test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { launchApp, requireApplication, json, events, eventually } from './support/harness.mjs';
import { THREAD, TURN, OTHER, CLIENT, signedOut, signedIn, usage, window,
  thread, turn, delta, itemDone, turnDone, message } from './support/fixtures.mjs';

before(requireApplication);

async function appFor(t, overrides) {
  const app = await launchApp(overrides);
  t.after(() => app.close());
  return app;
}
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
const send = (app, text = 'Hello', id = CLIENT) => post(app, `/api/threads/${THREAD}/messages`, { text, clientMessageId: id });
async function activate(app) {
  await json(await post(app, `/api/threads/${THREAD}/environment`, { kind: 'host' }));
  await json(await post(app, `/api/threads/${THREAD}/activate`));
}
const calls = async (app, method) => (await app.inspect()).requests.filter(r => r.method === method);
async function snapshot(app) { return json(await app.request(`/api/threads/${THREAD}`)); }
async function settle(app, text, status = 'completed') {
  const items = [message(text)];
  const saved = thread(app.peer.cwd, { turns: [turn(status, items)] });
  await app.set('thread/read', { result: { thread: saved } });
  await app.set('thread/turns/list', { result: { data: [turn(status, items)], nextCursor: null } });
  await app.emit(itemDone(text), turnDone(status));
}
async function error(response, status) {
  const body = await json(response, status);
  assert.equal(typeof body.error.code, 'string');
  assert.equal(typeof body.error.message, 'string');
  assert.equal(typeof body.error.retryable, 'boolean');
  assert.equal(typeof body.error.requestId, 'string');
  return body.error;
}

test('PROTO-01: one connection initializes before account/task requests', async t => {
  const app = await appFor(t);
  const health = await json(await app.request('/api/health'));
  // The page words itself for the backend's operating system.
  assert.equal(health.platform, { win32: 'windows', linux: 'linux' }[process.platform] ?? 'macos');
  assert.equal(health.backend, 'ok');
  await json(await app.request('/api/account'));
  await json(await app.request('/api/threads'));
  const transcript = await app.inspect();
  assert.equal(transcript.requests.filter(x => x.method === 'initialize').length, 1);
  assert.equal(transcript.requests[0].method, 'initialize');
  assert.equal(transcript.requests[1].method, 'initialized');
  assert.equal(transcript.requests[0].params.clientInfo.name, 'miniclaw');
  // Empty environments require the installed App Server's experimental schema.
  assert.equal(transcript.requests[0].params.capabilities?.experimentalApi, true);
});

test('PROTO-02: reversed RPC replies still reach the correct HTTP caller', async t => {
  const app = await appFor(t);
  await app.set('thread/list', { hold: true });
  const tasks = app.request('/api/threads');
  // Let authorization finish before holding the independent account request.
  await eventually(() => app.inspect(), s => s.held.some(x => x.method === 'thread/list'),
    'authorized task listing');
  await app.set('account/read', { hold: true });
  const account = app.request('/api/account');
  const held = await eventually(() => app.inspect(), s => s.held.some(x => x.method === 'account/read') && s.held.some(x => x.method === 'thread/list'), 'both RPC reads');
  for (const request of held.held.filter(x => x.method === 'account/read')) {
    await app.peer.call({ op: 'release', id: request.id, reply: { result: signedIn } });
  }
  for (const request of held.held.filter(x => x.method === 'thread/list')) {
    await app.peer.call({ op: 'release', id: request.id, reply: { result: { data: [], nextCursor: null } } });
  }
  assert.deepEqual((await json(await tasks)).threads, []);
  assert.equal((await json(await account)).email, 'reader@example.test');
});

test('AUTH-01: signed-out account is distinct from unavailable account', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  const response = await app.request('/api/account');
  assert.match(response.headers.get('cache-control') ?? '', /no-store/);
  const body = await json(response);
  assert.equal(body.status, 'signed_out');
  assert.equal(body.requiresOpenaiAuth, true);
});

test('AUTH-02: every protected route rejects a signed-out client', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  for (const path of ['/api/usage', '/api/threads', `/api/threads/${THREAD}`, `/api/threads/${THREAD}/history`, `/api/threads/${THREAD}/events`]) {
    await error(await app.request(path), 401);
  }
  for (const path of ['/api/threads', `/api/threads/${THREAD}/activate`, `/api/threads/${THREAD}/messages`, `/api/threads/${THREAD}/interrupt`]) {
    await error(await post(app, path), 401);
  }
  assert.equal((await calls(app, 'thread/start')).length, 0);
  assert.equal((await calls(app, 'turn/start')).length, 0);
});

test('AUTH-03: account DTO preserves an unfamiliar plan and removes extra secret fields', async t => {
  const app = await appFor(t, { 'account/read': { result: {
    ...signedIn, account: { ...signedIn.account, planType: 'future_plan', accessToken: 'TEST_ONLY_SECRET' },
  } } });
  const body = await json(await app.request('/api/account'));
  assert.equal(body.status, 'signed_in');
  assert.equal(body.planType, 'future_plan');
  assert.ok(!JSON.stringify(body).includes('TEST_ONLY_SECRET'));
  assert.ok(!app.logs().includes('TEST_ONLY_SECRET'));
});

test('AUTH-04: API-key auth is shown as unsupported, not ChatGPT sign-in', async t => {
  const app = await appFor(t, { 'account/read': { result: { account: { type: 'apiKey' }, requiresOpenaiAuth: true } } });
  assert.equal((await json(await app.request('/api/account'))).status, 'unsupported_auth');
  await error(await post(app, '/api/threads'), 401);
});

test('AUTH-05: double-clicking sign-in creates only one pending login', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  const [a, b] = await Promise.all([post(app, '/api/auth/start', { method: 'browser' }), post(app, '/api/auth/start', { method: 'browser' })]);
  const first = await json(a);
  const second = await json(b);
  assert.equal(first.attemptId, second.attemptId);
  assert.equal(typeof first.attemptId, 'string');
  assert.match(first.authUrl, /^https:\/\//);
  assert.equal((await calls(app, 'account/login/start')).length, 1);
  assert.equal((await calls(app, 'account/login/start'))[0].params.type, 'chatgpt');
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'pending');
});

test('AUTH-06: a completion notification refreshes account state', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  await json(await post(app, '/api/auth/start', { method: 'browser' }));
  await app.set('account/read', { result: signedIn });
  await app.emit({ method: 'account/login/completed', params: { loginId: 'login-fixture', success: true, error: null } },
    { method: 'account/updated', params: { authMode: 'chatgpt', planType: 'plus' } });
  await eventually(async () => json(await app.request('/api/auth/status')), s => s.status === 'succeeded', 'login completion');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
});

test('AUTH-07: device login returns a code without accepting browser-supplied credentials', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut }, 'account/login/start': {
    result: { type: 'chatgptDeviceCode', loginId: 'device-fixture', verificationUrl: 'https://auth.openai.com/codex/device', userCode: 'TEST-CODE' },
  } });
  const body = await json(await post(app, '/api/auth/start', { method: 'device' }));
  assert.equal(body.userCode, 'TEST-CODE');
  assert.match(body.verificationUrl, /^https:\/\//);
  assert.equal((await calls(app, 'account/login/start'))[0].params.type, 'chatgptDeviceCode');
  await error(await post(app, '/api/auth/start', { method: 'apiKey', apiKey: 'TEST_ONLY_SECRET' }), 400);
});

test('AUTH-08: cancel maps the local attempt to the upstream login ID', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  await json(await post(app, '/api/auth/start', { method: 'browser' }));
  await json(await post(app, '/api/auth/cancel'));
  assert.equal((await calls(app, 'account/login/cancel'))[0].params.loginId, 'login-fixture');
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'cancelled');
});

test('AUTH-14: expired device login can retry and ignores the old completion', async t => {
  // Returns a synthetic device response for one upstream login attempt.
  const device = id => ({ result: { type: 'chatgptDeviceCode', loginId: id,
    verificationUrl: 'https://auth.openai.com/codex/device', userCode: 'TEST-CODE' } });
  const app = await appFor(t, {
    'account/read': { result: signedOut },
    'account/login/start': device('device-old'),
  });
  const first = await json(await post(app, '/api/auth/start', { method: 'device' }));
  await app.emit({ method: 'account/login/completed', params: {
    loginId: 'device-old', success: false, error: 'Device code expired',
  } });
  await eventually(async () => json(await app.request('/api/auth/status')),
    status => status.status === 'failed', 'expired device login');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');

  await app.set('account/login/start', device('device-new'));
  const retry = await json(await post(app, '/api/auth/start', { method: 'device' }));
  assert.notEqual(retry.attemptId, first.attemptId);
  assert.equal(retry.status, 'pending');
  await app.emit({ method: 'account/login/completed', params: {
    loginId: 'device-old', success: true, error: null,
  } });
  assert.equal((await json(await app.request('/api/auth/status'))).status, 'pending');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
  await app.set('account/read', { result: signedIn });
  await app.emit({ method: 'account/login/completed', params: {
    loginId: 'device-new', success: true, error: null,
  } });
  await eventually(async () => json(await app.request('/api/auth/status')),
    status => status.status === 'succeeded', 'retried device login');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
  assert.equal((await calls(app, 'account/login/start')).length, 2);
});

test('AUTH-09: external Codex sign-out clears UI-facing data and closes a task feed', async t => {
  const app = await appFor(t);
  await activate(app);
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  await app.set('account/read', { result: signedOut });
  await app.emit({ method: 'account/updated', params: { authMode: null, planType: null } });
  await stream.waitClosed();
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
  await error(await app.request('/api/threads'), 401);
  await error(await app.request('/api/usage'), 401);
  assert.equal((await calls(app, 'account/logout')).length, 0);
});

test('AUTH-10: MiniClaw exposes no logout that can clear shared Codex credentials', async t => {
  const app = await appFor(t);
  await error(await post(app, '/api/auth/logout'), 404);
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
  assert.equal((await calls(app, 'account/logout')).length, 0);
});

test('AUTH-12: an existing Codex session works immediately in the inherited Codex home', async t => {
  const app = await appFor(t);
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
  await json(await app.request('/api/usage'));
  await json(await post(app, '/api/threads'), 201);
  assert.equal((await app.inspect()).codexHome, app.peer.codexHome);
  assert.equal((await calls(app, 'account/login/start')).length, 0);
  assert.equal((await calls(app, 'account/logout')).length, 0);
});

test('AUTH-13: fallback OAuth cannot replace an already authenticated Codex session', async t => {
  const app = await appFor(t);
  await error(await post(app, '/api/auth/start', { method: 'browser' }), 409);
  assert.equal((await calls(app, 'account/login/start')).length, 0);
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_in');
});

const usageCases = [
  ['USAGE-01: legacy bucket remains usable', { rateLimits: usage.rateLimits }, body => {
    assert.equal(body.buckets.length, 1); assert.equal(body.buckets[0].primary.usedPercent, 25);
    assert.equal(body.buckets[0].primary.resetsAt, 2000000000);
  }],
  ['USAGE-02: multiple buckets replace rather than duplicate legacy data', {
    ...usage, rateLimitsByLimitId: { codex: usage.rateLimits, fast: { limitId: 'fast', primary: { ...window, usedPercent: 80 } } },
  }, body => { assert.deepEqual(body.buckets.map(x => x.limitId).sort(), ['codex', 'fast']); }],
  ['USAGE-03: an explicitly empty bucket map stays empty', { ...usage, rateLimitsByLimitId: {} }, body => {
    assert.deepEqual(body.buckets, []);
  }],
  ['USAGE-04: missing limits remain unknown instead of zero', { rateLimits: {}, rateLimitsByLimitId: null }, body => {
    assert.equal(body.buckets[0].primary, null); assert.equal(body.buckets[0].secondary, null);
    assert.equal(body.ordinaryUsageAllowed, null);
  }],
  ['USAGE-05: a reset time in the past does not override denied access', {
    rateLimits: { limitId: 'codex', primary: { ...window, usedPercent: 5, resetsAt: 1 } }, ordinaryUsageAllowed: false,
  }, body => { assert.equal(body.ordinaryUsageAllowed, false); }],
  ['USAGE-06: keep the raw percentage for presentation to clamp', {
    rateLimits: { limitId: 'codex', primary: { usedPercent: 120, resetsAt: null, windowDurationMins: null } },
  }, body => { assert.equal(body.buckets[0].primary.usedPercent, 120); assert.equal(body.buckets[0].primary.resetsAt, null); }],
];
for (const [name, fixture, check] of usageCases) test(name, async t => {
  const app = await appFor(t, { 'account/rateLimits/read': { result: fixture } });
  const body = await json(await app.request('/api/usage'));
  assert.equal(body.status, 'fresh');
  assert.ok(Number.isFinite(Date.parse(body.fetchedAt)));
  check(body);
});

test('TASK-01: list returns task summaries and an opaque cursor', async t => {
  const app = await appFor(t);
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: 'opaque+/=cursor' } });
  const body = await json(await app.request('/api/threads'));
  assert.equal(body.threads[0].id, THREAD);
  assert.equal(body.nextCursor, 'opaque+/=cursor');
  await json(await app.request('/api/threads?cursor=opaque%2B%2F%3Dcursor'));
  assert.equal((await calls(app, 'thread/list')).at(-1).params.cursor, 'opaque+/=cursor');
});

test('TASK-02: new task uses the controlled workspace and read-only policy', async t => {
  const app = await appFor(t);
  assert.equal((await json(await post(app, '/api/threads'), 201)).thread.id, THREAD);
  const request = (await calls(app, 'thread/start'))[0];
  assert.equal(request.params.cwd, app.peer.cwd);
  assert.equal(request.params.sandbox, 'read-only');
  assert.equal(request.params.approvalPolicy, 'never');
  assert.equal(request.params.model, undefined);
});

test('TASK-03: a task outside MiniClaw workspace cannot be read or resumed', async t => {
  const app = await appFor(t, { 'thread/read': { result: { thread: thread('/unrelated/workspace', { id: OTHER }) } } });
  await error(await app.request(`/api/threads/${OTHER}`), 404);
  await error(await post(app, `/api/threads/${OTHER}/activate`), 404);
  assert.equal((await calls(app, 'thread/resume')).length, 0);
});

test('CHAT-01: accepted text and client ID reach Codex unchanged', async t => {
  const app = await appFor(t);
  await activate(app);
  const receipt = await json(await send(app, 'Hello\nworld'), 202);
  assert.equal(receipt.turnId, TURN);
  const request = (await calls(app, 'turn/start'))[0];
  assert.equal(request.params.clientUserMessageId, CLIENT);
  assert.equal(request.params.threadId, THREAD);
  assert.equal(request.params.input[0].type, 'text');
  assert.equal(request.params.input[0].text, 'Hello\nworld');
});

for (const [name, text, status] of [
  ['CHAT-02: blank text is rejected', ' \n\t', 400],
  ['CHAT-03: text limit is UTF-8 bytes, not JavaScript characters', '\u00e9'.repeat(16385), 413],
]) test(name, async t => {
  const app = await appFor(t); await activate(app);
  await error(await send(app, text), status);
  assert.equal((await calls(app, 'turn/start')).length, 0);
});

test('CHAT-04: two simultaneous sends cannot start two turns', async t => {
  const app = await appFor(t); await activate(app);
  const results = await Promise.all([send(app), send(app, 'Second', randomUUID())]);
  assert.deepEqual(results.map(r => r.status).sort(), [202, 409]);
  assert.equal((await calls(app, 'turn/start')).length, 1);
});

test('CHAT-05: retrying the same client message returns the original receipt', async t => {
  const app = await appFor(t); await activate(app);
  const first = await json(await send(app), 202);
  const second = await json(await send(app), 202);
  assert.equal(first.turnId, second.turnId);
  assert.equal((await calls(app, 'turn/start')).length, 1);
  await error(await send(app, 'Different text with reused ID'), 409);
});

test('STREAM-01: text arriving before the send reply is retained', async t => {
  const app = await appFor(t); await activate(app);
  await app.set('turn/start', { before: [delta('Early text')], result: { turn: turn() } });
  await json(await send(app), 202);
  await eventually(() => snapshot(app), s => s.messages.some(m => m.text === 'Early text'), 'early assistant text');
});

test('STREAM-02: separate items stay separate and final text replaces partial text', async t => {
  const app = await appFor(t); await activate(app);
  await json(await send(app), 202);
  await app.emit(delta('Draft'), delta('Other answer', 'assistant-2'), itemDone('Final answer'));
  const snap = await eventually(() => snapshot(app), s => s.messages.some(m => m.id === 'assistant-1' && m.text === 'Final answer'), 'canonical message');
  assert.equal(snap.messages.find(m => m.id === 'assistant-2').text, 'Other answer');
  assert.ok(!snap.messages.some(m => m.text === 'DraftFinal answer'));
});

test('STREAM-03: replay after a cursor recovers the gap without the previous delta', async t => {
  const app = await appFor(t); await activate(app);
  const first = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => first.close());
  await json(await send(app), 202); await app.emit(delta('A'));
  const seen = await first.next('message.delta');
  assert.ok(seen.id);
  assert.equal(seen.data.delta, 'A');
  await first.close();
  await app.emit(delta('B'), itemDone('AB'), turnDone());
  const resumed = await events(app, `/api/threads/${THREAD}/events`, { 'Last-Event-ID': seen.id });
  t.after(() => resumed.close());
  await resumed.next('turn.completed');
  assert.deepEqual(resumed.seen.filter(e => e.event === 'message.delta').map(e => e.data.delta), ['B']);
  assert.ok(resumed.seen.every(e => e.id !== seen.id));
});

test('STREAM-04: an unknown event epoch asks for a fresh snapshot', async t => {
  const app = await appFor(t); await activate(app);
  const stream = await events(app, `/api/threads/${THREAD}/events?after=old-backend%3A9`);
  t.after(() => stream.close());
  await stream.next('resync.required');
  const snap = await snapshot(app);
  assert.equal(typeof snap.eventCursor, 'string');
  assert.notEqual(snap.eventCursor, 'old-backend:9');
});

test('STREAM-05: interrupt acknowledgement does not finish the active turn', async t => {
  const app = await appFor(t); await activate(app); await json(await send(app), 202);
  await app.emit(delta('Partial answer'));
  await json(await post(app, `/api/threads/${THREAD}/interrupt`, { turnId: TURN }));
  assert.equal((await calls(app, 'turn/interrupt'))[0].params.turnId, TURN);
  assert.equal((await snapshot(app)).activeTurn.id, TURN);
  await settle(app, 'Partial answer', 'interrupted');
  const done = await eventually(() => snapshot(app), s => s.activeTurn === null, 'interrupted completion');
  assert.ok(done.messages.some(m => m.text === 'Partial answer'));
});

test('STREAM-06: unrelated task events cannot enter the selected task feed', async t => {
  const app = await appFor(t); await activate(app); await json(await send(app), 202);
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  await app.emit(delta('PRIVATE_OTHER_TASK', 'other-item', OTHER), delta('Own text'), itemDone('Own text'), turnDone());
  await stream.next('turn.completed');
  assert.ok(!JSON.stringify(stream.seen).includes('PRIVATE_OTHER_TASK'));
});

test('SEC-01: an unrelated website cannot invoke fallback OAuth or sends', async t => {
  const app = await appFor(t);
  for (const path of ['/api/auth/start', '/api/threads', `/api/threads/${THREAD}/messages`]) {
    await error(await app.request(path, { method: 'POST', body: {}, headers: { Origin: 'https://unrelated.example', 'Sec-Fetch-Site': 'cross-site' } }), 403);
  }
  for (const method of ['account/login/start', 'account/logout', 'thread/start', 'turn/start']) assert.equal((await calls(app, method)).length, 0);
});

test('SEC-02: hostile Host header is rejected', async t => {
  const app = await appFor(t);
  // Node fetch replaces Host, so use HTTP directly to exercise the guard.
  const { get } = await import('node:http');
  const status = await new Promise((resolve, reject) => {
    const request = get(`${app.url}/api/account`, {
      headers: { Host: 'attacker.example' },
    }, response => { response.resume(); resolve(response.statusCode); });
    request.on('error', reject);
  });
  assert.ok([400, 403, 421].includes(status));
});

test('SEC-03: browser-supplied configuration never reaches thread/start', async t => {
  const app = await appFor(t);
  await error(await post(app, '/api/threads', { cwd: '/private', sandbox: 'danger-full-access' }), 400);
  assert.equal((await calls(app, 'thread/start')).length, 0);
});

test('SEC-04: unknown API routes are JSON errors, not the frontend HTML fallback', async t => {
  const app = await appFor(t);
  await error(await app.request('/api/does-not-exist'), 404);
  await error(await post(app, '/api/rpc', { method: 'command/exec', params: { command: 'echo forbidden' } }), 404);
});

test('SEC-05: unknown server requests get a protocol error instead of hanging', async t => {
  const app = await appFor(t);
  await app.emit({ id: 'server-request-fixture', method: 'future/unknownRequest', params: {} });
  const transcript = await eventually(() => app.inspect(), s => s.responses.some(r => r.id === 'server-request-fixture'), 'server request refusal');
  const reply = transcript.responses.find(r => r.id === 'server-request-fixture');
  assert.equal(reply.error.code, -32601);
});

test('SEC-07: unsupported server requests receive schema-shaped refusals', async t => {
  const app = await appFor(t);
  const requests = [
    ['item/commandExecution/requestApproval', { decision: 'decline' }],
    ['item/fileChange/requestApproval', { decision: 'decline' }],
    ['item/permissions/requestApproval', { permissions: {}, scope: 'turn' }],
    ['item/tool/requestUserInput', { answers: {} }],
    ['item/tool/call', { success: false, contentItems: [] }],
  ];
  await app.emit(...requests.map(([method], index) => ({
    id: `refusal-${index}`, method, params: {},
  })));
  const transcript = await eventually(() => app.inspect(), state =>
    requests.every((_, index) => state.responses.some(reply => reply.id === `refusal-${index}`)),
  'unsupported request refusals');
  for (const [index, [, expected]] of requests.entries()) {
    const reply = transcript.responses.find(response => response.id === `refusal-${index}`);
    assert.deepEqual(reply.result, expected);
    assert.equal(reply.error, undefined);
  }
});

test('RECOVERY-01: a missing send acknowledgement reports uncertainty without resending', { timeout: 45000 }, async t => {
  const app = await appFor(t); await activate(app);
  await app.set('turn/start', { hold: true });
  const response = await app.request(`/api/threads/${THREAD}/messages`, {
    method: 'POST', body: { text: 'Only send once', clientMessageId: CLIENT },
    signal: AbortSignal.timeout(35000),
  });
  assert.equal((await error(response, 504)).outcomeUnknown, true);
  assert.equal((await calls(app, 'turn/start')).length, 1);
});

test('USAGE-07: a failed refresh keeps the last known usage marked stale', async t => {
  const app = await appFor(t);
  const initial = await json(await app.request('/api/usage'));
  await app.set('account/rateLimits/read', { error: { code: -32000, message: 'Usage temporarily unavailable' } });
  const refreshed = await json(await app.request('/api/usage'));
  assert.equal(refreshed.status, 'stale');
  assert.deepEqual(refreshed.buckets, initial.buckets);
  assert.equal(refreshed.fetchedAt, initial.fetchedAt);
});

test('AUTH-11: failed login completion does not authenticate the user', async t => {
  const app = await appFor(t, { 'account/read': { result: signedOut } });
  await json(await post(app, '/api/auth/start', { method: 'browser' }));
  await app.emit({ method: 'account/login/completed', params: { loginId: 'login-fixture', success: false, error: 'Login denied' } });
  await eventually(async () => json(await app.request('/api/auth/status')), s => s.status === 'failed', 'failed login');
  assert.equal((await json(await app.request('/api/account'))).status, 'signed_out');
});

test('SEC-06: JSON mutations without the custom request header are rejected', async t => {
  const app = await appFor(t);
  const response = await fetch(`${app.url}/api/threads`, {
    method: 'POST', headers: { Origin: app.url, 'Content-Type': 'application/json' },
    body: '{}', signal: AbortSignal.timeout(4000),
  });
  await error(response, 403);
  assert.equal((await calls(app, 'thread/start')).length, 0);
});
