// Checks permission changes and preserved conversations.
import assert from 'node:assert/strict';
// Runs isolated tests with lifecycle cleanup.
import { test } from 'node:test';
// Checks recovery records without reading credentials.
import { access } from 'node:fs/promises';
// Starts disposable backends and inspects protocol traffic.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies fixed task IDs and completion events.
import { THREAD, OTHER, CLIENT, turnDone, thread } from './support/fixtures.mjs';
// Names a helper the backend accepts on this platform; it is never driven.
import { standInHelper } from './support/computer-helper.mjs';

process.env.MINICLAW_HELPER_BIN = standInHelper();
const path = `/api/threads/${THREAD}`;

// Posts a JSON body to the disposable backend.
const post = (app, route, body) => app.request(route, { method: 'POST', body });

// Returns an idle conversation with one completed prompt and fake lifecycle replies.
async function idle(t) {
  const app = await launchApp();
  t.after(() => app.close());
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  await app.set('thread/list', { result: { data: [], nextCursor: null } });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `${path}/messages`, {
    text: 'Keep this conversation', clientMessageId: CLIENT,
  }), 202);
  await app.emit(turnDone());
  await eventually(() => app.request(path).then(json), value => !value.activeTurn);
  return app;
}

test('computer toggle keeps the same conversation in both directions', async t => {
  const app = await idle(t);
  const before = await json(await app.request(path));
  for (const enabled of [true, false]) {
    const snapshot = await json(await post(app, `${path}/computer`, { enabled }));
    assert.equal(snapshot.thread.id, THREAD);
    assert.equal(snapshot.thread.computerUse, enabled);
    assert.deepEqual(snapshot.messages, before.messages);
    assert.equal(snapshot.pending, false);
    const resume = (await app.inspect()).requests
      .filter(request => request.method === 'thread/resume').at(-1).params;
    assert.equal(resume.config['mcp_servers.miniclaw_computer'].enabled, enabled);
    if (enabled) assert.equal(resume.config.mcp_servers, undefined,
      'the empty server-map override must not shadow the enabled helper');
    assert.equal(resume.approvalPolicy, enabled ? 'on-request' : 'never');
    assert.equal(resume.sandbox, 'read-only');
  }
  const requests = (await app.inspect()).requests;
  assert.equal(requests.filter(request => request.method === 'thread/start').length, 1);
  assert.equal(requests.filter(request => request.method === 'turn/start').length, 1);
});

test('a held switch preserves state and excludes sends', async t => {
  const app = await idle(t);
  await app.set('thread/unarchive', { hold: true });
  const switching = post(app, `${path}/computer`, { enabled: true });
  const state = await eventually(() => app.inspect(), value => value.held.length > 0);
  const snapshot = await json(await app.request(path));
  assert.equal(snapshot.computerChanging, true);
  assert.equal(snapshot.pending, false);
  assert.equal(snapshot.messages[0].text, 'Keep this conversation');
  assert.equal((await json(await app.request('/api/threads'))).threads[0].id, THREAD);
  assert.equal((await post(app, `${path}/messages`, {
    text: 'Too early', clientMessageId: crypto.randomUUID(),
  })).status, 409);
  assert.equal((await post(app, `${path}/computer`, { enabled: false })).status, 409);
  await app.peer.call({ op: 'release', id: state.held[0].id,
    reply: { result: { thread: { id: THREAD } } } });
  await json(await switching);
});

test('failed unarchive can be retried without archiving again', async t => {
  const app = await idle(t);
  await app.set('thread/unarchive', { error: { code: -32000, message: 'Fixture failure' } });
  const error = await json(await post(app, `${path}/computer`, { enabled: true }), 502);
  assert.match(error.error.message, /restore/i);
  const failed = await json(await app.request(path));
  assert.ok(failed.computerError);
  assert.equal(failed.computerChanging, false);
  await app.set('thread/list', { result: { data: [], nextCursor: null }, cases: [
    { params: { archived: true }, handler: { result: {
      data: [thread(app.peer.cwd)], nextCursor: null,
    } } },
  ] });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  const recovered = await json(await post(app, `${path}/computer`, { enabled: true }));
  assert.equal(recovered.computerError, null);
  assert.equal(recovered.thread.computerUse, true);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/archive').length, 1);
});

test('child tasks block archival before any changes', async t => {
  const app = await idle(t);
  const before = (await app.inspect()).requests.length;
  await app.set('thread/list', { result: { data: [{ id: OTHER }], nextCursor: null } });
  assert.equal((await post(app, `${path}/computer`, { enabled: true })).status, 409);
  const requests = (await app.inspect()).requests.slice(before);
  assert.equal(requests.some(r => r.method === 'thread/archive'), false);
  assert.equal(requests.find(r => r.method === 'thread/list').params.ancestorThreadId, THREAD);
});

