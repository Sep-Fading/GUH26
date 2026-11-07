// Checks files and pictures added to a message: what each provider receives, what stays in the
// conversation, the limits, and what happens to the files when the conversation is deleted.
import assert from 'node:assert/strict';
// Registers the attachment tests.
import { test } from 'node:test';
// Reads what reached a provider or a folder.
import { mkdtemp, readFile, readdir, realpath, rm } from 'node:fs/promises';
// Locates temporary folders and the fake CLI.
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Names the fake conversation and its message.
import { THREAD, TURN, CLIENT } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
// A one-pixel PNG: enough for MiniClaw to recognise a picture by its first bytes.
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');
const NOTES = Buffer.from('alpha,beta\n1,2\n');
const BINARY = Buffer.from([0x50, 0x4b, 0x03, 0x04, 0x00, 0xff, 0xfe, 0x00, 0x01]);

/** Uploads one file to a conversation and returns what MiniClaw made of it. */
const upload = async (app, thread, name, bytes, status = 201) => json(await app.request(
  `/api/threads/${thread}/attachments?name=${encodeURIComponent(name)}`,
  { method: 'POST', body: bytes, headers: { 'Content-Type': 'application/octet-stream', 'X-MiniClaw-Request': '1' } }), status);
const last = async (app, method) => (await app.inspect()).requests.filter(request => request.method === method).at(-1).params;

/** Starts a backend and one plain conversation on this Mac. */
async function fixture(t, env = {}) {
  const trash = await mkdtemp(join(tmpdir(), 'mc-attach-trash-'));
  const app = await launchApp({}, { env: { MINICLAW_TRASH_DIR: trash, ...env } });
  t.after(async () => { await app.close(); await rm(trash, { recursive: true, force: true }); });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  return Object.assign(app, { trash });
}

test('a picture reaches Codex as a picture and stays in the conversation', async t => {
  const app = await fixture(t);
  const { attachment } = await upload(app, THREAD, 'tree.png', PNG);
  assert.deepEqual([attachment.name, attachment.kind, attachment.mediaType, attachment.size], ['tree.png', 'image', 'image/png', PNG.length]);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Match this tree', clientMessageId: CLIENT, attachments: [attachment.id] }), 202);
  const { input } = await last(app, 'turn/start');
  const picture = input.find(item => item.type === 'localImage');
  assert.deepEqual(await readFile(picture.path), PNG);
  assert.deepEqual(input.filter(item => item.type === 'text').map(item => item.text), ['Match this tree']);
  const shown = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(shown.messages.at(-1).attachments, [{ id: attachment.id, name: 'tree.png', kind: 'image', mediaType: 'image/png', size: PNG.length }]);
  const served = await app.request(`/api/threads/${THREAD}/attachments/${attachment.id}`);
  assert.deepEqual([served.status, served.headers.get('content-type'), served.headers.get('x-content-type-options')], [200, 'image/png', 'nosniff']);
  assert.deepEqual(Buffer.from(await served.arrayBuffer()), PNG);
});

test('a text file is read into the turn, and the message stays as typed', async t => {
  const app = await fixture(t);
  const { attachment } = await upload(app, THREAD, 'notes.csv', NOTES);
  assert.equal(attachment.kind, 'text');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Sum the columns', clientMessageId: CLIENT, attachments: [attachment.id] }), 202);
  const turn = await last(app, 'turn/start');
  assert.deepEqual(turn.input, [{ type: 'text', text: 'Sum the columns', text_elements: [] }]);
  const note = turn.additionalContext.miniclaw_attachments;
  assert.equal(note.kind, 'application');
  assert.ok(note.value.includes('notes.csv') && note.value.includes('alpha,beta\n1,2'), note.value);
  // A page never runs an uploaded file: anything that is not a picture is a download.
  const served = await app.request(`/api/threads/${THREAD}/attachments/${attachment.id}`);
  assert.equal(served.headers.get('content-type'), 'application/octet-stream');
  assert.match(served.headers.get('content-disposition'), /^attachment/);
});

test('a plain conversation keeps other files and names them to the model', async t => {
  const app = await fixture(t);
  const { attachment } = await upload(app, THREAD, 'scene.blend', BINARY);
  assert.equal(attachment.kind, 'file');
  // A picture drawn in markup could carry script, so it is never treated as a picture.
  assert.equal((await upload(app, THREAD, 'logo.svg', Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>'))).attachment.kind, 'text');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Keep this', clientMessageId: CLIENT, attachments: [attachment.id] }), 202);
  const note = (await last(app, 'turn/start')).additionalContext.miniclaw_attachments.value;
  assert.match(note, /scene\.blend/);
  assert.match(note, /9 bytes/);
  assert.match(note, /cannot open/i);
});

