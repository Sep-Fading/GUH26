// Asserts public replay and send safety contracts.
import assert from 'node:assert/strict';
// Runs disposable backend streaming regression cases.
import { test } from 'node:test';
// Starts synthetic peers and observes HTTP streams.
import { launchApp, json, events, eventually } from './support/harness.mjs';
// Supplies protocol-shaped messages without paid model calls.
import { THREAD, TURN, CLIENT, delta, itemDone, turnDone } from './support/fixtures.mjs';

// Sends a normal authenticated fixture mutation.
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });

// Starts one response and remembers the snapshot cursor from before it.
async function start(app) {
  await json(await post(app, '/api/threads'), 201);
  const before = await json(await app.request(`/api/threads/${THREAD}`));
  const input = { text: 'One question only', clientMessageId: CLIENT };
  await json(await post(app, `/api/threads/${THREAD}/messages`, input), 202);
  return { input, cursor: before.eventCursor };
}

for (const oversized of [false, true]) {
  test(oversized ? 'oversized event resync preserves complete text' :
    'byte-expired replay preserves history and send receipts', async t => {
    const app = await launchApp(); t.after(() => app.close());
    const { input, cursor } = await start(app);
    const chunk = 'a'.repeat(32768);
    const text = oversized ? 'b'.repeat(3 * 1024 * 1024) : chunk.repeat(32);
    if (!oversized) for (let index = 0; index < 32; index++) await app.emit(delta(chunk));
    await app.emit(itemDone(text), turnDone());
    const snapshot = await eventually(async () => json(await app.request(
      `/api/threads/${THREAD}`)), value => value.activeTurn === null);
    assert.equal(snapshot.messages.at(-1).text, text);
    const stream = await events(app, `/api/threads/${THREAD}/events`, {
      'Last-Event-ID': cursor,
    });
    t.after(() => stream.close());
    const marker = await stream.next('resync.required');
    assert.equal(marker.id, undefined);
    const replacement = await stream.next('snapshot');
    assert.equal(replacement.data.messages.at(-1).text, text);
    assert.equal(replacement.data.activeTurn, null);
    assert.equal(replacement.id, snapshot.eventCursor);
    const repeated = await json(await post(app, `/api/threads/${THREAD}/messages`, input), 202);
    assert.equal(repeated.turnId, TURN);
    const requests = (await app.inspect()).requests;
    assert.equal(requests.filter(request => request.method === 'turn/start').length, 1);
    assert.equal(requests.some(request => request.method === 'thread/archive' ||
      request.method === 'thread/delete'), false);
  });
}
