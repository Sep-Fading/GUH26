// Checks the message queue: messages wait while a response runs, leave one per turn in order,
// pause when a turn does not end well, and come back paused after a restart.
import assert from 'node:assert/strict';
// Registers the queue tests.
import { test } from 'node:test';
// Names queued messages.
import { randomUUID } from 'node:crypto';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually, events } from './support/harness.mjs';
// Names the fake conversation, its turn, and ends it.
import { THREAD, TURN, CLIENT, turnDone } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const queue = async app => (await json(await app.request(`/api/threads/${THREAD}`))).queue;
const turnStarts = async app => (await app.inspect()).requests.filter(request => request.method === 'turn/start');
const sentTexts = async app => (await turnStarts(app)).map(request => request.params.input[0].text);

/** Starts a conversation whose first response is still running. */
async function responding(t, env = {}) {
  const app = await launchApp({}, { env });
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'First', clientMessageId: CLIENT }), 202);
  return app;
}

/** Queues one message as the page would. */
const enqueue = (app, text, extra = {}) => post(app, `/api/threads/${THREAD}/queue`, { text, clientMessageId: randomUUID(), ...extra });

/** Makes the fake answer the next turn under its own id, then ends the running one. */
async function finish(app, next, status = 'completed') {
  if (next) {
    const turn = { id: next, status: 'inProgress', items: [] };
    await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  }
  await app.emit(turnDone(status));
}
const done = id => ({ method: 'turn/completed', params: { threadId: THREAD, turn: { id, status: 'completed', items: [] } } });

test('messages queue while a response runs and leave one per turn, in order', async t => {
  const app = await responding(t);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, { text: 'Direct', clientMessageId: randomUUID() })).status, 409,
    'a direct send is still refused while a response runs');
  const { item: second } = await json(await enqueue(app, 'Second', { model: 'fixture-model', effort: 'low', speed: 'priority' }), 201);
  assert.equal(second.speed, 'priority', 'a queued message keeps the speed chosen with it');
  await json(await enqueue(app, 'Third'), 201);
  assert.deepEqual([second.text, second.model, second.effort], ['Second', 'fixture-model', 'low']);
  assert.deepEqual((await queue(app)).items.map(item => item.text), ['Second', 'Third']);
  assert.deepEqual(await sentTexts(app), ['First'], 'nothing is sent while the response runs');

  await finish(app, 'turn-2');
  await eventually(() => sentTexts(app), texts => texts.length === 2, 'second turn');
  assert.deepEqual(await sentTexts(app), ['First', 'Second']);
  const started = (await turnStarts(app))[1].params;
  assert.deepEqual([started.model, started.effort, started.serviceTier], ['fixture-model', 'low', 'priority'], 'it uses what was chosen when it was queued');
  assert.deepEqual((await queue(app)).items.map(item => item.text), ['Third']);
  await app.emit(done('turn-2'));
  await eventually(() => sentTexts(app), texts => texts.length === 3, 'third turn');
  assert.deepEqual([(await sentTexts(app))[2], (await queue(app)).items], ['Third', []]);
  // The queued messages are ordinary messages of the conversation.
  const { messages } = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(messages.filter(message => message.role === 'user').map(message => message.text), ['First', 'Second', 'Third']);
});

// A queued message is sent by the backend, not the page, so the stream says which message started the turn.
test('a queued message names the turn it started', async t => {
  const app = await responding(t);
  const stream = await events(app, `/api/threads/${THREAD}/events`);
  t.after(() => stream.close());
  const id = randomUUID();
  await json(await enqueue(app, 'Second', { clientMessageId: id }), 201);
  await finish(app, 'turn-2');
  const { messages } = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
    value => value.activeTurn?.id === 'turn-2', 'second turn');
  assert.deepEqual(messages.map(message => [message.text, message.turnId]), [['First', TURN], ['Second', 'turn-2']]);
  const started = await eventually(() => stream.seen.filter(event => event.event === 'turn.started'),
    seen => seen.some(event => event.data.turnId === 'turn-2'), 'turn.started');
  assert.deepEqual([messages[1].id, started.find(event => event.data.turnId === 'turn-2').data.messageId], [id, id]);
});

