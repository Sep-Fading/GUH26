import assert from 'node:assert/strict';
import { test } from 'node:test';
import { writeFile } from 'node:fs/promises';
import { join, toNamespacedPath } from 'node:path';
import { launchApp, json, eventually, events } from './support/harness.mjs';
import { THREAD, OTHER, CLIENT, TURN, defaults, thread, started, turn,
  message, delta, itemDone, turnDone, signedOut } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
const activate = app => post(app, `/api/threads/${THREAD}/activate`);
const user = (text, id = 'user-1') => ({ id, type: 'userMessage',
  content: [{ type: 'text', text, text_elements: [] }], clientId: CLIENT });
async function appFor(t, overrides) {
  const app = await launchApp(overrides);
  t.after(() => app.close());
  return app;
}
const calls = async (app, method) =>
  (await app.inspect()).requests.filter(r => r.method === method);

test('list checks ownership and passes cursors', async t => {
  const app = await appFor(t);
  await app.set('thread/list', { result: { data: [
    thread('/foreign', { id: OTHER, preview: 'PRIVATE_OTHER_TASK' }),
    thread(app.peer.cwd, { modelProvider: 'custom', id: OTHER }),
    thread(app.peer.cwd, { model: 'other-model', reasoningEffort: 'ultra',
      path: '/private/transcript', extra: { token: 'TEST_ONLY_SECRET' } }),
  ], nextCursor: 'next+/=' } });
  const list = await json(await app.request('/api/threads'));
  assert.deepEqual(list.threads.map(task => task.id), [THREAD]);
  assert.equal(list.threads[0].model, 'other-model');
  assert.equal(list.threads[0].effort, 'ultra');
  assert.equal(list.nextCursor, 'next+/=');
  assert.doesNotMatch(JSON.stringify(list), /PRIVATE_OTHER_TASK|TEST_ONLY_SECRET|\/private/);
  await json(await app.request('/api/threads?cursor=next%2B%2F%3D'));
  const params = (await calls(app, 'thread/list')).at(-1).params;
  assert.deepEqual(params.cwd, spellings(app.peer.cwd));
  assert.deepEqual(params.modelProviders, ['openai']);
  assert.ok(params.sourceKinds.includes('appServer'));
  assert.ok(params.sourceKinds.includes('vscode'));
  assert.equal(params.archived, false);
  assert.equal(params.cursor, 'next+/=');
  assert.ok(params.limit <= 50);
  assert.equal((await calls(app, 'thread/resume')).length, 0);
});

// The backend's folder on Windows is the plain drive form (2026-10-04); before, it was the verbatim
// `\\?\C:\...` form. Codex's `thread/list` takes several exact folders, so both are asked for there,
// and a macOS folder is sent alone as before.
const spellings = cwd => process.platform === 'win32' ? [cwd, toNamespacedPath(cwd)] : cwd;

test('a chat saved under the verbatim Windows folder still belongs to the workspace', async t => {
  const app = await appFor(t);
  const verbatim = toNamespacedPath(app.peer.cwd);
  assert.equal(process.platform !== 'win32' || verbatim.startsWith('\\\\?\\') && !app.peer.cwd.startsWith('\\\\'), true);
  await app.set('thread/list', { result: { data: [
    thread(verbatim, { id: OTHER, name: 'Saved before the folder change' }),
    thread(app.peer.cwd, { name: 'Saved after it' }),
  ], nextCursor: null } });
  const list = await json(await app.request('/api/threads'));
  assert.deepEqual(list.threads.map(task => task.id).sort(), [THREAD, OTHER].sort());
  await app.set('thread/read', { result: { thread: thread(verbatim, { id: OTHER }) } });
  await app.set('thread/resume', { result: started(verbatim, { id: OTHER }) });
  await json(await post(app, `/api/threads/${OTHER}/activate`));
  const resume = (await calls(app, 'thread/resume')).at(-1).params;
  assert.equal(resume.threadId, OTHER);
  assert.equal(resume.cwd, app.peer.cwd, 'a resumed chat works in the plain folder');
});

