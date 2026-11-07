// Checks the name a user gives a conversation: it replaces the provider's name everywhere, lasts
// across a restart, can be taken back, and goes with the conversation.
import assert from 'node:assert/strict';
// Registers the naming tests.
import { test } from 'node:test';
// Locates the fake Claude CLI and its journal.
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
// Names the fake conversation and its message.
import { THREAD, CLIENT } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const listed = async (app, id) => (await json(await app.request('/api/threads'))).threads.find(thread => thread.id === id).name;
const shown = async (app, id) => (await json(await app.request(`/api/threads/${id}`))).thread.name;

test('a given name replaces the provider\'s, survives a restart and can be taken back', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Create a low-poly tree', clientMessageId: CLIENT }), 202);
  const before = await shown(app, THREAD);
  const { thread } = await json(await post(app, `/api/threads/${THREAD}/name`, { name: '  Roblox tree  ' }));
  assert.equal(thread.name, 'Roblox tree');
  assert.deepEqual([await shown(app, THREAD), await listed(app, THREAD)], ['Roblox tree', 'Roblox tree']);
  for (const name of ['x'.repeat(121), 'two\nlines', 7]) assert.equal((await post(app, `/api/threads/${THREAD}/name`, { name })).status, 400, String(name).slice(0, 12));
  assert.equal((await post(app, '/api/threads/019a0000-0000-7000-8000-000000000000/name', { name: 'Ghost' })).status, 404);
  // An empty name gives the conversation back the name it had.
  await json(await post(app, `/api/threads/${THREAD}/name`, { name: '' }));
  assert.deepEqual([await shown(app, THREAD), await listed(app, THREAD)], [before, before]);
  await json(await post(app, `/api/threads/${THREAD}/name`, { name: 'Roblox tree' }));
  await app.restart();
  assert.deepEqual([await listed(app, THREAD), await shown(app, THREAD)], ['Roblox tree', 'Roblox tree']);
});

test('a Claude conversation is named the same way', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-name-claude-'));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: 'claude-sonnet-5' }), 201);
  await json(await post(app, `/api/threads/${thread.id}/name`, { name: 'Tree notes' }));
  assert.deepEqual([await shown(app, thread.id), await listed(app, thread.id)], ['Tree notes', 'Tree notes']);
});
