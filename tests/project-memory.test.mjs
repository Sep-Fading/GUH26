// Checks a project's shared memory: the tool that writes and reads it, the short index a
// conversation is told once, the switch, the limits, and the routes behind project settings.
import assert from 'node:assert/strict';
// Registers the memory tests.
import { test } from 'node:test';
// Locates the fake Claude CLI and its journal.
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Names the fake conversations and builds their start answers.
import { THREAD, OTHER, TURN, started, turnDone } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const text = result => result.contentItems.map(item => item.text).join('\n');

/** Starts a project with one conversation. */
async function fixture(t) {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' } }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
  return Object.assign(app, { project });
}

/** Calls the tool as the model would and returns its result. */
async function call(app, thread, args, id = crypto.randomUUID()) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_project', threadId: thread, turnId: TURN, callId: id, arguments: args } });
  return (await eventually(() => app.inspect(), value => value.responses.some(response => response.id === id), `tool reply ${id}`, 10000))
    .responses.find(response => response.id === id).result;
}

/** Sends one message, ends its turn, and returns the project note that went with it. */
async function note(app, thread, words) {
  const before = (await app.inspect()).requests.filter(request => request.method === 'turn/start').length;
  await json(await post(app, `/api/threads/${thread}/messages`, { text: words, clientMessageId: crypto.randomUUID() }), 202);
  const state = await eventually(() => app.inspect(), value => value.requests.filter(request => request.method === 'turn/start').length > before, 'turn');
  const sent = state.requests.filter(request => request.method === 'turn/start').at(-1).params.additionalContext.miniclaw_project?.value ?? null;
  await app.emit(turnDone());
  await eventually(async () => json(await app.request(`/api/threads/${thread}`)), value => value.activeTurn === null, 'turn end');
  return sent;
}

test('only a project conversation on this Mac is offered the tool, as one tool with actions', async t => {
  const app = await fixture(t);
  const starts = () => app.inspect().then(state => state.requests.filter(request => request.method === 'thread/start'));
  const tool = (await starts()).at(-1).params.dynamicTools.find(tool => tool.name === 'miniclaw_project');
  assert.equal(tool.type, 'function');
  assert.deepEqual(tool.inputSchema.properties.action.enum.slice(0, 3), ['memory_read', 'memory_save', 'memory_delete']);
  assert.match((await starts()).at(-1).params.baseInstructions, /miniclaw_project/);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const plain = (await starts()).at(-1).params;
  assert.equal(plain.dynamicTools.some(tool => tool.name === 'miniclaw_project'), false);
  assert.doesNotMatch(plain.baseInstructions, /miniclaw_project/);
  assert.equal(text(await call(app, OTHER, { action: 'memory_read', name: 'x' })).includes('not in a project'), true);
});

test('notes are saved, replaced, read and deleted, within limits and never with a secret', async t => {
  const app = await fixture(t);
  const saved = await call(app, THREAD, { action: 'memory_save', name: 'Art style', description: 'Low-poly, flat colours.', text: 'Trees under 300 triangles. Palette: 8 colours.' });
  assert.equal(saved.success, true);
  await call(app, THREAD, { action: 'memory_save', name: 'art style', description: 'Low-poly, flat colours.', text: 'Trees under 250 triangles.' });
  const read = await call(app, THREAD, { action: 'memory_read', name: 'Art style' });
  assert.match(text(read), /250 triangles/);
  assert.doesNotMatch(text(read), /300 triangles/, 'a save under the same name, in any case, replaces the note');
  const listed = (await json(await app.request(`/api/projects/${app.project.id}/memory`))).notes;
  assert.deepEqual(listed.map(item => [item.name, item.description]), [['Art style', 'Low-poly, flat colours.']]);
  for (const bad of [{ name: '', description: 'd', text: 't' }, { name: 'x'.repeat(81), description: 'd', text: 't' }, { name: 'n', description: 'x'.repeat(201), text: 't' },
    { name: 'n', description: 'd', text: 'x'.repeat(8193) }, { name: 'two\nlines', description: 'd', text: 't' }, { name: 'n', description: 'd' }]) {
    assert.equal((await call(app, THREAD, { action: 'memory_save', ...bad })).success, false, JSON.stringify(bad).slice(0, 60));
  }
  for (const secret of ['The key is sk-proj-abcdefghijklmnopqrstuvwxyz0123456789', 'ghp_abcdefghijklmnopqrstuvwxyz0123456789', '-----BEGIN OPENSSH PRIVATE KEY-----\nabc',
    'AKIAIOSFODNN7EXAMPLE', 'password: hunter2hunter2hunter2']) {
    const refused = await call(app, THREAD, { action: 'memory_save', name: 'Login', description: 'How to sign in.', text: secret });
    assert.equal(refused.success, false, secret.slice(0, 20));
    assert.match(text(refused), /secret/i);
  }
  assert.equal((await call(app, THREAD, { action: 'memory_read', name: 'Nothing' })).success, false);
  assert.equal((await call(app, THREAD, { action: 'memory_delete', name: 'ART STYLE' })).success, true);
  assert.deepEqual((await json(await app.request(`/api/projects/${app.project.id}/memory`))).notes, []);
  // What was remembered and forgotten is shown to the user.
  const activities = (await json(await app.request(`/api/threads/${THREAD}`))).activities.filter(entry => entry.kind === 'memory');
  assert.deepEqual(activities.filter(entry => entry.status === 'ok').map(entry => entry.title).slice(0, 2), ['Remembered: Art style', 'Remembered: art style']);
  assert.ok(activities.some(entry => entry.title === 'Forgot: ART STYLE'));
  assert.match(activities[0].output, /300 triangles/);
});