test('saved chats return after restart', async t => {
  const app = await appFor(t);
  await json(await post(app, '/api/threads'), 201);
  const saved = { ...defaults(app.peer.cwd),
    'thread/list': { result: { data: [thread(app.peer.cwd)], nextCursor: null } },
    'thread/read': { result: { thread: thread(app.peer.cwd) } },
    'thread/resume': { result: { ...started(app.peer.cwd),
      model: 'other-model', reasoningEffort: 'ultra' } },
    'thread/turns/list': { result: { data: [turn('completed', [
      user('A saved question'), message('A saved answer'),
    ])], nextCursor: null } },
  };
  // This file is the fake peer's saved history, not MiniClaw application state.
  await writeFile(join(app.peer.dir, 'scenario.json'), JSON.stringify({ handlers: saved }));
  const home = app.peer.codexHome;
  await app.restart();
  assert.equal((await app.inspect()).codexHome, home);
  assert.equal((await json(await app.request('/api/threads'))).threads[0].id, THREAD);
  await json(await activate(app));
  const restored = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(restored.messages.map(m => m.text), ['A saved question', 'A saved answer']);
  assert.equal(restored.thread.model, 'other-model');
  assert.equal(restored.thread.effort, 'ultra');
  assert.equal(restored.activeTurn, null);
  assert.equal((await calls(app, 'turn/start')).length, 0);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Continue', clientMessageId: '019a5555-5555-7555-8555-555555555555',
    model: 'other-model', effort: 'ultra',
  }), 202);
  assert.equal((await calls(app, 'turn/start')).length, 1);
  const params = (await calls(app, 'thread/resume'))[0].params;
  assert.equal(params.cwd, app.peer.cwd);
  assert.equal(params.excludeTurns, true);
  assert.equal(params.approvalPolicy, 'never');
  assert.equal(params.sandbox, 'read-only');
  assert.deepEqual(params.runtimeWorkspaceRoots, []);
});

test('opening history does not start a response', async t => {
  const app = await appFor(t);
  const recent = turn('completed', [user('Question'), message('Answer'),
    { id: 'tool', type: 'mcpToolCall', result: { secret: 'TEST_ONLY_SECRET' } }]);
  await app.set('thread/turns/list', { result: { data: [recent], nextCursor: 'older' } });
  const current = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(current.messages.map(m => m.text), ['Question', 'Answer']);
  assert.equal(current.historyCursor, 'older');
  assert.doesNotMatch(JSON.stringify(current), /TEST_ONLY_SECRET/);
  assert.equal((await calls(app, 'turn/start')).length, 0);
  await app.set('thread/turns/list', { result: { data: [
    { ...turn('completed', [message('Older answer', 'older-message')]), id: 'older-turn' },
  ], nextCursor: null } });
  const older = await json(await app.request(`/api/threads/${THREAD}/history?cursor=older`));
  assert.deepEqual(older.messages.map(m => m.text), ['Older answer']);
  assert.equal(older.nextCursor, null);
  const params = (await calls(app, 'thread/turns/list')).at(-1).params;
  assert.equal(params.cursor, 'older');
  assert.equal(params.itemsView, 'notLoaded', 'turns are listed without their items');
  assert.equal(params.sortDirection, 'desc');
});

// A conversation whose saved tool results run to tens of megabytes used to come back in one
// reply too large for the connection, which then ended and took Codex down with it. Each turn's
// items are now read in small pages; the conversation shows every message.
test('a turn without items is read page by page', async t => {
  const app = await appFor(t);
  const entry = item => ({ turnId: TURN, item, startedAtMs: 1, completedAtMs: 2 });
  await app.set('thread/turns/list', { result: { data: [
    { ...turn('completed'), itemsView: 'notLoaded' },
  ], nextCursor: null } });
  await app.set('thread/items/list', {
    result: { data: [entry(user('Question')),
      entry({ id: 'tool', type: 'dynamicToolCall', output: 'TEST_ONLY_SECRET' })], nextCursor: 'rest' },
    cases: [{ params: { cursor: 'rest' }, handler: { result: {
      data: [entry(message('Answer'))], nextCursor: null } } }],
  });
  const current = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(current.messages.map(m => m.text), ['Question', 'Answer']);
  assert.doesNotMatch(JSON.stringify(current), /TEST_ONLY_SECRET/);
  const pages = await calls(app, 'thread/items/list');
  assert.equal(pages.length, 2);
  assert.deepEqual(pages.map(p => p.params.cursor), [null, 'rest']);
  for (const page of pages) {
    assert.equal(page.params.threadId, THREAD);
    assert.equal(page.params.turnId, TURN);
    assert.equal(page.params.limit, 6, 'a page is small enough for any saved tool result');
    assert.equal(page.params.sortDirection, 'asc');
  }
});

test('a turn that arrives with its items is read as given', async t => {
  const app = await appFor(t);
  await app.set('thread/turns/list', { result: { data: [
    { ...turn('completed', [user('Q'), message('A')]), itemsView: 'full' },
  ], nextCursor: null } });
  const current = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(current.messages.map(m => m.text), ['Q', 'A']);
  assert.equal((await calls(app, 'thread/items/list')).length, 0);
});

