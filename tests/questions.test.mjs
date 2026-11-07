// Checks questions from the model: the tool's shape, the answers it gets back, what happens when
// nobody answers, "Answer in chat", and the switch that turns asking off.
import assert from 'node:assert/strict';
// Registers the question tests.
import { test } from 'node:test';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Names the fake conversation and its turn.
import { THREAD, TURN, CLIENT } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const text = result => result.contentItems.map(item => item.text).join('\n');
const snapshot = async app => json(await app.request(`/api/threads/${THREAD}`));
const LANGUAGE = { question: 'Which language should the server use?', header: 'Language', multiSelect: false,
  options: [{ label: 'TypeScript', description: 'The official SDK for Node.' }, { label: 'Python', description: 'The official mcp package.' }] };
const FEATURES = { question: 'Which tools should it have?', header: 'Tools', multiSelect: true,
  options: [{ label: 'Search', description: 'Find notes.' }, { label: 'Create', description: 'Add a note.' }, { label: 'Delete', description: 'Remove a note.' }] };

/** Starts a plain conversation whose first response is running. */
async function fixture(t, env = {}) {
  const app = await launchApp({}, { env });
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  return app;
}

/** Sends the tool call as the model would; the reply arrives once the user has answered. */
const ask = (app, id, questions) => app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD, turnId: TURN,
  callId: id, arguments: { questions } } });
const reply = async (app, id) => (await eventually(() => app.inspect(), value => value.responses.some(response => response.id === id),
  `tool reply ${id}`, 10000)).responses.find(response => response.id === id).result;
const open = async app => (await eventually(() => snapshot(app), value => value.question, 'question popup')).question;

test('every Mac conversation is offered the tool, and its answers come back as the result', async t => {
  const app = await fixture(t);
  const start = (await app.inspect()).requests.find(request => request.method === 'thread/start');
  const tool = start.params.dynamicTools.find(tool => tool.name === 'miniclaw_ask');
  assert.equal(tool.type, 'function');
  assert.match(tool.description, /own answer/i);
  assert.deepEqual([tool.inputSchema.properties.questions.minItems, tool.inputSchema.properties.questions.maxItems], [1, 4]);

  await ask(app, 'ask-1', [LANGUAGE, FEATURES]);
  const question = await open(app);
  assert.deepEqual(question.questions.map(item => [item.header, item.multiSelect, item.options.length]), [['Language', false, 2], ['Tools', true, 3]]);
  assert.ok(question.expiresInSeconds > 1700 && question.expiresInSeconds <= 1800);
  // One pick for the first, two picks and the user's own words for the second.
  for (const bad of [{ answers: [{ selected: ['TypeScript'] }] }, { answers: [{ selected: ['TypeScript', 'Python'] }, { selected: ['Search'] }] },
    { answers: [{ selected: ['Rust'] }, { selected: ['Search'] }] }, { answers: [{ selected: [] }, { selected: ['Search'] }] },
    { answers: [{ selected: [], other: 'x'.repeat(4001) }, { selected: ['Search'] }] }]) {
    assert.equal((await post(app, `/api/threads/${THREAD}/questions/${question.id}`, bad)).status, 400, JSON.stringify(bad).slice(0, 80));
  }
  await json(await post(app, `/api/threads/${THREAD}/questions/${question.id}`,
    { answers: [{ selected: ['Python'] }, { selected: ['Search', 'Create'], other: 'Also export to Markdown' }] }));
  const result = await reply(app, 'ask-1');
  assert.equal(result.success, true);
  assert.match(text(result), /Which language should the server use\?\s*\n?.*Python/);
  assert.match(text(result), /Search, Create/);
  assert.match(text(result), /Also export to Markdown/);
  assert.equal((await snapshot(app)).question, null);
  // It is shown in Activity with the questions and the answers.
  const entry = (await snapshot(app)).activities.find(item => item.id === 'ask-1');
  assert.deepEqual([entry.kind, entry.status], ['question', 'ok']);
  assert.match(entry.output, /Python/);
  // The time it took is how long the user was given, not the instant it was written down.
  assert.ok(entry.endedAt > entry.startedAt, `${entry.startedAt} to ${entry.endedAt}`);
  // An answer of the user's own words alone is an answer.
  await ask(app, 'ask-2', [LANGUAGE]);
  const second = await open(app);
  await json(await post(app, `/api/threads/${THREAD}/questions/${second.id}`, { answers: [{ selected: [], other: 'Go, please' }] }));
  assert.match(text(await reply(app, 'ask-2')), /Go, please/);
});