test('a project conversation also gets every upload in its working folder', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-attach-root-')));
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build things.', active: true }), 201);
  const { project } = await json(await post(app, '/api/projects', { name: 'Demo', environment: { kind: 'host' }, workspace: root, agentIds: [agent.id] }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: agent.id }), 201);
  const ids = [];
  for (const [name, bytes] of [['scene.blend', BINARY], ['tree.png', PNG], ['scene.blend', NOTES]]) ids.push((await upload(app, THREAD, name, bytes)).attachment.id);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Open the scene', clientMessageId: CLIENT, attachments: ids }), 202);
  // A second file of the same name never replaces the first.
  assert.deepEqual((await readdir(join(root, 'uploads'))).sort(), ['scene 2.blend', 'scene.blend', 'tree.png']);
  assert.deepEqual(await readFile(join(root, 'uploads', 'scene.blend')), BINARY);
  const note = (await last(app, 'turn/start')).additionalContext.miniclaw_attachments.value;
  for (const path of ['uploads/scene.blend', 'uploads/scene 2.blend', 'uploads/tree.png']) assert.ok(note.includes(path), path);
});

test('limits and ownership are enforced', async t => {
  const app = await fixture(t, { MINICLAW_TEST_UPLOAD_BYTES: '4096' });
  const send = (attachments, text = 'Hi', id = crypto.randomUUID()) => post(app, `/api/threads/${THREAD}/messages`, { text, clientMessageId: id, attachments });
  assert.equal((await upload(app, THREAD, 'big.bin', Buffer.alloc(4097), 413)).error.code, 'attachment_too_large');
  for (const name of ['', '../up.txt', 'a/b.txt', '.hidden', 'x'.repeat(256)]) await upload(app, THREAD, name, NOTES, 400);
  await upload(app, THREAD, 'empty.txt', Buffer.alloc(0), 400);
  await upload(app, '019a0000-0000-7000-8000-000000000000', 'notes.csv', NOTES, 404);
  const ids = [];
  for (let index = 0; index < 11; index += 1) ids.push((await upload(app, THREAD, `n${index}.txt`, NOTES)).attachment.id);
  assert.equal((await send(ids)).status, 400);
  assert.equal((await send([ids[0], ids[0]])).status, 400);
  assert.equal((await send([crypto.randomUUID()])).status, 400);
  // An upload that was never sent can be taken back; one that was sent cannot be sent again.
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${ids[10]}`, { method: 'DELETE', body: {} })).status, 200);
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${ids[10]}`)).status, 404);
  // A message may be attachments alone.
  await json(await send([ids[0]], '', CLIENT), 202);
  // The provider still gets words: the names of what was attached.
  assert.match((await last(app, 'turn/start')).input[0].text, /n0\.txt/);
  // The same send again is the same turn; the upload cannot join another message or be removed.
  await json(await send([ids[0]], '', CLIENT), 202);
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${ids[0]}`, { method: 'DELETE', body: {} })).status, 409);
});

test('deleting the conversation moves its uploads to the Trash', async t => {
  const app = await fixture(t);
  const { attachment } = await upload(app, THREAD, 'tree.png', PNG);
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${attachment.id}`)).status, 404);
  const moved = await eventually(() => readdir(app.trash), names => names.length === 1, 'uploads in the Trash');
  assert.match(moved[0], /MiniClaw uploads/);
});

test('Claude receives the picture as a picture and the text file as text', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-attach-claude-'));
  const journal = join(folder, 'calls.jsonl');
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: journal } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  const picture = (await upload(app, thread.id, 'tree.png', PNG)).attachment;
  const notes = (await upload(app, thread.id, 'notes.csv', NOTES)).attachment;
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Match this tree', clientMessageId: crypto.randomUUID(), model: MODEL,
    attachments: [picture.id, notes.id] }), 202);
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  assert.deepEqual(done.messages.find(message => message.role === 'user').attachments.map(item => item.name), ['tree.png', 'notes.csv']);
  const call = (await readFile(journal, 'utf8')).trim().split('\n').map(line => JSON.parse(line)).at(-1);
  assert.equal(call.args[call.args.indexOf('--input-format') + 1], 'stream-json');
  // Pictures come first, as Anthropic's vision guide advises, then the words.
  assert.deepEqual(call.content.map(block => block.type), ['image', 'text']);
  assert.deepEqual(call.content[0].source, { type: 'base64', media_type: 'image/png', data: PNG.toString('base64') });
  assert.ok(call.content[1].text.includes('alpha,beta') && call.content[1].text.endsWith('Match this tree'), call.content[1].text);
});

test('a queued message leaves with its uploads, and taking it back removes them', async t => {
  const app = await fixture(t);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'First', clientMessageId: CLIENT }), 202);
  const picture = (await upload(app, THREAD, 'tree.png', PNG)).attachment;
  const dropped = (await upload(app, THREAD, 'notes.csv', NOTES)).attachment;
  const enqueue = (text, attachments) => post(app, `/api/threads/${THREAD}/queue`, { text, clientMessageId: crypto.randomUUID(), attachments });
  const { item } = await json(await enqueue('Second', [picture.id]), 201);
  assert.deepEqual(item.attachments.map(upload => upload.name), ['tree.png']);
  const taken = (await json(await enqueue('Third', [dropped.id]), 201)).item;
  await json(await post(app, `/api/threads/${THREAD}/queue/${taken.id}/delete`));
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${dropped.id}`)).status, 404);
  const turn = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  const starts = await eventually(async () => (await app.inspect()).requests.filter(request => request.method === 'turn/start'),
    found => found.length === 2, 'the queued turn');
  assert.deepEqual(starts[1].params.input.map(part => part.type), ['localImage', 'text']);
});