test('activation is coalesced and keeps early completion', async t => {
  const app = await appFor(t);
  await app.set('thread/resume', { hold: true });
  const first = activate(app);
  const held = await eventually(() => app.inspect(),
    s => s.held.some(r => r.method === 'thread/resume'), 'resume pending');
  const second = activate(app);
  await app.emit(delta('Draft'), itemDone('Canonical'), turnDone());
  await app.peer.call({ op: 'release', id: held.held[0].id,
    reply: { result: started(app.peer.cwd) } });
  await json(await first);
  await json(await second);
  assert.equal((await calls(app, 'thread/resume')).length, 1);
  const current = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(current.messages.at(-1).text, 'Canonical');
  assert.equal(current.activeTurn, null);
});

test('foreign and malformed saved tasks cannot resume', async t => {
  const app = await appFor(t);
  for (const value of [thread('/foreign'), thread(app.peer.cwd, { id: OTHER }),
    thread(app.peer.cwd, { modelProvider: 'custom' })]) {
    await app.set('thread/read', { result: { thread: value } });
    assert.equal((await activate(app)).status, 404);
  }
  assert.equal((await calls(app, 'thread/resume')).length, 0);
});

test('account changes discard pending history', async t => {
  const app = await appFor(t);
  await app.set('thread/resume', { hold: true });
  const loading = activate(app);
  const held = await eventually(() => app.inspect(),
    s => s.held.some(r => r.method === 'thread/resume'), 'resume pending');
  await app.set('account/read', { result: signedOut });
  await app.emit({ method: 'account/updated', params: { authMode: null } });
  await app.peer.call({ op: 'release', id: held.held[0].id,
    reply: { result: started(app.peer.cwd) } });
  assert.notEqual((await loading).status, 200);
  assert.equal((await app.request(`/api/threads/${THREAD}`)).status, 401);
});

test('a reconnect loads an unopened saved task', async t => {
  const app = await appFor(t);
  const stream = await events(app, `/api/threads/${THREAD}/events?after=previous:12`);
  t.after(() => stream.close());
  await stream.next('resync.required');
  const snapshot = await stream.next('snapshot');
  assert.equal(snapshot.data.thread.id, THREAD);
  assert.equal((await calls(app, 'thread/resume')).length, 1);
  assert.equal((await calls(app, 'turn/start')).length, 0);
});

test('a conversation that is still responding opens, and its partial text is never published twice', async t => {
  // Requirement change, 2026-09-21: this used to answer 409 until the response ended.
  const app = await appFor(t);
  await app.set('thread/turns/list', { hold: true });
  const opening = activate(app);
  const held = await eventually(() => app.inspect(),
    s => s.held.some(r => r.method === 'thread/turns/list'), 'history pending');
  // The same words arrive twice: once as a live delta while loading, once in the saved history.
  await app.emit(delta('Partial'));
  await app.peer.call({ op: 'release', id: held.held[0].id, reply: { result: {
    data: [turn('inProgress', [message('Partial')])], nextCursor: null,
  } } });
  assert.equal((await opening).status, 200);
  const open = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(open.messages.map(m => [m.text, m.complete]), [['Partial', false]], 'once, not "PartialPartial"');
  assert.ok(open.activeTurn, 'the turn is shown as running');
  // Later deltas cannot be placed against the saved text, so they wait for the whole message.
  await app.emit(delta(' and more'));
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).messages[0].text, 'Partial');
  await app.emit(itemDone('Partial and more, complete.'), turnDone());
  const done = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), v => !v.activeTurn, 'turn end');
  assert.deepEqual(done.messages.map(m => [m.text, m.complete]), [['Partial and more, complete.', true]]);
});

test('malformed or oversized history stays unavailable', async t => {
  const app = await appFor(t);
  for (const data of [
    [turn('completed', [{ id: 'bad', type: 'agentMessage', text: null }])],
    [turn('completed', [message('a'), message('b')])],
    [turn('completed', [message('x'.repeat(2 * 1024 * 1024 + 1))])],
  ]) {
    await app.set('thread/turns/list', { result: { data, nextCursor: null } });
    assert.equal((await activate(app)).status, 503);
  }
});

test('activity timestamps survive listing and advance with responses', async t => {
  const app = await appFor(t);
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd, {
    createdAt: 1600000000, updatedAt: 1700000000,
  })], nextCursor: null } });
  const list = await json(await app.request('/api/threads'));
  assert.equal(list.threads[0].updatedAt, 1700000000000);
  await json(await post(app, '/api/threads'), 201);
  const before = Date.now();
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Work', clientMessageId: CLIENT,
  }), 202);
  await app.emit(delta('Working'), turnDone());
  const status = await eventually(async () => json(await app.request('/api/threads/working')),
    value => value.activity?.[THREAD] >= before && value.threads.length === 0);
  assert.ok(status.activity[THREAD] >= before);
  const refreshed = await json(await app.request('/api/threads'));
  assert.equal(refreshed.threads[0].updatedAt, status.activity[THREAD]);
});
