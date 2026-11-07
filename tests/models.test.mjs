import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launchApp, json, eventually } from './support/harness.mjs';
import { models, signedOut, THREAD, CLIENT, TURN, started } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
const send = (app, choice) => post(app, `/api/threads/${THREAD}/messages`, {
  text: 'Hello', clientMessageId: CLIENT, ...choice,
});

test('lists all catalog pages', async t => {
  const app = await launchApp({ 'model/list': { hold: true } });
  t.after(() => app.close());
  const response = app.request('/api/models');
  response.catch(() => {});
  for (const [index, cursor] of [[0, null], [1, 'opaque+/=']]) {
    const log = await eventually(() => app.inspect(), state =>
      state.held.some(r => r.method === 'model/list'));
    const request = log.held.find(r => r.method === 'model/list');
    assert.equal(request.params.includeHidden, true);
    assert.equal(request.params.cursor ?? null, cursor);
    await app.peer.call({ op: 'release', id: request.id, reply: { result: {
      data: [{ ...models[index], secretExtra: 'must-not-leak' }],
      nextCursor: index === 0 ? 'opaque+/=' : null,
    } } });
  }
  const body = await json(await response);
  assert.deepEqual(body.models.map(m => m.model), ['fixture-model', 'other-model']);
  assert.equal(body.models[1].hidden, true);
  assert.deepEqual(body.models[1].supportedReasoningEfforts,
    models[1].supportedReasoningEfforts);
  // The speed tiers a model offers reach the page; a model without any lists none.
  assert.deepEqual(body.models[0].serviceTiers, models[0].serviceTiers);
  assert.deepEqual(body.models[1].serviceTiers, []);
  assert.equal(JSON.stringify(body).includes('must-not-leak'), false);
});

test('models require ChatGPT auth', async t => {
  const app = await launchApp({ 'account/read': { result: signedOut } });
  t.after(() => app.close());
  assert.equal((await app.request('/api/models')).status, 401);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'model/list'), false);
});

test('catalog errors are not empty success', async t => {
  const app = await launchApp({ 'model/list': {
    error: { code: -32000, message: 'private upstream detail' },
  } });
  t.after(() => app.close());
  const response = await app.request('/api/models');
  assert.equal(response.status, 502);
  assert.equal((await response.text()).includes('private upstream detail'), false);
});

test('repeated catalog cursors fail', async t => {
  const app = await launchApp({ 'model/list': {
    result: { data: models, nextCursor: 'loop' },
  } });
  t.after(() => app.close());
  const response = await app.request('/api/models');
  assert.ok(response.status >= 500);
  assert.ok((await app.inspect()).requests.filter(r => r.method === 'model/list').length <= 3);
});

test('creation uses requested model and effort', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await app.set('thread/start', { result: {
    ...started(app.peer.cwd), model: 'other-model', reasoningEffort: 'ultra',
  } });
  const body = await json(await post(app, '/api/threads', {
    model: 'other-model', effort: 'ultra',
  }), 201);
  const request = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  assert.equal(request.params.model, 'other-model');
  assert.equal(request.params.config.model_reasoning_effort, 'ultra');
  assert.deepEqual(request.params.environments, []);
  assert.equal(request.params.approvalPolicy, 'never');
  assert.equal(body.thread.model, 'other-model');
  assert.equal(body.thread.effort, 'ultra');
});

test('send changes settings and preserves them in snapshots', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  await json(await send(app, { model: 'other-model', effort: 'ultra' }), 202);
  const requests = (await app.inspect()).requests;
  const request = requests.find(r => r.method === 'turn/start');
  assert.equal(request.params.model, 'other-model');
  assert.equal(request.params.effort, 'ultra');
  assert.deepEqual(request.params.environments, []);
  assert.equal(requests.some(r => r.method.startsWith('config/') &&
    r.method.endsWith('write')), false);
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.thread.model, 'other-model');
  assert.equal(snapshot.thread.effort, 'ultra');
});

test('new model without effort uses its own default', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  await json(await send(app, { model: 'other-model' }), 202);
  assert.equal((await app.inspect()).requests.find(r => r.method === 'turn/start')
    .params.effort, 'none');
});

test('rejects invalid choices before sending', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  for (const choice of [
    { model: 'not-in-catalog', effort: 'high' },
    { model: 'fixture-model', effort: 'ultra' },
    { model: 'catalog-a', effort: 'low' },
    { effort: 'high' }, { model: '', effort: 'low' },
  ]) assert.equal((await send(app, choice)).status, 400);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
  assert.deepEqual((await json(await app.request(`/api/threads/${THREAD}`))).messages, []);
});

// Speed: Codex's service tiers. Verified 2026-09-22 against Codex 0.155: model/list carries
// serviceTiers per model, thread/start and turn/start take serviceTier and echo it, and the
// thread record does not keep it, so MiniClaw keeps a conversation's speed itself.
test('a chosen speed goes out as serviceTier on creation and on every send, shows in the snapshot, and survives a restart', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const body = await json(await post(app, '/api/threads', { model: 'fixture-model', speed: 'priority' }), 201);
  assert.equal((await app.inspect()).requests.find(r => r.method === 'thread/start').params.serviceTier, 'priority');
  assert.equal(body.thread.speed, 'priority');
  await json(await send(app, { model: 'fixture-model', effort: 'high', speed: 'priority' }), 202);
  const request = (await app.inspect()).requests.find(r => r.method === 'turn/start');
  assert.deepEqual([request.params.effort, request.params.serviceTier], ['high', 'priority']);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).thread.speed, 'priority');
  await app.restart();
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).thread.speed, 'priority', 'kept by MiniClaw, since Codex forgets it');
  // Standard again: nothing is sent, and the kept speed goes.
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: '019a4444-4444-7444-8444-444444444445', model: 'fixture-model', effort: 'high' }), 202);
  const plain = (await app.inspect()).requests.filter(r => r.method === 'turn/start').at(-1);
  assert.equal('serviceTier' in plain.params, false);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).thread.speed, null);
});

test('a speed the model does not offer, or one without a model, is refused before anything is sent', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  for (const choice of [{ model: 'other-model', speed: 'priority' }, { model: 'fixture-model', speed: 'fast' }, { speed: 'priority' },
    { model: 'fixture-model', speed: '' }]) {
    assert.equal((await send(app, choice)).status, 400, JSON.stringify(choice));
  }
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
  assert.equal((await post(app, '/api/threads', { model: 'other-model', speed: 'priority' })).status, 400);
});

test('an agent carries a default speed, checked like its effort, and its conversations start with it', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const bad = await post(app, '/api/agents', { name: 'Quick', instructions: 'Hurry.', active: true, model: 'other-model', speed: 'priority' });
  assert.equal(bad.status, 400);
  const { agent } = await json(await post(app, '/api/agents', { name: 'Quick', instructions: 'Hurry.', active: true, model: 'fixture-model', effort: 'low', speed: 'priority' }), 201);
  assert.equal(agent.speed, 'priority');
  const body = await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  assert.equal(body.thread.speed, 'priority');
  assert.equal((await app.inspect()).requests.find(r => r.method === 'thread/start').params.serviceTier, 'priority');
});

test('receipt includes the requested settings', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads'), 201);
  const choice = { model: 'fixture-model', effort: 'high' };
  await json(await send(app, choice), 202);
  await app.set('model/list', { error: { code: -32000, message: 'offline' } });
  assert.equal((await json(await send(app, choice), 202)).turnId, TURN);
  assert.equal((await send(app, { ...choice, effort: 'low' })).status, 409);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start').length, 1);
});
