// Reads installed providers' current catalogs without sending model prompts.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { launchApp, json, eventually } from '../support/harness.mjs';

test('installed providers refresh their model catalogs', { skip: process.env.MINICLAW_PROBE_MODELS !== '1' }, async t => {
  const app = await launchApp({}, { real: true, env: {
    CLAUDE_BIN: process.env.CLAUDE_BIN ?? join(homedir(), '.local/bin/claude'),
  } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(),
    value => value.providers?.every(provider => provider.status === 'signed_in'), 'provider sign-ins', 30000);
  const first = await json(await app.request('/api/models'));
  const fresh = await json(await app.request('/api/models?refresh=true'));
  for (const provider of ['codex', 'claude']) {
    const models = fresh.models.filter(model => (model.provider ?? 'codex') === provider);
    assert.ok(models.length, `${provider} returned models`);
    assert.ok(first.models.some(model => (model.provider ?? 'codex') === provider));
    console.log(`${provider}: ${models.map(model => `${model.model} (${model.supportedReasoningEfforts.map(e => e.reasoningEffort).join('/') || 'no explicit effort'})`).join(', ')}`);
  }
});