// "Send now" stops a turn that is still working. Its step keeps its turn and its message names that turn,
// and the message sent now names the next one, so the page keeps the stopped turn's cards above it.
test('"send now" leaves the stopped turn named on its own message, with its steps', async t => {
  const app = await responding(t);
  await app.emit({ id: 'ask-1', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD,
    turnId: TURN, callId: 'ask-1', arguments: {} } });
  const id = randomUUID();
  const { item } = await json(await enqueue(app, 'Urgent', { clientMessageId: id }), 201);
  const turn = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/queue/${item.id}/send-now`), 202);
  await eventually(() => app.inspect(), value => value.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  const snapshot = await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)),
    value => value.activeTurn?.id === 'turn-2', 'urgent turn');
  assert.deepEqual(snapshot.messages.map(message => [message.id, message.turnId]), [[CLIENT, TURN], [id, 'turn-2']]);
  assert.equal(snapshot.activities.find(entry => entry.id === 'ask-1').turnId, TURN);
});

test('queueing into an idle conversation sends at once; the same id is queued once', async t => {
  const app = await responding(t);
  const id = randomUUID();
  await json(await post(app, `/api/threads/${THREAD}/queue`, { text: 'Once', clientMessageId: id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/queue`, { text: 'Once', clientMessageId: id }), 201);
  assert.equal((await queue(app)).items.length, 1);
  assert.equal((await post(app, `/api/threads/${THREAD}/queue`, { text: 'Other words', clientMessageId: id })).status, 409);
  await finish(app, 'turn-2');
  await eventually(() => sentTexts(app), texts => texts.length === 2, 'queued send');
  await app.emit(done('turn-2'));
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'idle');
  const turn = { id: 'turn-3', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await enqueue(app, 'While idle'), 201);
  await eventually(() => sentTexts(app), texts => texts.at(-1) === 'While idle', 'idle queue sends at once');
  assert.deepEqual((await queue(app)).items, []);
});

test('a queued message can be edited and removed, and the queue cleared; bad input is refused', async t => {
  const app = await responding(t);
  const { item } = await json(await enqueue(app, 'Draft'), 201);
  await json(await enqueue(app, 'Keep'), 201);
  const edited = await json(await post(app, `/api/threads/${THREAD}/queue/${item.id}`, { text: 'Final words' }));
  assert.equal(edited.item.text, 'Final words');
  for (const bad of [{ text: '' }, { text: 'x'.repeat(32769) }, { text: 'ok', extra: 1 }]) {
    assert.equal((await post(app, `/api/threads/${THREAD}/queue/${item.id}`, bad)).status, 400);
  }
  assert.equal((await post(app, `/api/threads/${THREAD}/queue/${randomUUID()}`, { text: 'x' })).status, 404);
  await json(await post(app, `/api/threads/${THREAD}/queue/${item.id}/delete`));
  assert.deepEqual((await queue(app)).items.map(queued => queued.text), ['Keep']);
  for (let index = 0; index < 19; index += 1) await json(await enqueue(app, `Filler ${index}`), 201);
  assert.equal((await enqueue(app, 'One too many')).status, 409, 'a queue holds 20 messages');
  await json(await post(app, `/api/threads/${THREAD}/queue/clear`));
  assert.deepEqual((await queue(app)).items, []);
  assert.equal((await enqueue(app, '')).status, 400);
});

test('a turn that fails or is stopped pauses the queue until Resume', async t => {
  const app = await responding(t);
  await json(await enqueue(app, 'After failure'), 201);
  await finish(app, 'turn-2', 'failed');
  const paused = await eventually(() => queue(app), value => value.paused, 'paused after a failed turn');
  assert.equal(paused.reason, 'failed');
  assert.deepEqual(await sentTexts(app), ['First'], 'nothing is sent after a failure');
  // A message queued into a paused queue waits too, even though the conversation is idle.
  await json(await enqueue(app, 'Also waits'), 201);
  assert.deepEqual(await sentTexts(app), ['First']);
  await json(await post(app, `/api/threads/${THREAD}/queue/resume`));
  await eventually(() => sentTexts(app), texts => texts.length === 2, 'resumed');
  assert.deepEqual([(await sentTexts(app))[1], (await queue(app)).paused], ['After failure', false]);
  // Stop pauses as well.
  await json(await post(app, `/api/threads/${THREAD}/interrupt`, { turnId: 'turn-2' }));
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: 'turn-2', status: 'interrupted', items: [] } } });
  const stopped = await eventually(() => queue(app), value => value.paused, 'paused after Stop');
  assert.equal(stopped.reason, 'stopped');
  assert.equal((await sentTexts(app)).length, 2);
});

test('"send now" interrupts the running response and sends that message next, without pausing', async t => {
  const app = await responding(t);
  await json(await enqueue(app, 'Later'), 201);
  const { item } = await json(await enqueue(app, 'Urgent'), 201);
  const turn = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/queue/${item.id}/send-now`), 202);
  await eventually(() => app.inspect(), value => value.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
  assert.deepEqual(await sentTexts(app), ['First'], 'it waits for the turn to really end');
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  await eventually(() => sentTexts(app), texts => texts.length === 2, 'urgent send');
  const state = await queue(app);
  assert.deepEqual([(await sentTexts(app))[1], state.items.map(queued => queued.text), state.paused], ['Urgent', ['Later'], false]);
});

test('a queue survives a restart, paused, and goes with its conversation', async t => {
  const app = await responding(t);
  await json(await enqueue(app, 'Kept'), 201);
  await app.restart();
  const kept = await queue(app);
  assert.deepEqual([kept.items.map(item => item.text), kept.paused, kept.reason], [['Kept'], true, 'restart']);
  assert.deepEqual(await sentTexts(app), [], 'nothing is sent by itself after a restart');
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  assert.equal((await app.request(`/api/threads/${THREAD}/queue`)).status, 404);
});
