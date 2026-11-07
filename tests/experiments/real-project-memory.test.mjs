// Real models, a project that remembers. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-project-memory.test.mjs
// It checks what no fake can: that a real model saves a note when asked, that a NEW conversation
// of the project answers from it without being told, and that it finds a detail which exists
// only in the earlier conversation's transcript. Add MINICLAW_PROBE_CLAUDE=1 for Claude Code.
import assert from 'node:assert/strict';
// Registers the opt-in experiment.
import { test } from 'node:test';
// Finds the installed Claude CLI.
import { execFileSync } from 'node:child_process';
// Starts MiniClaw against the installed providers and their existing sign-ins.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

/** Runs the two-conversation scenario for one model. */
async function remembers(app, choice) {
  const { project } = await json(await post(app, '/api/projects', { name: `Game ${choice.model}`, environment: { kind: 'host' },
    instructions: 'End every reply with the word KIWI on its own line.' }), 201);
  const talk = async (thread, text) => {
    await json(await post(app, `/api/threads/${thread}/messages`, { text, clientMessageId: crypto.randomUUID(), ...choice }), 202);
    const done = await eventually(async () => json(await app.request(`/api/threads/${thread}`)),
      value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'the reply', 180000);
    return { reply: done.messages.at(-1).text, steps: done.activities.filter(entry => entry.kind === 'memory').map(entry => `${entry.title} [${entry.status}]`) };
  };
  const open = async () => (await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, ...choice }), 201)).thread.id;

  const first = await open();
  const told = await talk(first, 'Remember for this project: our art style is low-poly with exactly 8 colours, and trees stay under 280 triangles. '
    + 'Separately, and not worth remembering: the trunk profile\'s codename is HEX-ZEBRA-7. Then save a short summary of this conversation.');
  console.log(`${choice.model} A:`, told.steps.join(' | '), '=>', told.reply.replaceAll('\n', ' / ').slice(0, 200));
  assert.ok(told.steps.some(step => step.startsWith('Remembered:') && step.endsWith('[ok]')), 'it saved a note');
  assert.match(told.reply, /KIWI\s*$/, 'the project instructions reached it');
  const notes = (await json(await app.request(`/api/projects/${project.id}/memory`))).notes;
  console.log(`${choice.model} NOTES:`, JSON.stringify(notes.map(note => [note.name, note.description])));

  const second = await open();
  const colours = await talk(second, 'Without asking me anything: how many colours does our art style allow? Answer with the number.');
  console.log(`${choice.model} B1:`, colours.steps.join(' | '), '=>', colours.reply.replaceAll('\n', ' / ').slice(0, 200));
  assert.match(colours.reply, /\b(8|eight)\b/i);
  const codename = await talk(second, 'In an earlier conversation of this project I mentioned a codename for the trunk profile. Find it and tell me.');
  console.log(`${choice.model} B2:`, codename.steps.join(' | '), '=>', codename.reply.replaceAll('\n', ' / ').slice(0, 200));
  assert.match(codename.reply, /HEX-ZEBRA-7/);
}

test('a real Codex model remembers across conversations of a project', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 900000 }, async t => {
  const app = await launchApp({}, { real: true });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  await remembers(app, { model: 'gpt-5.6-luna', effort: 'low' });
});

test('a real Claude model does too', { skip: !(enabled && process.env.MINICLAW_PROBE_CLAUDE === '1') && 'set MINICLAW_PROBE_CLAUDE=1 as well', timeout: 900000 }, async t => {
  const app = await launchApp({}, { real: true, env: { CLAUDE_BIN: execFileSync('/bin/zsh', ['-lc', 'command -v claude'], { encoding: 'utf8' }).trim() } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/models')).json(), value => JSON.stringify(value).includes('claude-haiku'), 'the Claude models', 60000);
  await remembers(app, { model: 'claude-haiku-4-5-20251001', effort: 'low' });
});