test('the index travels once, again only after another conversation changed it, and the switch stops it', async t => {
  const app = await fixture(t);
  await json(await post(app, `/api/projects/${app.project.id}/memory`, { name: 'Art style', description: 'Low-poly, flat colours.', text: 'FULL-TEXT-OF-THE-NOTE' }), 201);
  const first = await note(app, THREAD, 'Hello');
  assert.match(first, /Art style: Low-poly, flat colours\./);
  assert.doesNotMatch(first, /FULL-TEXT-OF-THE-NOTE/, 'only the index travels');
  assert.match(first, /memory_read/);
  assert.equal(await note(app, THREAD, 'Again'), null, 'an unchanged index is not sent again');
  // Its own writes do not come back to it.
  await call(app, THREAD, { action: 'memory_save', name: 'Scale', description: 'One unit is one metre.', text: 'Metres.' });
  assert.equal(await note(app, THREAD, 'And again'), null);
  // A change from outside, here by the user in settings, does.
  await json(await post(app, `/api/projects/${app.project.id}/memory`, { name: 'Engine', description: 'Roblox Studio.', text: 'Luau.' }), 201);
  const changed = await note(app, THREAD, 'Once more');
  for (const name of ['Art style', 'Scale', 'Engine']) assert.ok(changed.includes(name), name);
  // Changing only a note's text leaves the index as it was, so nobody is told it again.
  await json(await post(app, `/api/projects/${app.project.id}/memory/Engine`, { name: 'Engine', description: 'Roblox Studio.', text: 'Luau, strict mode.' }));
  assert.equal(await note(app, THREAD, 'Text only'), null);
  // Switched off, nothing is told and the tool refuses.
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: 'project_memory', on: false }));
  await json(await post(app, `/api/projects/${app.project.id}/memory`, { name: 'Later', description: 'Added while off.', text: 'x' }), 201);
  assert.equal(await note(app, THREAD, 'Off now'), null);
  assert.match(text(await call(app, THREAD, { action: 'memory_read', name: 'Art style' })), /switched off/i);
});

test('the user adds, edits and deletes notes in project settings, and they go with the project', async t => {
  const app = await fixture(t);
  const path = `/api/projects/${app.project.id}/memory`;
  await json(await post(app, path, { name: 'Engine', description: 'Roblox Studio.', text: 'Luau.' }), 201);
  assert.equal((await post(app, path, { name: 'Bad', description: 'd', text: 'ghp_abcdefghijklmnopqrstuvwxyz0123456789' })).status, 400);
  const { note: edited } = await json(await post(app, `${path}/Engine`, { name: 'Game engine', description: 'Roblox Studio.', text: 'Luau, strict mode.' }));
  assert.deepEqual([edited.name, edited.text], ['Game engine', 'Luau, strict mode.']);
  assert.equal((await post(app, `${path}/Nothing`, { name: 'n', description: 'd', text: 't' })).status, 404);
  await json(await post(app, `${path}/Game%20engine/delete`));
  assert.deepEqual((await json(await app.request(path))).notes, []);
  assert.equal((await app.request('/api/projects/019a0000-0000-7000-8000-000000000000/memory')).status, 404);
});

test('a Claude conversation gets the tool and the index with its message, not in the system prompt', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-memory-claude-'));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/projects/${project.id}/memory`, { name: 'Engine', description: 'Roblox Studio.', text: 'Luau.' }), 201);
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, model: MODEL }), 201);
  const say = async words => {
    await json(await post(app, `/api/threads/${thread.id}/messages`, { text: words, clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
    const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
      value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
    const run = (await readFile(join(folder, 'calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line)).at(-1);
    return { reply: done.messages.at(-1).text, prompt: run.prompt, system: run.args[run.args.indexOf('--system-prompt') + 1] };
  };
  const saved = await say(`project ${JSON.stringify({ action: 'memory_save', name: 'Scale', description: 'One unit is one metre.', text: 'Metres.' })}`);
  assert.match(saved.reply, /^miniclaw_project error=false/);
  assert.match(saved.prompt, /Engine: Roblox Studio\./);
  assert.doesNotMatch(saved.system, /Engine: Roblox Studio\./);
  const next = await say('Hello');
  assert.doesNotMatch(next.prompt, /Roblox Studio/, 'not told twice');
  assert.equal(next.system, saved.system);
  assert.deepEqual((await json(await app.request(`/api/projects/${project.id}/memory`))).notes.map(item => item.name).sort(), ['Engine', 'Scale']);
});