test('account changes finish restoration without enabling tools', async t => {
  const app = await idle(t);
  await app.set('thread/archive', { hold: true });
  const switching = post(app, `${path}/computer`, { enabled: true });
  const state = await eventually(() => app.inspect(), value => value.held.length > 0);
  await app.emit({ method: 'account/updated', params: { authMode: null } });
  await app.peer.call({ op: 'release', id: state.held[0].id, reply: { result: {} } });
  assert.equal((await switching).status, 503);
  const requests = (await app.inspect()).requests;
  assert.ok(requests.some(r => r.method === 'thread/unarchive'));
  assert.equal(requests.some(r => r.method === 'thread/resume'), false);
});

test('closing the HTTP request does not abandon restoration', async t => {
  const app = await idle(t);
  await app.set('thread/unarchive', { hold: true });
  const controller = new AbortController();
  const switching = app.request(`${path}/computer`, {
    method: 'POST', body: { enabled: true }, signal: controller.signal,
  }).catch(() => null);
  const state = await eventually(() => app.inspect(), value => value.held.length > 0);
  controller.abort();
  await switching;
  await app.peer.call({ op: 'release', id: state.held[0].id,
    reply: { result: { thread: { id: THREAD } } } });
  await eventually(() => app.request(path).then(json), value =>
    value.thread.computerUse === true && value.computerChanging === false);
});

test('duplicate toggle requests do not reload twice', async t => {
  const app = await idle(t);
  await json(await post(app, `${path}/computer`, { enabled: true }));
  await json(await post(app, `${path}/computer`, { enabled: true }));
  const requests = (await app.inspect()).requests;
  assert.equal(requests.filter(request => request.method === 'thread/archive').length, 1);
});

test('active responses reject permission changes', async t => {
  const app = await idle(t);
  await app.emit({ method: 'turn/started', params: { threadId: THREAD, turn: { id: OTHER } } });
  await eventually(() => app.request(path).then(json), value => !!value.activeTurn);
  assert.equal((await post(app, `${path}/computer`, { enabled: true })).status, 409);
  assert.equal((await app.inspect()).requests.some(request =>
    request.method === 'thread/archive'), false);
});

test('malformed toggles cannot carry arbitrary runtime configuration', async t => {
  const app = await idle(t);
  for (const body of [{}, { enabled: 'yes' }, { enabled: true, config: {} }]) {
    assert.equal((await post(app, `${path}/computer`, body)).status, 400);
  }
});

test('failed reload blocks sends without discarding the transcript', async t => {
  const app = await idle(t);
  await app.set('thread/resume', { error: { code: -32000, message: 'Fixture failure' } });
  assert.equal((await post(app, `${path}/computer`, { enabled: true })).status, 502);
  const snapshot = await json(await app.request(path));
  assert.equal(snapshot.thread.computerUse, false);
  assert.equal(snapshot.messages[0].text, 'Keep this conversation');
  assert.equal((await post(app, `${path}/messages`, {
    text: 'Must not run', clientMessageId: crypto.randomUUID(),
  })).status, 409);
});

test('an account change during resume stops the old tool runtime', async t => {
  const app = await idle(t);
  await app.set('thread/resume', { hold: true });
  const switching = post(app, `${path}/computer`, { enabled: true });
  const state = await eventually(() => app.inspect(), value => value.held.length > 0);
  await app.emit({ method: 'account/updated', params: { authMode: null } });
  await app.peer.call({ op: 'release', id: state.held[0].id, reply: { result: {} } });
  assert.equal((await switching).status, 503);
  await eventually(() => app.request('/api/health').then(json),
    value => value.codex !== 'connected', 'old-account runtime shutdown');
});

test('a restart recovers a task left archived by a failed switch', async t => {
  const app = await idle(t);
  await app.set('thread/unarchive', { error: { code: -32000, message: 'Fixture failure' } });
  await json(await post(app, `${path}/computer`, { enabled: true }), 502);
  const marker = `${app.peer.dir}/computer-recovery/${THREAD}`;
  await access(marker);
  await app.restart();
  await app.set('thread/list', { result: { data: [], nextCursor: null }, cases: [
    { params: { archived: true }, handler: { result: {
      data: [thread(app.peer.cwd)], nextCursor: null,
    } } },
  ] });
  await app.set('thread/unarchive', { result: { thread: { id: OTHER } } });
  const error = await json(await app.request('/api/threads'), 503);
  assert.equal(error.error.code, 'computer_recovery');
  await access(marker);
  await app.set('thread/unarchive', { result: { thread: thread(app.peer.cwd) } });
  await json(await app.request('/api/threads'));
  assert.ok((await app.inspect()).requests.some(r => r.method === 'thread/unarchive'));
  await assert.rejects(access(marker), { code: 'ENOENT' });
});

test('retry checks whether restoration already happened before reloading', async t => {
  const app = await idle(t);
  await app.set('thread/unarchive', { error: { code: -32000, message: 'Lost acknowledgement' } });
  await json(await post(app, `${path}/computer`, { enabled: true }), 502);
  await app.set('thread/unarchive', { result: { thread: thread(app.peer.cwd) } });
  await json(await post(app, `${path}/computer`, { enabled: true }));
  const requests = (await app.inspect()).requests;
  assert.ok(requests.some(r => r.method === 'thread/list' && r.params.archived === true));
  assert.equal(requests.filter(r => r.method === 'thread/archive').length, 2);
});
