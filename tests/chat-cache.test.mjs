// Checks cache admission through real HTTP requests.
import assert from 'node:assert/strict';
// Registers isolated cache admission acceptance tests.
import { test } from 'node:test';
// Generates distinct synthetic saved chat identifiers.
import { randomUUID } from 'node:crypto';
// Starts the synthetic Codex integration harness.
import { launchApp, json, events, eventually } from './support/harness.mjs';
// Builds schema-shaped history and notification fixtures.
import { THREAD, CLIENT, thread, started, turnDone } from './support/fixtures.mjs';

/** Starts a backend and registers its cleanup. */
async function appFor(t) {
  const app = await launchApp();
  t.after(() => app.close());
  return app;
}

/** Configures the synthetic history reader for one task. */
async function saved(app, id) {
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { id }) } });
  await app.set('thread/resume', { result: started(app.peer.cwd, { id }) });
}

/** Loads a sequence of distinct idle saved tasks. */
async function fill(app, count) {
  const ids = [];
  for (let index = 0; index < count; index += 1) {
    const id = randomUUID();
    await saved(app, id);
    await json(await app.request(`/api/threads/${id}`));
    ids.push(id);
  }
  return ids;
}

test('idle history yields a cache slot and reloads with a fresh cursor', async t => {
  const app = await appFor(t);
  const first = await json(await app.request(`/api/threads/${THREAD}`));
  await fill(app, 32);
  await saved(app, THREAD);
  const restored = await json(await app.request(`/api/threads/${THREAD}`));
  assert.notEqual(restored.eventCursor, first.eventCursor);
  const requests = (await app.inspect()).requests;
  assert.equal(requests.filter(r => r.method === 'thread/resume'
    && r.params.threadId === THREAD).length, 2);
  assert.equal(requests.filter(r => ['thread/archive', 'thread/delete', 'turn/start']
    .includes(r.method)).length, 0);
});

test('new chats use idle capacity while an observed chat keeps its stream', async t => {
  const app = await appFor(t);
  const feed = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => feed.close());
  const initial = await feed.next('snapshot');
  await fill(app, 31);
  const id = randomUUID();
  await app.set('thread/start', { result: started(app.peer.cwd, { id }) });
  await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } }), 201);
  const retained = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(retained.eventCursor, initial.data.eventCursor);
  await app.emit({ method: 'turn/started', params: {
    threadId: THREAD, turn: { id: 'observed-turn' },
  } });
  assert.equal((await feed.next('turn.started')).data.turnId, 'observed-turn');
});

test('completed send receipts survive cache pressure without a second send', async t => {
  const app = await appFor(t);
  await json(await app.request(`/api/threads/${THREAD}/environment`, {
    method: 'POST', body: { kind: 'host' },
  }));
  await json(await app.request(`/api/threads/${THREAD}`));
  const body = { text: 'Keep my receipt', clientMessageId: CLIENT };
  const route = `/api/threads/${THREAD}/messages`;
  const first = await json(await app.request(route, { method: 'POST', body }), 202);
  await app.emit(turnDone());
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
    snapshot => snapshot.activeTurn === null, 'completed response');
  await fill(app, 33);
  const repeat = await json(await app.request(route, { method: 'POST', body }), 202);
  assert.equal(repeat.turnId, first.turnId);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start').length, 1);
});

test('full observed cache refuses admission until a stream closes', async t => {
  const app = await appFor(t);
  const feeds = [];
  t.after(() => { for (const feed of feeds) feed.close(); });
  for (let index = 0; index < 32; index += 1) {
    const id = randomUUID();
    await saved(app, id);
    const feed = await events(app, `/api/threads/${id}/events`);
    await feed.next('snapshot');
    feeds.push(feed);
  }
  const id = randomUUID();
  await saved(app, id);
  const blocked = await json(await app.request(`/api/threads/${id}`), 409);
  assert.equal(blocked.error.code, 'task_limit');
  await app.set('thread/start', { result: started(app.peer.cwd, { id }) });
  await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } }), 409);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/start').length, 0);
  feeds[0].close();
  await eventually(async () => (await app.request(`/api/threads/${id}`)).status,
    status => status === 200, 'released stream capacity');
});

test('new empty chats stay cached before saved history is available', async t => {
  const app = await appFor(t);
  await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } }), 201);
  await fill(app, 32);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd) } });
  await app.set('thread/resume', { error: { code: -32600, message: 'No rollout found' } });
  await app.set('thread/turns/list', { error: { code: -32600, message: 'Not materialized yet' } });
  const retained = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(retained.thread.id, THREAD);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/resume'
    && r.params.threadId === THREAD).length, 0);
});

test('a conversation whose messages were all answered gives its cache slot back', async t => {
  // Each send leaves a receipt for safe retries. A receipt of a finished turn must not hold
  // the conversation in the cache for ever, or the 33rd used conversation needs a restart.
  const app = await appFor(t);
  for (let index = 0; index < 33; index += 1) {
    const id = randomUUID();
    await saved(app, id);
    await json(await app.request(`/api/threads/${id}/environment`, { method: 'POST', body: { kind: 'host' } }));
    await json(await app.request(`/api/threads/${id}/messages`, { method: 'POST',
      body: { text: `Message ${index}`, clientMessageId: randomUUID() } }), 202);
    const done = turnDone();
    done.params.threadId = id;
    await app.emit(done);
    await eventually(async () => json(await app.request(`/api/threads/${id}`)), value => !value.activeTurn, `turn ${index} end`);
  }
});
