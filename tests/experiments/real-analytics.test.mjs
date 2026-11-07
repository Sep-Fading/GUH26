// A real model, real token counts. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-analytics.test.mjs
// It checks what no fake can: that the numbers the Analytics tab shows are the provider's own.
// Add MINICLAW_PROBE_CLAUDE=1 to run the same through Claude Code.
import assert from 'node:assert/strict';
// Registers the opt-in experiment.
import { test } from 'node:test';
// Finds the installed Claude CLI.
import { execFileSync } from 'node:child_process';
// Starts MiniClaw against the installed providers and their existing sign-ins.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

/** Sends one message and returns what MiniClaw then recorded about it. */
async function records(app, choice) {
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...choice }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Name three primary colours. One line.',
    clientMessageId: crypto.randomUUID(), ...choice }), 202);
  await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'the reply', 180000);
  const found = await eventually(async () => json(await app.request('/api/analytics?days=1')),
    value => value.totals.turns >= 1, 'the recorded turn', 30000);
  console.log(`${choice.model}:`, JSON.stringify({ totals: found.totals, models: found.models }));
  // The provider charged something, and the parts add up to what is shown as the whole.
  assert.ok(found.totals.tokens > 0, 'the provider reported tokens');
  assert.ok(found.totals.inputTokens > 0 && found.totals.outputTokens > 0);
  assert.deepEqual(found.models.map(row => [row.name, row.provider]), [[choice.model, choice.model.startsWith('claude') ? 'claude' : 'codex']]);
  assert.equal(found.daily.at(-1).tokens, found.totals.tokens, 'today\'s column holds the whole day');
  assert.ok(found.totals.seconds > 0, 'the wait was timed');
}

test('a real Codex turn is recorded with the tokens Codex reported', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  await records(app, { model: 'gpt-5.6-luna', effort: 'low' });
});

test('a real Claude turn is recorded with the tokens the CLI reported', { skip: !(enabled && process.env.MINICLAW_PROBE_CLAUDE === '1') && 'set MINICLAW_PROBE_CLAUDE=1 as well', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true, env: { CLAUDE_BIN: execFileSync('/bin/zsh', ['-lc', 'command -v claude'], { encoding: 'utf8' }).trim() } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/models')).json(), value => JSON.stringify(value).includes('claude-haiku'), 'the Claude models', 60000);
  await records(app, { model: 'claude-haiku-4-5-20251001', effort: 'low' });
});
