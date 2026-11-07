// Real models, the two creating commands. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-create.test.mjs
// It checks what no fake can: that a real model ends with a block MiniClaw can read, for a skill
// and for an agent. Add MINICLAW_PROBE_CLAUDE=1 to run the same through Claude Code.
import assert from 'node:assert/strict';
// Registers the opt-in experiment.
import { test } from 'node:test';
// Finds the installed Claude CLI.
import { execFileSync } from 'node:child_process';
// Starts MiniClaw against the installed providers and their existing sign-ins.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

/** Sends one command, answers any question with its first option, and returns the parsed block. */
async function handsOver(app, choice, text, kind) {
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...choice }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text, clientMessageId: crypto.randomUUID(), ...choice }), 202);
  for (;;) {
    const state = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
      value => value.question || (!value.activeTurn && value.messages.at(-1)?.role === 'assistant'), 'a question or the reply', 180000);
    if (!state.question) {
      const reply = state.messages.at(-1).text;
      const found = reply.match(new RegExp('```' + kind + '\\n([\\s\\S]*?)\\n```'));
      assert.ok(found, `no ${kind} block in: ${reply.slice(0, 400)}`);
      return JSON.parse(found[1]);
    }
    console.log('ASKED:', state.question.questions.map(item => item.question).join(' / '));
    await json(await post(app, `/api/threads/${thread.id}/questions/${state.question.id}`,
      { answers: state.question.questions.map(item => ({ selected: [item.options[0].label] })) }));
  }
}

/** Runs both commands and checks the fields the page's parser requires. */
async function both(app, choice) {
  const skill = await handsOver(app, choice, '/skill-create a skill that makes commit messages follow Conventional Commits. Decide the details yourself.', 'miniclaw-skill');
  console.log(`${choice.model} SKILL:`, JSON.stringify(skill).slice(0, 300));
  assert.ok(skill.name.length <= 80 && skill.instructions.length > 40 && skill.instructions.length <= 16000);
  assert.equal(typeof (skill.slash ?? false), 'boolean');
  const agent = await handsOver(app, choice, '/agent-create an agent that reviews the code in /private/tmp/site and may only read it. Decide the details yourself.', 'miniclaw-agent');
  console.log(`${choice.model} AGENT:`, JSON.stringify({ ...agent, instructions: `${agent.instructions.slice(0, 80)}...` }));
  assert.ok(agent.name.length <= 80 && agent.instructions.length > 40);
  assert.ok(Array.isArray(agent.permissions) && agent.permissions.every(item => item.kind && item.reason));
  assert.ok(agent.permissions.some(item => item.kind === 'folder' && item.path === '/private/tmp/site' && item.action === 'read'), 'it suggests the folder it was told about');
  assert.equal(agent.permissions.some(item => item.action === 'write'), false, 'it does not ask for more than it was told');
}

test('a real Codex model hands over a skill and an agent', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 600000 }, async t => {
  const app = await launchApp({}, { real: true });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  await both(app, { model: 'gpt-5.6-luna', effort: 'low' });
});

test('a real Claude model does too', { skip: !(enabled && process.env.MINICLAW_PROBE_CLAUDE === '1') && 'set MINICLAW_PROBE_CLAUDE=1 as well', timeout: 600000 }, async t => {
  const app = await launchApp({}, { real: true, env: { CLAUDE_BIN: execFileSync('/bin/zsh', ['-lc', 'command -v claude'], { encoding: 'utf8' }).trim() } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/models')).json(), value => JSON.stringify(value).includes('claude-haiku'), 'the Claude models', 60000);
  await both(app, { model: 'claude-haiku-4-5-20251001', effort: 'low' });
});
