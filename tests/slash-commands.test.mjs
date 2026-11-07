// Checks slash commands: which exist, and that a message starting with one carries that
// skill's instructions for that one turn while the typed text is sent unchanged.
import assert from 'node:assert/strict';
// Registers the slash command tests.
import { test } from 'node:test';
// Names further messages.
import { randomUUID } from 'node:crypto';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Names the fake conversation and ends its turn.
import { THREAD, CLIENT, turnDone } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const commands = async app => (await json(await app.request('/api/slash-commands'))).commands;

/** Sends one message and returns what the provider was given for that turn. */
async function turn(app, text, clientMessageId = randomUUID()) {
  const before = (await app.inspect()).requests.filter(request => request.method === 'turn/start').length;
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text, clientMessageId }), 202);
  const state = await eventually(() => app.inspect(),
    value => value.requests.filter(request => request.method === 'turn/start').length > before, 'turn start');
  const params = state.requests.filter(request => request.method === 'turn/start').at(-1).params;
  await app.emit(turnDone());
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  // A called skill travels under its own key, for that turn only. Measured with the real Codex
  // (tests/experiments/skill-context-cost.test.mjs): an unchanged key costs nothing, a changed one is
  // added again in full, so a one-turn extra must not make the conversation's skills block change.
  return { skills: params.additionalContext.miniclaw_slash?.value ?? '', base: params.additionalContext.miniclaw_skills.value,
    text: params.input[0].text };
}

test('the built-in /mcp-create and the user\'s callable skills are listed', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await commands(app)).map(command => [command.name, command.builtin]), [['mcp-create', true], ['skill-create', true], ['agent-create', true]]);
  const skill = { name: 'Release Notes', description: 'Writes release notes.', instructions: 'Write release notes from the git log.' };
  await json(await post(app, '/api/skills', { ...skill, active: true, slash: true }), 201);
  await json(await post(app, '/api/skills', { ...skill, name: 'Hidden', active: true }), 201);
  await json(await post(app, '/api/skills', { ...skill, name: 'Asleep', active: false, slash: true }), 201);
  // A name that would collide with a built-in is not offered twice.
  await json(await post(app, '/api/skills', { ...skill, name: 'MCP create', active: true, slash: true }), 201);
  assert.deepEqual((await commands(app)).map(command => [command.name, command.builtin, command.description]),
    [['mcp-create', true, 'Guides you through building an MCP server, tests it, and hands it over for you to add.'],
      ['skill-create', true, 'Writes a new skill, or a better version of one you have, and hands it over for you to review.'],
      ['agent-create', true, 'Writes a new agent, or a better version of one you have, and hands it over for you to review.'],
      ['release-notes', false, 'Writes release notes.']]);
});

test('/mcp-create attaches its instructions to that turn only and sends the text as typed', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const plain = await turn(app, 'Hello', CLIENT);
  assert.doesNotMatch(plain.skills, /miniclaw-mcp-server/);
  const first = await turn(app, '/mcp-create a server that searches my notes');
  assert.equal(first.text, '/mcp-create a server that searches my notes');
  for (const must of [/TypeScript/, /Python/, /ask .*which language/i, /@modelcontextprotocol\/server/, /mcp\[cli\]/, /stdout/,
    /```miniclaw-mcp-server/, /cannot add/i, /never ask .* value/i]) assert.match(first.skills, must);
  assert.equal(first.base, plain.base, "the conversation's own skills block is the same with and without a command");
  const after = await turn(app, 'Python please');
  assert.doesNotMatch(after.skills, /miniclaw-mcp-server/, 'the next turn is an ordinary one');
  // The instructions stay in force while the work continues: the command can be given again.
  assert.match((await turn(app, '/mcp-create')).skills, /miniclaw-mcp-server/);
});

test('a callable skill runs by its slash name beside the conversation\'s own skills; an unknown command is plain text', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/skills', { name: 'Release Notes', instructions: 'Write release notes from the git log.', active: true, slash: true }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const called = await turn(app, '/release-notes for 0.2.0', CLIENT);
  assert.match(called.skills, /Write release notes from the git log\./);
  assert.match(called.skills, /Release Notes/);
  assert.doesNotMatch(called.base, /Write release notes/);
  const unknown = await turn(app, '/nonsense do it');
  assert.equal(unknown.text, '/nonsense do it');
  assert.doesNotMatch(unknown.skills, /release notes/i);
  // A path is not a command.
  assert.doesNotMatch((await turn(app, '/release-notes/old is a folder')).skills, /release notes/i);
});

test('/skill-create and /agent-create know what exists, show one item when it is named, and hand over a block', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, '/api/skills', { name: 'Release Notes', description: 'Writes release notes.', instructions: 'SECRET-SKILL-TEXT from the git log.', active: true }), 201);
  await json(await post(app, '/api/skills', { name: 'Tone', description: 'Plain words.', instructions: 'OTHER-SKILL-TEXT', active: false }), 201);
  await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'SECRET-AGENT-TEXT build things.', active: true }), 201);

  const skill = (await turn(app, '/skill-create one that reviews pull requests', CLIENT)).skills;
  for (const must of [/```miniclaw-skill/, /"instructions"/, /cannot save/i, /miniclaw_ask/, /Release Notes: Writes release notes\./, /Tone: Plain words\./]) assert.match(skill, must);
  // Instructions of what exists are not handed out wholesale.
  assert.doesNotMatch(skill, /SECRET-SKILL-TEXT|OTHER-SKILL-TEXT/);
  // Naming one is asking for it, in any letter case.
  const edit = (await turn(app, '/skill-create make release notes shorter')).skills;
  assert.match(edit, /SECRET-SKILL-TEXT/);
  assert.doesNotMatch(edit, /OTHER-SKILL-TEXT/);
  assert.match(edit, /"replaces"/);

  const agent = (await turn(app, '/agent-create a reviewer')).skills;
  for (const must of [/```miniclaw-agent/, /"permissions"/, /"reason"/, /approves? each/i, /cannot save/i, /Builder/, /Release Notes/, /fixture-model|Model A/]) assert.match(agent, must);
  assert.doesNotMatch(agent, /SECRET-AGENT-TEXT/);
  assert.match((await turn(app, '/agent-create improve the builder agent')).skills, /SECRET-AGENT-TEXT/);
});

test('for Claude a called skill goes with the message, so the cached system prompt does not change', async t => {
  const { mkdtemp, readFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { fileURLToPath } = await import('node:url');
  const folder = await mkdtemp(join(tmpdir(), 'mc-slash-claude-'));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)),
    MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  const runs = async () => (await readFile(join(folder, 'calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  const say = async text => {
    await json(await post(app, `/api/threads/${thread.id}/messages`, { text, clientMessageId: randomUUID(), model: MODEL }), 202);
    await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
      value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
    const run = (await runs()).at(-1);
    return { prompt: run.prompt, system: run.args[run.args.indexOf('--system-prompt') + 1] };
  };
  const plain = await say('Hello');
  const called = await say('/mcp-create a notes server');
  assert.match(called.prompt, /miniclaw-mcp-server/);
  assert.match(called.prompt, /\/mcp-create a notes server$/);
  assert.equal(called.system, plain.system, 'the system prompt is the same with and without a command');
  // What the user sees is what they typed.
  const { messages } = await json(await app.request(`/api/threads/${thread.id}`));
  assert.deepEqual(messages.filter(message => message.role === 'user').map(message => message.text), ['Hello', '/mcp-create a notes server']);
});
