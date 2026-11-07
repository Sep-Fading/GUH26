// A real model, a real question. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-questions.test.mjs
// It checks two things no fake can: that a real model opens the question popup instead of asking
// in its message, and that the briefing lets it answer questions about its surroundings itself.
import assert from 'node:assert/strict';
// Registers the opt-in experiment.
import { test } from 'node:test';
// Starts MiniClaw against the installed Codex and its existing sign-in.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
// A small model at low effort; never a large one for a probe.
const MODEL = { model: 'gpt-5.6-luna', effort: 'low' };
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

test('a real Codex model asks through the popup and knows where it is', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true });
  t.after(() => app.close());
  // The real Codex takes a few seconds to start and read the existing sign-in.
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...MODEL }), 201);
  const snapshot = async () => json(await app.request(`/api/threads/${thread.id}`));
  const say = async text => json(await post(app, `/api/threads/${thread.id}/messages`, { text, clientMessageId: crypto.randomUUID(), ...MODEL }), 202);
  const settled = () => eventually(snapshot, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'the reply', 120000);

  // 1. Surroundings: nothing here should need a question.
  await say('Without asking me anything: what is today\'s date, what kind of computer are you running on, and can you read files in this conversation? One short line each.');
  const first = await settled();
  const knows = first.messages.at(-1).text;
  console.log('SURROUNDINGS:', knows.replaceAll('\n', ' | '));
  assert.equal(first.activities.some(entry => entry.kind === 'question'), false, 'it did not ask');
  assert.match(knows, new RegExp(String(new Date().getFullYear())));
  assert.match(knows, /mac/i);

  // 2. A decision that is the user's: it must come as a popup, not as text.
  await say('I want a tiny script that prints a greeting. I have not decided the language, and that choice is mine. Find out from me, then write it in your reply.');
  const asked = await eventually(snapshot, value => value.question || (!value.activeTurn && value.messages.at(-1)?.role === 'assistant'), 'a question or a reply', 120000);
  console.log('ASKED:', JSON.stringify(asked.question?.questions ?? null));
  assert.ok(asked.question, `the model asked in its message instead of the popup: ${asked.messages.at(-1)?.text?.slice(0, 300)}`);
  const question = asked.question.questions[0];
  await json(await post(app, `/api/threads/${thread.id}/questions/${asked.question.id}`,
    { answers: asked.question.questions.map((item, index) => index === 0 ? { selected: [], other: 'Python' } : { selected: [item.options[0].label] }) }));
  const done = await settled();
  console.log('ANSWER:', done.messages.at(-1).text.slice(0, 300).replaceAll('\n', ' | '));
  assert.match(done.messages.at(-1).text, /print\(/);
  assert.ok(question.options.length >= 2);
});
