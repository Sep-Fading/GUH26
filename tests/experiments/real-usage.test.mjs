// Reads real provider quotas through MiniClaw without sending a model prompt.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { launchApp, json, eventually } from '../support/harness.mjs';

test('installed providers return real account limits', { skip: process.env.MINICLAW_PROBE_USAGE !== '1' }, async t => {
  const app = await launchApp({}, { real: true, env: {
    CLAUDE_BIN: process.env.CLAUDE_BIN ?? join(homedir(), '.local/bin/claude'),
  } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(),
    value => value.providers?.every(provider => provider.status === 'signed_in'), 'provider sign-ins', 30000);
  for (const provider of ['codex', 'claude']) {
    const reading = await json(await app.request(`/api/usage?provider=${provider}&refresh=true`));
    assert.equal(reading.status, 'fresh', provider);
    assert.ok(reading.buckets.some(bucket => [bucket.primary, bucket.secondary]
      .some(window => Number.isFinite(window?.usedPercent))), `${provider} returned percentages`);
    console.log(`${provider}: fresh, ${reading.buckets.length} limit groups, reset times ${reading.buckets
      .flatMap(bucket => [bucket.primary, bucket.secondary]).filter(window => window?.resetsAt).length}`);
  }
});
