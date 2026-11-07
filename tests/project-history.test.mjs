// Checks what a project conversation learns of the project's earlier conversations: the digest
// told once with the first message, the summaries agents save, and the bounded look-up.
import assert from 'node:assert/strict';
// Registers the history tests.
import { test } from 'node:test';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Names the fake conversations and builds their events.
import { THREAD, OTHER, TURN, started, message } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const text = result => result.contentItems.map(item => item.text).join('\n');
const THIRD = '019a4444-4444-7444-8444-444444444444';

/** Calls the tool as the model would and returns its result. */
async function call(app, thread, args, id = crypto.randomUUID()) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_project', threadId: thread, turnId: TURN, callId: id, arguments: args } });
  return (await eventually(() => app.inspect(), value => value.responses.some(response => response.id === id), `tool reply ${id}`, 10000))
    .responses.find(response => response.id === id).result;
}

/** Starts a conversation under its own id, in a project or outside one. */
async function open(app, id, projectId) {
  await app.set('thread/start', { result: started(app.peer.cwd, { id }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...(projectId ? { projectId } : {}) }), 201);
}

/** Sends one message, lets the fake answer, ends the turn, and returns what the project note said. */
async function exchange(app, thread, words, answer) {
  // The fake Codex names every turn alike unless told otherwise, and a finished turn's id is spent.
  const turn = { id: crypto.randomUUID(), status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: thread, turn } }] });
  const before = (await app.inspect()).requests.filter(request => request.method === 'turn/start').length;
  await json(await post(app, `/api/threads/${thread}/messages`, { text: words, clientMessageId: crypto.randomUUID() }), 202);
  const state = await eventually(() => app.inspect(), value => value.requests.filter(request => request.method === 'turn/start').length > before, 'turn');
  const sent = state.requests.filter(request => request.method === 'turn/start').at(-1).params.additionalContext.miniclaw_project?.value ?? null;
  await app.emit({ method: 'item/completed', params: { threadId: thread, turnId: turn.id, completedAtMs: 1900000000000, item: message(answer, crypto.randomUUID()) } },
    { method: 'turn/completed', params: { threadId: thread, turn: { ...turn, status: 'completed' } } });
  await eventually(async () => json(await app.request(`/api/threads/${thread}`)), value => value.activeTurn === null && value.messages.at(-1)?.role === 'assistant', 'turn end');
  return sent;
}

test('a new conversation is told the project\'s earlier conversations once, and can look them up', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' } }), 201);
  await open(app, THREAD, project.id);
  assert.equal(await exchange(app, THREAD, 'Make a low-poly tree', 'The trunk uses the hexagon profile HEX-DETAIL.'), null, 'a first conversation has nothing to be told');
  await json(await post(app, `/api/threads/${THREAD}/name`, { name: 'Roblox tree' }));
  const saved = await call(app, THREAD, { action: 'summary_save', text: 'Built a 280-triangle tree; the user wants flat colours.' });
  assert.equal(saved.success, true);
  assert.equal((await call(app, THREAD, { action: 'summary_save', text: 'x'.repeat(601) })).success, false);
  assert.equal((await call(app, THREAD, { action: 'summary_save', text: 'token: abcdefghijklmnopqrstuvwxyz' })).success, false);

  await open(app, OTHER, project.id);
  const told = await exchange(app, OTHER, 'Now a rock', 'A rock it is.');
  assert.match(told, /## Earlier conversations in this project/);
  assert.match(told, new RegExp(`Roblox tree \\(id ${THREAD}\\): Built a 280-triangle tree`));
  assert.doesNotMatch(told, /HEX-DETAIL/, 'the digest holds summaries, never transcripts');
  assert.doesNotMatch(told, /Now a rock/, 'a conversation is not told of itself');
  assert.equal(await exchange(app, OTHER, 'And a bush', 'A bush.'), null, 'the digest is a snapshot, told once');

  const listed = text(await call(app, OTHER, { action: 'conversations_list' }));
  assert.match(listed, /Roblox tree/);
  assert.doesNotMatch(listed, new RegExp(OTHER), 'the asking conversation is left out');
  const found = text(await call(app, OTHER, { action: 'conversations_search', query: 'hex-detail' }));
  assert.match(found, new RegExp(`${THREAD}.*Roblox tree`));
  assert.match(found, /hexagon profile HEX-DETAIL/);
  // Every word has to be there, in any order; an exact phrase is not needed.
  assert.match(text(await call(app, OTHER, { action: 'conversations_search', query: 'profile TRUNK hex-detail' })), /hexagon profile HEX-DETAIL/);
  assert.match(text(await call(app, OTHER, { action: 'conversations_search', query: 'trunk pentagon' })), /nothing/i);
  assert.match(text(await call(app, OTHER, { action: 'conversations_search', query: 'no such words' })), /nothing/i);
  const read = text(await call(app, OTHER, { action: 'conversation_read', id: THREAD }));
  assert.match(read, /User: Make a low-poly tree/);
  assert.match(read, /Assistant: The trunk uses the hexagon profile HEX-DETAIL\./);
  assert.match(read, /page 1 of 1/i);
  // What was looked up is shown to the user.
  const titles = (await json(await app.request(`/api/threads/${OTHER}`))).activities.filter(entry => entry.kind === 'memory').map(entry => entry.title);
  assert.ok(titles.includes('Searched conversations for: hex-detail') && titles.includes('Read conversation: Roblox tree'), titles.join(' | '));
});

test('look-up stays inside the project and is bounded', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' } }), 201);
  const { project: other } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' } }), 201);
  await open(app, THREAD, project.id);
  await exchange(app, THREAD, 'Long one', `START ${'word '.repeat(4000)} END-OF-LONG`);
  await open(app, OTHER, other.id);
  await exchange(app, OTHER, 'Elsewhere', 'SECRET-OF-THE-OTHER-PROJECT');
  await open(app, THIRD, project.id);
  await exchange(app, THIRD, 'Hello', 'Hi.');
  assert.equal((await call(app, THIRD, { action: 'conversation_read', id: OTHER })).success, false, 'another project\'s conversation');
  assert.equal((await call(app, THIRD, { action: 'conversation_read', id: crypto.randomUUID() })).success, false);
  assert.match(text(await call(app, THIRD, { action: 'conversations_search', query: 'SECRET-OF-THE-OTHER' })), /nothing/i);
  const first = text(await call(app, THIRD, { action: 'conversation_read', id: THREAD }));
  assert.ok(first.length <= 9000, `one page, got ${first.length}`);
  assert.match(first, /page 1 of 3/i);
  assert.doesNotMatch(first, /END-OF-LONG/);
  assert.match(text(await call(app, THIRD, { action: 'conversation_read', id: THREAD, page: 3 })), /END-OF-LONG/);
  assert.equal((await call(app, THIRD, { action: 'conversation_read', id: THREAD, page: 4 })).success, false);
  for (const query of ['', 'ab', 'x'.repeat(201)]) assert.equal((await call(app, THIRD, { action: 'conversations_search', query })).success, false, query.slice(0, 5));
  // Deleting a conversation takes its copy and its summary with it.
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  assert.doesNotMatch(text(await call(app, THIRD, { action: 'conversations_list' })), new RegExp(THREAD));
});
