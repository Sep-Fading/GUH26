// Checks search through the real HTTP backend.
import assert from 'node:assert/strict';
import { test } from 'node:test';
// Creates a private journal for the Claude fixture.
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts isolated providers and polls background indexing.
import { launchApp, json, eventually } from './support/harness.mjs';
// Builds owned history with stable message identities.
import { THREAD, thread, turn, message, CLIENT, itemDone, turnDone } from './support/fixtures.mjs';

// Waits until the full history index finishes.
async function indexed(app, query) {
  return eventually(async () => json(await app.request(`/api/search?q=${encodeURIComponent(query)}`)),
    value => !value.indexing, 'search indexed');
}

test('search reads unopened history, matches typos and reuses the index', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { result: { data: [turn('completed', [
    message('The authentication flow uses a local session.', 'target'),
    message('An unrelated answer.', 'other'),
  ])], nextCursor: null } });
  const result = await indexed(app, 'authetication');
  assert.equal(result.results.length, 1);
  assert.equal(result.results[0].thread.id, THREAD);
  assert.equal(result.results[0].messageId, 'target');
  assert.match(result.results[0].snippet, /authentication/);
  const reads = (await app.inspect()).requests.filter(r => r.method === 'thread/turns/list').length;
  assert.equal((await indexed(app, 'AUTH flow')).results[0].messageId, 'target');
  assert.equal((await indexed(app, 'unrelated')).results[0].messageId, 'other');
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/turns/list').length, reads);
  assert.equal((await app.inspect()).requests.filter(r => ['thread/resume', 'turn/start'].includes(r.method)).length, 0);
});

test('search excludes foreign history and treats syntax as text', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread('/foreign')], nextCursor: null } });
  assert.deepEqual((await indexed(app, 'private')).results, []);
  assert.deepEqual((await indexed(app, '" OR *')).results, []);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/turns/list').length, 0);
});

test('search reads every history page', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { cases: [
    { params: { cursor: null }, handler: { result: { data: [turn('completed', [message('Recent text', 'recent')])], nextCursor: 'older' } } },
    { params: { cursor: 'older' }, handler: { result: { data: [turn('completed', [message('Ancient authentication', 'ancient')])], nextCursor: null } } },
  ] });
  const result = await indexed(app, 'ancient');
  assert.equal(result.results[0]?.messageId, 'ancient');
  assert.equal(result.incomplete, false);
});

test('refresh discovers new history and failures remain visible', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { result: { data: [], nextCursor: null } });
  assert.deepEqual((await indexed(app, 'later')).results, []);
  await app.set('thread/turns/list', { result: { data: [turn('completed', [message('Later history', 'later')])], nextCursor: null } });
  await json(await app.request('/api/search/refresh', { method: 'POST', body: {} }));
  assert.equal((await indexed(app, 'later')).results[0]?.messageId, 'later');
  await app.set('thread/turns/list', { error: { code: -32000, message: 'Unavailable' } });
  await json(await app.request('/api/search/refresh', { method: 'POST', body: {} }));
  assert.equal((await indexed(app, 'later')).incomplete, true);
});


test('finished responses enter an existing index', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { result: { data: [], nextCursor: null } });
  await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
  await indexed(app, 'fresh');
  await json(await app.request(`/api/threads/${THREAD}`));
  await json(await app.request(`/api/threads/${THREAD}/messages`, { method: 'POST',
    body: { text: 'A question', clientMessageId: CLIENT } }), 202);
  await app.emit(itemDone('Fresh searchable answer'), turnDone());
  await eventually(async () => (await indexed(app, 'searchable')).results,
    hits => hits.some(hit => hit.snippet.includes('Fresh searchable answer')), 'completed response indexed');
});

test('an account change discards indexed conversations', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { result: { data: [turn('completed', [message('Sensitive old account', 'old')])], nextCursor: null } });
  assert.equal((await indexed(app, 'sensitive')).results.length, 1);
  await app.set('account/read', { result: { account: { type: 'chatgpt', email: 'different@example.test', planType: 'plus' }, requiresOpenaiAuth: true } });
  await app.set('thread/list', { result: { data: [], nextCursor: null } });
  assert.deepEqual((await indexed(app, 'sensitive')).results, []);
});


test('Claude history is searchable alongside Codex', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-search-claude-'));
  const app = await launchApp({}, { env: {
    CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)),
    MC_CLAUDE_LOG: join(dir, 'calls.jsonl'),
  } });
  t.after(async () => { await app.close(); await rm(dir, { recursive: true, force: true }); });
  const { thread: created } = await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' }, model: 'claude-sonnet-5' } }), 201);
  await json(await app.request(`/api/threads/${created.id}/messages`, { method: 'POST',
    body: { text: 'Hello', clientMessageId: crypto.randomUUID(), model: 'claude-sonnet-5' } }), 202);
  await eventually(async () => json(await app.request(`/api/threads/${created.id}`)),
    value => value.messages.some(m => m.text === 'Hello from Claude'), 'Claude response');
  const result = await indexed(app, 'Claude');
  assert.ok(result.results.some(hit => hit.thread.id === created.id && hit.snippet.includes('Hello from Claude')));
});

test('deleted conversations disappear from search', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
  await app.set('thread/turns/list', { result: { data: [turn('completed', [message('Delete this searchable passage')])], nextCursor: null } });
  assert.equal((await indexed(app, 'searchable')).results.length, 1);
  await app.set('thread/delete', { result: {} });
  await app.set('thread/list', { result: { data: [], nextCursor: null } });
  await json(await app.request(`/api/threads/${THREAD}/delete`, { method: 'POST', body: {} }));
  assert.deepEqual((await indexed(app, 'searchable')).results, []);
});