test('a malformed question is explained to the model and nothing is shown', async t => {
  const app = await fixture(t);
  for (const [id, questions] of [['bad-1', []], ['bad-2', [{ ...LANGUAGE, options: [LANGUAGE.options[0]] }]], ['bad-3', Array(5).fill(LANGUAGE)],
    ['bad-4', [{ ...LANGUAGE, question: '' }]], ['bad-5', 'which?']]) {
    await ask(app, id, questions);
    const result = await reply(app, id);
    assert.equal(result.success, false, id);
    assert.match(text(result), /1 to 4 questions/);
  }
  assert.equal((await snapshot(app)).question, null);
});

test('"Answer in chat" stops the response, and so does 30 minutes without an answer', async t => {
  const app = await fixture(t, { MINICLAW_TEST_MINUTE_MS: '20' });
  await ask(app, 'ask-chat', [LANGUAGE]);
  const question = await open(app);
  await json(await post(app, `/api/threads/${THREAD}/questions/${question.id}`, { action: 'chat' }));
  await eventually(() => app.inspect(), value => value.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
  assert.equal((await snapshot(app)).question, null);
  assert.equal((await reply(app, 'ask-chat')).success, false);
  // A second conversation turn: nobody answers, and the turn is ended rather than left waiting.
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  await eventually(() => snapshot(app), value => value.activeTurn === null, 'turn end');
  const turn = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: crypto.randomUUID() }), 202);
  await app.emit({ id: 'ask-late', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD, turnId: 'turn-2', callId: 'ask-late',
    arguments: { questions: [LANGUAGE] } } });
  await eventually(() => app.inspect(), value => value.requests.filter(request => request.method === 'turn/interrupt').length === 2, 'interrupt after the wait', 8000);
  const late = await reply(app, 'ask-late');
  assert.equal(late.success, false);
  assert.match(text(late), /did not answer/i);
  const entry = (await snapshot(app)).activities.find(item => item.id === 'ask-late');
  assert.equal(entry.status, 'refused');
});

test('asking can be switched off for one conversation and back on', async t => {
  const app = await fixture(t);
  const capabilities = async () => (await json(await app.request(`/api/capabilities?thread=${THREAD}`))).capabilities;
  assert.deepEqual((await capabilities()).filter(item => item.key === 'questions').map(item => [item.name, item.allowed, item.on]), [['Ask me questions', true, true]]);
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'questions', on: false }));
  assert.equal((await capabilities()).find(item => item.key === 'questions').on, false);
  await ask(app, 'ask-off', [LANGUAGE]);
  const result = await reply(app, 'ask-off');
  assert.equal(result.success, false);
  assert.match(text(result), /switched questions off/i);
  assert.equal((await snapshot(app)).question, null);
  // Switching it back on removes a rule, which ends the running turn as every removal does; the next turn may ask.
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'questions', on: true }));
  assert.equal((await capabilities()).find(item => item.key === 'questions').on, true);
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  await eventually(() => snapshot(app), value => value.activeTurn === null, 'turn end');
  const turn = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: crypto.randomUUID() }), 202);
  await app.emit({ id: 'ask-on', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD, turnId: 'turn-2', callId: 'ask-on',
    arguments: { questions: [LANGUAGE] } } });
  assert.ok(await open(app));
});

test('a Claude conversation asks through the same popup', async t => {
  const { mkdtemp, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { fileURLToPath } = await import('node:url');
  const folder = await mkdtemp(join(tmpdir(), 'mc-ask-claude-'));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)),
    MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: `ask ${JSON.stringify({ questions: [LANGUAGE] })}`,
    clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  const waiting = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)), value => value.question, 'Claude question', 15000);
  await json(await post(app, `/api/threads/${thread.id}/questions/${waiting.question.id}`, { answers: [{ selected: ['TypeScript'] }] }));
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  assert.match(done.messages.at(-1).text, /^miniclaw_ask error=false: .*TypeScript/s);
});
