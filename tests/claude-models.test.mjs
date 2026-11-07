// Checks live model discovery, refresh and selection through the actual API and CLI bridge.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { launchApp, json, eventually } from './support/harness.mjs';

const binary = fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url));
const initial = { value: 'sonnet', resolvedModel: 'claude-test-original', displayName: 'Original',
  description: 'Existing model', supportsEffort: true, supportedEffortLevels: ['low', 'high'] };
const added = { value: 'opus[1m]', resolvedModel: 'claude-test-new[1m]', displayName: 'New model',
  description: 'Newly released model', supportsEffort: true, supportedEffortLevels: ['medium', 'max'] };

/** Starts a provider with a catalog the test can change without restarting. */
async function start(t) {
  const dir = await mkdtemp(join(tmpdir(), 'mc-models-'));
  const file = join(dir, 'models.json'), journal = join(dir, 'models.jsonl');
  await writeFile(file, JSON.stringify({ models: [initial] }));
  const app = await launchApp({}, { codex: false, env: { CLAUDE_BIN: binary,
    MC_CLAUDE_MODELS_FILE: file, MC_CLAUDE_MODEL_LOG: journal, MC_CLAUDE_LOG: join(dir, 'calls.jsonl'),
    ANTHROPIC_API_KEY: 'must-not-pass' } });
  t.after(async () => { await app.close(); await rm(dir, { recursive: true, force: true }); });
  return Object.assign(app, { file, journal });
}

test('refresh discovers and uses new Claude models', async t => {
  const app = await start(t);
  const first = await json(await app.request('/api/models'));
  assert.deepEqual(first.models.map(m => m.model), ['claude-test-original']);
  assert.equal(first.models[0].isDefault, true);
  assert.deepEqual(first.models[0].supportedReasoningEfforts.map(e => e.reasoningEffort), ['low', 'high']);
  await writeFile(app.file, JSON.stringify({ models: [initial,
    { ...added, value: 'default', displayName: 'Default (recommended)' }, added,
    { value: 'claude-test-small', displayName: 'Small', description: 'No effort options' }],
    secret: 'never-expose' }));
  assert.equal((await json(await app.request('/api/models'))).models.length, 1, 'ordinary reads share the short cache');
  const refreshed = await json(await app.request('/api/models?refresh=true'));
  assert.deepEqual(refreshed.models.map(m => m.model), ['claude-test-original', 'claude-test-new[1m]', 'claude-test-small']);
  assert.equal(refreshed.models[1].displayName, 'New model');
  assert.equal(refreshed.models[1].isDefault, true);
  assert.deepEqual(refreshed.models[2].supportedReasoningEfforts, []);
  assert.equal(JSON.stringify(refreshed).includes('never-expose'), false);
  const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: {
    environment: { kind: 'host' }, model: added.resolvedModel, effort: 'max',
  } }), 201);
  assert.equal(thread.provider, 'claude');
  await json(await app.request(`/api/threads/${thread.id}/messages`, { method: 'POST', body: {
    text: 'Hello', clientMessageId: crypto.randomUUID(), model: added.resolvedModel, effort: 'max',
  } }), 202);
  await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    s => s.messages.length === 2 && !s.activeTurn, 'new model reply');
  const invalid = await app.request('/api/threads', { method: 'POST', body: {
    environment: { kind: 'host' }, model: 'claude-test-small', effort: 'high',
  } });
  assert.equal(invalid.status, 400);
  const reads = (await readFile(app.journal, 'utf8')).trim().split('\n').map(JSON.parse);
  assert.equal(reads.length, 2, 'validation reuses the discovered catalog');
  assert.ok(reads.every(read => read.apiKey === null && read.args.includes('--safe-mode') &&
    read.args.includes('--no-session-persistence') && read.requests.length === 1));
});

test('failed refresh reports an error and retains the last catalog for validation', async t => {
  const app = await start(t);
  await json(await app.request('/api/models'));
  await writeFile(app.file, JSON.stringify({ error: 'private upstream failure' }));
  const failure = await app.request('/api/models?refresh=true');
  assert.ok(failure.status >= 500);
  const body = await failure.text();
  assert.equal(body.includes('private upstream failure'), false);
  assert.equal(body.includes('Codex'), false, 'a Claude failure names the correct provider');
  assert.deepEqual((await json(await app.request('/api/models'))).models.map(m => m.model), ['claude-test-original']);
  assert.equal((await app.request('/api/models?refresh=wrong')).status, 400);
});
