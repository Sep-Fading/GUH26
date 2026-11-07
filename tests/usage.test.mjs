// Checks provider quotas through the real backend and disposable CLI fixtures.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { launchApp, json } from './support/harness.mjs';

const binary = fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url));

/** Starts a Claude-only app and removes it after the assertion. */
async function start(t, extra = {}) {
  // The fake CLI appends its calls to this log; `/dev/null` is no file on Windows.
  const dir = await mkdtemp(join(tmpdir(), 'mc-usage-'));
  const app = await launchApp({}, { codex: false, env: { CLAUDE_BIN: binary,
    MC_CLAUDE_LOG: join(dir, 'calls.jsonl'), ...extra } });
  t.after(async () => { await app.close(); await rm(dir, { recursive: true, force: true }); });
  return app;
}

test('Claude limits work without Codex and include model windows', async t => {
  const app = await start(t);
  const body = await json(await app.request('/api/usage?provider=claude'));
  assert.equal(body.status, 'fresh');
  assert.equal(body.provider, 'claude');
  assert.equal(body.buckets[0].primary.usedPercent, 10);
  assert.equal(body.buckets[0].primary.resetsAt, 2000000000);
  assert.equal(body.buckets[0].secondary.usedPercent, 51);
  assert.equal(body.buckets[1].limitName, 'Sonnet');
  assert.equal(body.buckets[1].secondary.usedPercent, 98);
  assert.equal(body.ordinaryUsageAllowed, null, 'quota percentages alone cannot decide whether a turn is allowed');
  assert.ok(Number.isFinite(Date.parse(body.fetchedAt)));
  assert.equal((await app.request('/api/usage?provider=unknown')).status, 400);
});

test('Claude usage is read-only, cached, and retained as stale after a failed refresh', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-usage-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const file = `${dir}/usage.json`, journal = `${dir}/usage-calls.jsonl`;
  await writeFile(file, JSON.stringify({ rate_limits_available: true,
    rate_limits: { five_hour: { utilization: 0, resets_at: null }, seven_day: null }, secret: 'never-expose' }));
  const next = await start(t, { MC_CLAUDE_USAGE_FILE: file, MC_CLAUDE_LOG: journal,
    ANTHROPIC_API_KEY: 'must-not-pass' });
  const initial = await json(await next.request('/api/usage?provider=claude'));
  assert.equal(initial.buckets[0].primary.usedPercent, 0);
  assert.equal(initial.buckets[0].secondary, null);
  assert.equal(JSON.stringify(initial).includes('never-expose'), false);
  await json(await next.request('/api/usage?provider=claude'));
  const calls = (await readFile(journal, 'utf8')).trim().split('\n').map(JSON.parse);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].apiKey, null);
  assert.deepEqual(calls[0].requests.map(row => row.request.subtype), ['initialize', 'get_usage']);
  assert.equal(calls[0].requests[1].request.skip_behaviors, true);
  assert.ok(calls[0].args.includes('--safe-mode'));
  assert.ok(calls[0].args.includes('--no-session-persistence'));
  await writeFile(file, JSON.stringify({ error: 'Unsupported request' }));
  const stale = await json(await next.request('/api/usage?provider=claude&refresh=true'));
  assert.equal(stale.status, 'stale');
  assert.deepEqual(stale.buckets, initial.buckets);
  assert.equal(stale.fetchedAt, initial.fetchedAt);
});

test('Claude sign-in is required and missing limits stay unknown', async t => {
  const signedOut = await start(t, { MC_CLAUDE_SIGNED_OUT: '1' });
  assert.equal((await signedOut.request('/api/usage?provider=claude')).status, 401);
  const app = await start(t);
  const file = `${app.peer.dir}/missing-usage.json`;
  await writeFile(file, JSON.stringify({ rate_limits_available: false, rate_limits: null }));
  const missing = await start(t, { MC_CLAUDE_USAGE_FILE: file });
  const body = await json(await missing.request('/api/usage?provider=claude'));
  assert.equal(body.status, 'unavailable');
  assert.deepEqual(body.buckets, []);
  assert.equal(body.ordinaryUsageAllowed, null);
});

test('Claude structured windows take precedence and include enabled credit limits', async t => {
  const app = await start(t);
  const file = `${app.peer.dir}/structured-usage.json`;
  await writeFile(file, JSON.stringify({ rate_limits_available: true, rate_limits: {
    five_hour: { utilization: 1 }, seven_day: { utilization: 2 },
    limits: [
      { kind: 'session', group: 'session', percent: 12, resets_at: 'invalid' },
      { kind: 'weekly_all', group: 'weekly', percent: 34, resets_at: '2033-05-18T03:33:20Z' },
      { kind: 'weekly_scoped', group: 'weekly', percent: 99, scope: { model: { display_name: 'Sonnet' } } },
    ],
    model_scoped: [{ display_name: 'Sonnet', utilization: 2 }],
    spend: { enabled: true, percent: 120 },
  } }));
  const next = await start(t, { MC_CLAUDE_USAGE_FILE: file });
  const body = await json(await next.request('/api/usage?provider=claude'));
  assert.equal(body.buckets.length, 3, 'legacy and structured rows must not duplicate each other');
  assert.equal(body.buckets[0].primary.usedPercent, 12);
  assert.equal(body.buckets[0].primary.resetsAt, null);
  assert.equal(body.buckets[0].secondary.usedPercent, 34);
  assert.equal(body.buckets[1].secondary.usedPercent, 99);
  assert.equal(body.buckets[2].primary.usedPercent, 120);
  assert.equal(body.buckets[2].primary.windowDurationMins, null);
});
