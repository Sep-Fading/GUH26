// Checks the Codex image switch and generated pictures without using paid generation.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { launchApp, json, events } from './support/harness.mjs';
import { THREAD, TURN, CLIENT, turnDone, turn, defaults, started } from './support/fixtures.mjs';

const MAIN = '00000000-0000-0000-0000-000000000000';
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScL/nwAAAABJRU5ErkJggg==';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const feature = { target_type: 'feature', name: 'image_generation' };
const image = (status, result = '') => ({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN,
  item: { type: 'imageGeneration', id: 'image-1', status, result } } });

test('image generation needs agent access and a conversation switch', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const capabilities = async query => (await json(await app.request(`/api/capabilities?${query}`))).capabilities;
  const row = async query => (await capabilities(query)).find(item => item.key === 'image_generation');
  assert.deepEqual([ (await row('')).allowed, (await row('')).on ], [false, false]);
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'allow' }));
  assert.deepEqual([ (await row('')).allowed, (await row('')).on ], [true, false]);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { image_generation: true } }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  assert.deepEqual([ (await row(`thread=${THREAD}`)).allowed, (await row(`thread=${THREAD}`)).on ], [true, true]);
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start');
  assert.equal(start.params.config['features.image_generation'], true);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: false }));
  assert.equal((await row(`thread=${THREAD}`)).on, false);
  const resumed = (await app.inspect()).requests.filter(request => request.method === 'thread/resume');
  assert.equal(resumed.at(-1)?.params.config['features.image_generation'], false);
  await app.restart();
  assert.equal((await row(`thread=${THREAD}`)).on, false);
});

test('an existing host conversation reloads its image tool when switched on', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: true }));
  const requests = (await app.inspect()).requests;
  assert.ok(requests.some(request => request.method === 'thread/archive'));
  assert.ok(requests.some(request => request.method === 'thread/unarchive'));
  assert.equal(requests.filter(request => request.method === 'thread/resume').at(-1)?.params.config['features.image_generation'], true);
});

test('revoking image access blocks a loaded chat until its native tool is reloaded', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { image_generation: true } }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'remove' }));
  const blocked = await json(await app.request(`/api/threads/${THREAD}`));
  assert.match(blocked.imageError, /Reload this chat/);
  assert.equal(blocked.imageRetryOn, false);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT })).status, 409);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: false }));
  const resumed = (await app.inspect()).requests.filter(request => request.method === 'thread/resume');
  assert.equal(resumed.at(-1)?.params.config['features.image_generation'], false);
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).imageError, null);
});

test('direct conversation image revocation also requires native reload', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { image_generation: true } }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: feature, effect: 'block' }));
  const blocked = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(blocked.imageRetryOn, false);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT })).status, 409);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: false }));
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).imageError, null);
});

test('project image revocation blocks its loaded host conversation', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', { name: 'Art', environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/permissions/project/${project.id}`, { target: feature, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id,
    capabilities: { image_generation: true } }), 201);
  await json(await post(app, `/api/permissions/project/${project.id}`, { target: feature, effect: 'remove' }));
  const blocked = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(blocked.imageRetryOn, false);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT })).status, 409);
});

test('a failed image-tool reload blocks sending until retry succeeds', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: feature, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  await app.set('thread/resume', { error: { code: -32000, message: 'Lost acknowledgement' } });
  assert.equal((await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: true })).status, 502);
  const failed = await json(await app.request(`/api/threads/${THREAD}`));
  assert.match(failed.imageError, /Retry image generation/);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT })).status, 409);
  await app.set('thread/resume', { result: started(app.peer.cwd) });
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'image_generation', on: true }));
  const restored = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(restored.imageError, null);
});

test('generated image is kept as a private assistant attachment', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const stream = await events(app, `/api/threads/${THREAD}/events`); t.after(() => stream.close());
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw a red dot', clientMessageId: CLIENT }), 202);
  await app.emit({ method: 'item/started', params: { threadId: THREAD, turnId: TURN,
    item: { type: 'imageGeneration', id: 'image-1', status: '', result: '' } } });
  assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).messages
    .find(message => message.id === 'image-1')?.imageStatus, 'generating');
  await app.emit(image('completed', PNG), turnDone());
  await stream.next('turn.completed');
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  const generated = snapshot.messages.find(message => message.id === 'image-1');
  assert.equal(generated.role, 'assistant');
  assert.equal(generated.attachments?.[0]?.kind, 'image');
  assert.equal(JSON.stringify(snapshot).includes(PNG), false);
  assert.equal(stream.seen.some(event => JSON.stringify(event).includes(PNG)), false);
  const response = await app.request(`/api/threads/${THREAD}/attachments/${generated.attachments[0].id}`);
  assert.equal(response.headers.get('content-type'), 'image/png');
  assert.deepEqual(Buffer.from(await response.arrayBuffer()), Buffer.from(PNG, 'base64'));
  await app.emit(image('completed', PNG));
  const replayed = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(replayed.messages.filter(message => message.id === 'image-1').length, 1);
  assert.equal(replayed.messages.find(message => message.id === 'image-1').attachments[0].id,
    generated.attachments[0].id);
  await writeFile(join(app.peer.dir, 'scenario.json'), JSON.stringify({ handlers: {
    ...defaults(app.peer.cwd),
    'thread/turns/list': { result: { data: [turn('completed', [
      { type: 'imageGeneration', id: 'image-1', status: 'completed', result: PNG },
    ])], nextCursor: null } },
  } }));
  await app.restart();
  await json(await post(app, `/api/threads/${THREAD}/activate`));
  const restored = await json(await app.request(`/api/threads/${THREAD}`));
  assert.ok(restored.messages.find(message => message.id === 'image-1'), JSON.stringify(restored.messages));
  assert.equal(restored.messages.find(message => message.id === 'image-1').attachments[0].id,
    generated.attachments[0].id);
});

test('generation failures stay visible without exposing raw output', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT }), 202);
  await app.emit(image('failed'), turnDone());
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.messages.find(message => message.id === 'image-1')?.imageStatus, 'failed');
});

test('invalid completed image reports a save failure', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Draw', clientMessageId: CLIENT }), 202);
  await app.emit(image('completed', 'not a PNG'), turnDone());
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.match(snapshot.messages.find(message => message.id === 'image-1')?.text, /Could not save generated image/);
});
