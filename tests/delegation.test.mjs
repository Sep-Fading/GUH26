// Checks delegated runs end to end against the fake Codex: no paid inference.
import assert from 'node:assert/strict';
import { test } from 'node:test';
// Creates disposable files outside protected fixture storage.
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, win32 } from 'node:path';
import { launchApp, json, eventually } from './support/harness.mjs';
import { TOOLS_UNAVAILABLE, assertGate, closedRefusal } from './support/sandbox.mjs';
import { THREAD, TURN, OTHER, CLIENT, started, message, delta } from './support/fixtures.mjs';

const deniedHostFile = process.platform === 'win32'
  ? win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32/drivers/etc/hosts') : '/etc/hosts';

const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
const agent = async (app, extra) => (await json(await post(app, '/api/agents',
  { instructions: 'Do the work.', active: true, ...extra }), 201)).agent;

// Starts a chat with a lead agent that may delegate to Reviewer, with a turn in progress.
async function leadChat(t, delegation = { maxPerTurn: 2, minutes: 5 }, options = {}) {
  const app = await launchApp({}, options);
  t.after(() => app.close());
  const reviewer = await agent(app, { name: 'Reviewer', instructions: 'Review carefully.' });
  const outsider = await agent(app, { name: 'Outsider' });
  const lead = await agent(app, { name: 'Lead', delegatesTo: [reviewer.id], delegation });
  await json(await post(app, '/api/threads', { agentId: lead.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Ship it', clientMessageId: CLIENT }), 202);
  // The next thread the fake Codex creates is the delegated run's.
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
  return { app, reviewer, outsider, lead };
}
const snapshot = async app => json(await app.request(`/api/threads/${THREAD}`));
// Requirement change 2026-09-23: a hand-off carries a short summary for its card; the tests give one unless they test it.
const delegate = (app, id, args) => app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_delegate',
  threadId: THREAD, turnId: TURN, callId: id, arguments: { summary: 'Do the task', ...args } } });
const reply = async (app, id) => (await eventually(() => app.inspect(),
  value => value.responses.some(r => r.id === id), `tool reply ${id}`)).responses.find(r => r.id === id).result;
const text = result => result.contentItems.map(item => item.text).join('\n');
// Plays the delegated agent's whole turn in its own thread.
const childAnswers = (app, words) => app.emit(
  { method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } },
  { method: 'item/completed', params: { threadId: OTHER, turnId: TURN, completedAtMs: 1900000000000, item: message(words, 'child-1') } },
  { method: 'turn/completed', params: { threadId: OTHER, turn: { id: TURN, status: 'completed', items: [] } } });

// While Windows cannot enforce the sandbox, a delegated agent is offered no sandboxed tool: its
// dynamic tools lack miniclaw_scoped_native, and a call that names it anyway is refused at once with
// SANDBOX_UNAVAILABLE, asks nobody, and shows in the parent as the agent's refused step. Only those
// reports select the Windows expectations, so each test checks the real approval once the gate opens.
// Tells whether the delegated run's call `id` met the closed gate, and checks the refusal when it did.
async function childGateClosed(app, id) {
  const child = (await app.inspect()).requests.filter(r => r.method === 'thread/start')[1];
  const names = (child.params.dynamicTools ?? []).map(tool => tool.name);
  if (process.platform !== 'win32') return false;
  if (!await assertGate(app, !names.includes('miniclaw_scoped_native'))) return false;
  // A delegated run keeps the MCP gateway and, as on the Mac, is not offered the question tool.
  assert.deepEqual(names, ['miniclaw_mcp']);
  const refused = await reply(app, id);
  assert.equal(refused.success, false);
  assert.equal(text(refused), await closedRefusal(app, TOOLS_UNAVAILABLE));
  const seen = await eventually(() => snapshot(app), value => value.activities.some(a => a.id === id
    && a.status !== 'running'), 'refused step in the parent');
  const entry = seen.activities.find(a => a.id === id);
  assert.deepEqual([entry.agent, entry.status, entry.runId], ['Reviewer', 'refused', seen.delegations[0].id]);
  assert.deepEqual(seen.delegatedRequests.filter(item => item.kind !== 'question'), [], 'nobody is asked');
  assert.equal(seen.scopedApproval ?? null, null);
  return true;
}

test('only an agent with recipients is offered the delegation tool, and it names them', async t => {
  const { app } = await leadChat(t);
  const tools = (await app.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools;
  const tool = tools.find(tool => tool.name === 'miniclaw_delegate');
  assert.ok(tool, 'offered');
  assert.equal(tool.type, 'function', 'the real Codex rejects a dynamic tool without its type');
  assert.match(tool.description, /Reviewer/);
  assert.doesNotMatch(tool.description, /Outsider/);
});

// 2026-09-22: the user asked a plain conversation to hand a task to their Blender agent and it had no
// tool for it. Decision: a conversation with no agent (the main agent) may hand a task to any active
// agent, asked each time like every delegation, with the default limits.
test('a plain conversation may hand a task to any active agent, asked first, and names them to the model', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  // With no active agent there is nobody to hand anything to.
  await json(await post(app, '/api/threads'), 201);
  let tools = (await app.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools ?? [];
  assert.equal(tools.some(tool => tool.name === 'miniclaw_delegate'), false);
  await app.close();

  const second = await launchApp();
  t.after(() => second.close());
  await json(await post(second, '/api/agents', { name: 'Blender', instructions: 'Model in Blender.', active: true }), 201);
  await json(await post(second, '/api/agents', { name: 'Asleep', instructions: 'Rest.', active: false }), 201);
  await json(await post(second, '/api/threads'), 201);
  tools = (await second.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools ?? [];
  const tool = tools.find(tool => tool.name === 'miniclaw_delegate');
  assert.ok(tool, 'offered to a plain conversation');
  assert.match(tool.description, /Blender/);
  assert.doesNotMatch(tool.description, /Asleep/);
  await json(await post(second, `/api/threads/${THREAD}/messages`, { text: 'Make tiles', clientMessageId: CLIENT }), 202);
  const note = (await second.inspect()).requests.find(r => r.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(note, /You may delegate to: Blender/);
  await second.set('thread/start', { result: started(second.peer.cwd, { id: OTHER, sessionId: OTHER }) });
  await delegate(second, 'p-1', { agent: 'Blender', task: 'Build a tile set.' });
  const asked = await eventually(() => snapshot(second), value => value.delegationPrompt, 'delegation prompt');
  assert.equal(asked.delegationPrompt.agent, 'Blender');
  await json(await post(second, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => second.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  const child = (await second.inspect()).requests.filter(r => r.method === 'thread/start')[1].params;
  assert.match(child.developerInstructions, /Model in Blender/);
  await childAnswers(second, 'Tiles exported.');
  const result = await reply(second, 'p-1');
  assert.equal(result.success, true);
  assert.match(text(result), /Answer from the agent Blender:[\s\S]*Tiles exported\./);
  // An inactive agent cannot be named.
  await delegate(second, 'p-2', { agent: 'Asleep', task: 'Wake up.' });
  assert.equal((await reply(second, 'p-2')).success, false);
});

// 2026-09-22: an agent hands pictures and files along with the task: the conversation's uploads and the
// pictures an MCP tool returned, named by the id MiniClaw gives the model. Each is checked before the user
// is asked, listed in the prompt, and copied into the recipient's own conversation with its first message.
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');
const upload = async (app, thread, name, bytes) => (await json(await app.request(`/api/threads/${thread}/attachments?name=${encodeURIComponent(name)}`,
  { method: 'POST', body: bytes, headers: { 'Content-Type': 'application/octet-stream', 'X-MiniClaw-Request': '1' } }), 201)).attachment;

test('a picture the user attached goes with the task to the recipient, which sees it as a picture', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/agents', { name: 'Blender', instructions: 'Model in Blender.', active: true }), 201);
  await json(await post(app, '/api/threads'), 201);
  const picture = await upload(app, THREAD, 'layout.png', PNG);
  const draft = await upload(app, THREAD, 'draft.png', PNG);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Use this layout', clientMessageId: CLIENT, attachments: [picture.id] }), 202);
  // The model is told each attachment's id, which is how it names one to hand over.
  const sent = (await app.inspect()).requests.find(r => r.method === 'turn/start').params;
  assert.match(sent.additionalContext.miniclaw_attachments.value, new RegExp(`layout\\.png[^\\n]*attachment id is ${picture.id}`));
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
  // An id the conversation does not have, or an upload no message carries, is refused before anyone is asked.
  await delegate(app, 'a-0', { agent: 'Blender', task: 'Build it.', attachments: ['00000000-0000-4000-8000-000000000009'] });
  assert.match(text(await reply(app, 'a-0')), /not a picture or file of this conversation/);
  await delegate(app, 'a-1', { agent: 'Blender', task: 'Build it.', attachments: [draft.id] });
  assert.equal((await reply(app, 'a-1')).success, false);
  assert.equal((await snapshot(app)).delegationPrompt, null);

  await delegate(app, 'a-2', { agent: 'Blender', task: 'Build tiles from this layout.', attachments: [picture.id] });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'delegation prompt');
  assert.deepEqual(asked.delegationPrompt.attachments.map(item => [item.id, item.name, item.kind]), [[picture.id, 'layout.png', 'image']]);
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  const state = await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  const input = state.requests.find(r => r.method === 'turn/start' && r.params.threadId === OTHER).params.input;
  assert.deepEqual(input.map(part => part.type), ['localImage', 'text']);
  assert.match(input[1].text, /Build tiles from this layout\./);
  await childAnswers(app, 'Tiles built.');
  assert.equal((await reply(app, 'a-2')).success, true);
  // The run records what went with it, for the conversation's card.
  const [run] = (await snapshot(app)).delegations.filter(run => run.status === 'done');
  assert.deepEqual(run.attachments.map(item => item.name), ['layout.png']);
  // The recipient's copy is its own: the parent's upload is untouched and still served.
  assert.equal((await app.request(`/api/threads/${THREAD}/attachments/${picture.id}`)).status, 200);
});

// 2026-09-23: a delegated Blender agent failed because its run started with the Blender MCP server off.
// An agent's "Enabled by default" capabilities seed every conversation of that agent, delegated runs included.
test("a delegated run starts with the recipient's on-by-default capabilities, such as its MCP server", async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'blender', kind: 'stdio', command: process.execPath }), 201);
  const blender = await agent(app, { name: 'Blender', instructions: 'Model in Blender.' });
  await json(await post(app, `/api/permissions/agent/${blender.id}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, `/api/agents/${blender.id}/capability-defaults`, { key: `mcp:${server.id}`, on: true }));
  await json(await post(app, '/api/threads'), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Make tiles', clientMessageId: CLIENT }), 202);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
  await delegate(app, 'b-1', { agent: 'Blender', task: 'Build a tile set.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'delegation prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  const child = (await json(await app.request(`/api/capabilities?thread=${OTHER}`))).capabilities.find(item => item.key === `mcp:${server.id}`);
  assert.deepEqual([child.allowed, child.on], [true, true], 'the Blender server is on in the run');
  const note = (await app.inspect()).requests.find(r => r.method === 'turn/start' && r.params.threadId === OTHER)
    .params.additionalContext.miniclaw_environment.value;
  assert.match(note, /MCP servers: blender/);
  // The parent, a main-agent conversation, did not get the Blender agent's default.
  const parent = (await json(await app.request(`/api/capabilities?thread=${THREAD}`))).capabilities.find(item => item.key === `mcp:${server.id}`);
  assert.equal(parent.on, false);
  await childAnswers(app, 'Tiles built.');
  assert.equal((await reply(app, 'b-1')).success, true);
});

test('a delegation waits for the user; "wait" runs the agent and hands its labelled answer back', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Review the release notes.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'delegation prompt');
  assert.deepEqual([asked.delegationPrompt.agent, asked.delegationPrompt.task], ['Reviewer', 'Review the release notes.']);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/start').length, 1,
    'nothing starts before the user answers');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));

  const child = await eventually(() => app.inspect(),
    value => value.requests.filter(r => r.method === 'thread/start').length === 2
      && value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'delegated run start');
  const start = child.requests.filter(r => r.method === 'thread/start')[1].params;
  assert.match(start.developerInstructions, /Review carefully/);
  assert.equal((start.dynamicTools ?? []).some(tool => tool.name === 'miniclaw_delegate'), false, 'no nesting');
  assert.equal((start.dynamicTools ?? []).some(tool => tool.name === 'miniclaw_ask'), false, 'a delegated run decides for itself and asks the user nothing');
  const sent = child.requests.find(r => r.method === 'turn/start' && r.params.threadId === OTHER);
  assert.match(JSON.stringify(sent.params.input), /Review the release notes\./);
  assert.equal((await snapshot(app)).delegations[0].status, 'running');

  await childAnswers(app, 'Two typos on line 4.');
  const result = await reply(app, 'd-1');
  assert.equal(result.success, true);
  assert.match(text(result), /Reviewer/);
  assert.match(text(result), /Two typos on line 4\./);
  const done = (await snapshot(app)).delegations;
  assert.deepEqual(done.map(run => [run.agent, run.mode, run.status, run.result, run.thread]),
    [['Reviewer', 'wait', 'done', 'Two typos on line 4.', OTHER]]);
  assert.equal(done[0].turnId, TURN);
  // The run is not a conversation of its own unless the user makes it one.
  const { threads } = await json(await app.request('/api/threads'));
  assert.equal(threads.some(task => task.id === OTHER), false);
});

test('a declined, unknown or unpermitted delegation starts nothing, and the limit holds', async t => {
  const { app } = await leadChat(t, { maxPerTurn: 1, minutes: 5 });
  for (const [id, args] of [['d-x', { agent: 'Outsider', task: 'x' }], ['d-y', { agent: 'Nobody', task: 'x' }],
    ['d-z', { agent: 'Reviewer', task: '' }]]) {
    await delegate(app, id, args);
    const refused = await reply(app, id);
    assert.equal(refused.success, false, id);
    assert.equal((await snapshot(app)).delegationPrompt ?? null, null, 'no prompt for a refused request');
  }
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'First' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'reject' }));
  const declined = await reply(app, 'd-1');
  assert.equal(declined.success, false);
  assert.match(text(declined), /declined/i);
  // A declined request still used the turn's single allowance.
  await delegate(app, 'd-2', { agent: 'Reviewer', task: 'Second' });
  assert.match(text(await reply(app, 'd-2')), /limit/i);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/start').length, 1);
});

// Requirement 2026-10-11 (user decision): the main agent's delegation limits are editable like any agent's,
// and a time limit may be up to a day. A test minute lasts 25 ms here.
const FAST = { env: { MINICLAW_TEST_MINUTE_MS: '25' } };
// Answers the open prompt with `mode` and waits until the run's own turn has started.
async function answer(app, mode) {
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'delegation prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  return asked.delegationPrompt;
}

test("a plain conversation delegates within the main agent's own limits, and a run stops at its time limit", async t => {
  const app = await launchApp({}, FAST);
  t.after(() => app.close());
  await agent(app, { name: 'Blender', instructions: 'Model in Blender.' });
  const saved = await json(await post(app, '/api/main-agent', { model: null, effort: null, delegation: { maxPerTurn: 1, minutes: 2 } }));
  assert.deepEqual(saved.delegation, { maxPerTurn: 1, minutes: 2 });
  await json(await post(app, '/api/threads'), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Make tiles', clientMessageId: CLIENT }), 202);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
  const started_at = Date.now();
  await delegate(app, 'm-1', { agent: 'Blender', task: 'Build a tile set.' });
  const prompt = await answer(app, 'background');
  // The prompt says how long the run may take, and how long the conversation would wait for it.
  assert.deepEqual([prompt.minutes, prompt.waitMinutes], [2, 2]);
  assert.equal((await reply(app, 'm-1')).success, true);
  // The main agent's one delegation for this message is used.
  await delegate(app, 'm-2', { agent: 'Blender', task: 'Another.' });
  assert.match(text(await reply(app, 'm-2')), /limit/i);
  // The run never answers, so it stops at two (test) minutes, not at the default ten.
  const ended = await eventually(() => snapshot(app), value => value.delegations[0].status === 'failed', 'run stopped', 5000);
  assert.ok(Date.now() - started_at >= 2 * 25, 'not before its limit');
  assert.match(ended.delegations[0].result, /stopped at its 2 minute limit/);
  assert.ok((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), 'the run was interrupted');
});

// Requirement change 2026-10-11 (lead decision): a waited run whose limit is an hour or less behaves as before:
// it stops at its limit and the parent's call returns that it did not finish. A background run gets its full limit.
test('a waited run with a limit of an hour or less stops at its limit, while a background run gets its full limit', async t => {
  const { app } = await leadChat(t, { maxPerTurn: 2, minutes: 60 }, FAST);
  await delegate(app, 'w-1', { agent: 'Reviewer', task: 'Review everything.' });
  const prompt = await answer(app, 'wait');
  assert.deepEqual([prompt.minutes, prompt.waitMinutes], [60, 60]);
  const waited = await reply(app, 'w-1');
  assert.equal(waited.success, false);
  assert.match(text(waited), /stopped at its 60 minute limit/);
  assert.doesNotMatch(text(waited), /background/, 'nothing is handed over');
  const stopped = (await snapshot(app)).delegations[0];
  assert.deepEqual([stopped.status, stopped.mode, stopped.handedOver], ['failed', 'wait', false]);
  assert.ok((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), 'the run was interrupted');

  await app.set('thread/start', { result: started(app.peer.cwd, { id: '019a7777-7777-7777-8777-777777777777', sessionId: '019a7777-7777-7777-8777-777777777777' }) });
  const begun = Date.now();
  await delegate(app, 'w-2', { agent: 'Reviewer', task: 'Review in the background.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'second prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await reply(app, 'w-2');
  const ended = await eventually(() => snapshot(app), value => value.delegations[1]?.status === 'failed', 'background run stopped', 10_000);
  assert.ok(Date.now() - begun >= 60 * 25, 'it ran its whole limit');
  assert.match(ended.delegations[1].result, /stopped at its 60 minute limit/);
});

// Requirement change 2026-10-11 (lead decision): at the hour, a waited run whose own limit is longer is handed to the
// background instead of being stopped. The parent's call returns then, as a background hand-off does, with the run id
// and how to check on it; the run keeps going to its full limit and its result comes back as a background result.
test('a waited run longer than an hour continues in the background at the hour, and the parent carries on', async t => {
  const { app } = await leadChat(t, { maxPerTurn: 2, minutes: 240 }, FAST);
  const begun = Date.now();
  await delegate(app, 'h-1', { agent: 'Reviewer', task: 'Review everything.' });
  const prompt = await answer(app, 'wait');
  assert.deepEqual([prompt.minutes, prompt.waitMinutes], [240, 60]);
  const running = await eventually(() => snapshot(app), value => value.delegations[0]?.status === 'running', 'running');
  assert.deepEqual([running.delegations[0].mode, running.delegations[0].handedOver], ['wait', false]);
  const handed = await reply(app, 'h-1');
  assert.ok(Date.now() - begun >= 60 * 25, 'not before the hour');
  assert.equal(handed.success, true);
  const run = (await snapshot(app)).delegations[0];
  assert.match(text(handed), /Reviewer has not finished after 60 minutes, so it continues in the background/);
  assert.match(text(handed), new RegExp(`runId ${run.id}`), 'the run id, to check on it');
  assert.match(text(handed), /action=status/);
  assert.match(text(handed), /Do not wait for it and do not repeat the request/);
  assert.deepEqual([run.status, run.mode, run.handedOver], ['running', 'background', true]);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), false, 'the run was not stopped');
  // The parent's turn carries on with the answer and ends; the run does not stop with it.
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await eventually(() => snapshot(app), value => !value.activeTurn, 'parent turn end');
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal((await snapshot(app)).delegations[0].status, 'running');
  await childAnswers(app, 'Everything reviewed.');
  const done = await eventually(() => snapshot(app), value => value.delegations[0].status === 'done', 'run done');
  assert.deepEqual([done.delegations[0].result, done.delegations[0].mode, done.delegations[0].handedOver],
    ['Everything reviewed.', 'background', true]);
  // As after a background hand-off, the idle parent carries on with the result by itself, once.
  const turns = await eventually(async () => (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD),
    value => value.length === 2, 'the parent carries on by itself');
  assert.match(turns[1].params.input[0].text, /^\[MiniClaw notice\] Reviewer finished "Do the task"/);
  assert.match(JSON.stringify(turns[1].params.additionalContext), /Everything reviewed\./);
});

// Stopping the parent while it still waits stops the run, as before. Once handed over, the run is a background run:
// stopping the parent leaves it going to its full limit, and its result waits for the user's next message.
test('after the hand-over, stopping the parent leaves the run going to its full limit, and its result waits for the user', async t => {
  const { app } = await leadChat(t, { maxPerTurn: 2, minutes: 120 }, FAST);
  await delegate(app, 'h-2', { agent: 'Reviewer', task: 'Review slowly.' });
  await answer(app, 'wait');
  assert.match(text(await reply(app, 'h-2')), /continues in the background/);
  await json(await post(app, `/api/threads/${THREAD}/interrupt`, { turnId: TURN }));
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  await eventually(() => snapshot(app), value => !value.activeTurn, 'parent stopped');
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal((await snapshot(app)).delegations[0].status, 'running', 'the run did not stop with the parent');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), false);
  const ended = await eventually(() => snapshot(app), value => value.delegations[0].status === 'failed', 'stopped at its full limit', 10_000);
  assert.match(ended.delegations[0].result, /stopped at its 120 minute limit/);
  assert.ok((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), 'interrupted at the limit');
  await new Promise(resolve => setTimeout(resolve, 400));
  const parentTurns = async () => (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD);
  assert.equal((await parentTurns()).length, 1, 'a stopped parent does not carry on by itself');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'And now?', clientMessageId: '019a6666-6666-7666-8666-666666666666' }), 202);
  const next = await eventually(parentTurns, value => value.length === 2, 'next message');
  assert.match(JSON.stringify(next[1].params.additionalContext), /stopped at its 120 minute limit/);
});

test('a restart during a handed-over run closes it as it closes any background run, and the parent hears of it', async t => {
  const { app } = await leadChat(t, { maxPerTurn: 2, minutes: 600 }, FAST);
  await delegate(app, 'h-3', { agent: 'Reviewer', task: 'Review for hours.' });
  await answer(app, 'wait');
  assert.match(text(await reply(app, 'h-3')), /continues in the background/);
  await app.restart();
  const closed = (await snapshot(app)).delegations[0];
  assert.deepEqual([closed.status, closed.mode, closed.handedOver], ['failed', 'background', true]);
  assert.match(closed.result, /not run again/);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === OTHER).length, 0,
    'the run is not repeated after the restart');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Where is the review?', clientMessageId: '019a6666-6666-7666-8666-666666666666' }), 202);
  const next = await eventually(async () => (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD),
    value => value.length === 1, 'next message');
  assert.match(JSON.stringify(next[0].params.additionalContext), /not run again/);
});

// Lead decision 2026-10-11: the model is told its limits (delegations per message, a run's time limit, and the
// hour a waited run gets), so it can plan for them. The main agent's limits are read again, as at a hand-off.
test("the delegation tool and each message's note state the limits, and follow a change to the main agent's", async t => {
  const { app: lead } = await leadChat(t, { maxPerTurn: 2, minutes: 5 });
  const offered = (await lead.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools.find(tool => tool.name === 'miniclaw_delegate');
  assert.match(offered.description, /Limits: at most 2 delegations per user message; each run may take up to 5 minutes\./);
  assert.doesNotMatch(offered.description, /60 minutes/, 'a run of an hour or less is never handed over');
  const leadNote = (await lead.inspect()).requests.find(r => r.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(leadNote, /You may delegate to: Reviewer \(at most 2 delegations per user message; each run may take up to 5 minutes\)/);

  const app = await launchApp();
  t.after(() => app.close());
  await agent(app, { name: 'Blender', instructions: 'Model in Blender.' });
  await json(await post(app, '/api/main-agent', { model: null, effort: null, delegation: { maxPerTurn: 4, minutes: 150 } }));
  await json(await post(app, '/api/threads'), 201);
  const tool = (await app.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools.find(tool => tool.name === 'miniclaw_delegate');
  assert.match(tool.description, /Limits: at most 4 delegations per user message; each run may take up to 150 minutes; a run the user chose to wait for that is still going after 60 minutes continues in the background\./);
  // A change reaches the next message's note at once...
  await json(await post(app, '/api/main-agent', { model: null, effort: null, delegation: { maxPerTurn: 1, minutes: 30 } }));
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Make tiles', clientMessageId: CLIENT }), 202);
  const note = (await app.inspect()).requests.find(r => r.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(note, /You may delegate to: Blender \(at most 1 delegation per user message; each run may take up to 30 minutes\)/);
  // ...and the tool's description when the conversation is loaded again.
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  const resumed = (await app.inspect()).requests.find(r => r.method === 'thread/resume').params.dynamicTools.find(tool => tool.name === 'miniclaw_delegate');
  assert.match(resumed.description, /Limits: at most 1 delegation per user message; each run may take up to 30 minutes\./);
});

// Requirement change 2026-09-23: a background result no longer waits for the user's next message. When the
// parent is idle MiniClaw starts a turn for it with the result, marked as MiniClaw's notice, once.
test('"background" answers the agent at once; when the run ends the idle parent carries on with its result by itself', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Check the licence headers.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  const early = await reply(app, 'd-1');
  assert.equal(early.success, true);
  assert.match(text(early), /background/);
  assert.match(text(early), /Do not wait/);
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'
    && r.params.threadId === OTHER), 'background run start');
  // The parent's turn ends while the run is still going.
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  assert.equal((await snapshot(app)).delegations[0].status, 'running');
  await childAnswers(app, 'All 14 files have headers.');
  const done = await eventually(() => snapshot(app), value => value.delegations[0].status === 'done', 'run done');
  assert.deepEqual([done.delegations[0].mode, done.delegations[0].result], ['background', 'All 14 files have headers.']);

  // The idle lead agent is given a turn of its own with the result, which MiniClaw marks as its notice.
  const turns = await eventually(async () => (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD),
    value => value.length === 2, 'the parent carries on by itself');
  const auto = turns[1].params;
  assert.match(auto.input[0].text, /^\[MiniClaw notice\] Reviewer finished "Do the task"/);
  assert.match(JSON.stringify(auto.additionalContext), /All 14 files have headers\./);
  const shown = await eventually(() => snapshot(app), value => value.messages.some(m => m.text?.startsWith('[MiniClaw notice]')), 'notice kept');
  assert.equal(shown.messages.filter(m => m.role === 'user').length, 2);
  // Delivered once: the user's next message does not carry it again, and nothing else starts.
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await eventually(() => snapshot(app), value => !value.activeTurn, 'turn end');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'And now?', clientMessageId: '019a6666-6666-7666-8666-666666666666' }), 202);
  const last = (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD);
  assert.equal(last.length, 3);
  assert.equal(JSON.stringify(last.at(-1).params.additionalContext ?? {}).includes('All 14 files have headers.'), false);
});

test('a result that ends while the parent is still responding waits for that response, then the parent carries on', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Check.', summary: 'Check the headers' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await reply(app, 'd-1');
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  await childAnswers(app, 'Done checking.');
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'done', 'run done');
  const parentTurns = async () => (await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD).length;
  await new Promise(resolve => setTimeout(resolve, 400));
  assert.equal(await parentTurns(), 1, 'the running response is not interrupted or doubled');
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await eventually(parentTurns, value => value === 2, 'the parent carries on after its response');
});

test('a stopped parent does not carry on by itself; the result waits for the user', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Check.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await reply(app, 'd-1');
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  // The user stops the parent's response.
  await json(await post(app, `/api/threads/${THREAD}/interrupt`, { turnId: TURN }));
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'interrupted', items: [] } } });
  await eventually(() => snapshot(app), value => !value.activeTurn, 'stopped');
  await childAnswers(app, 'Done checking.');
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'done', 'run done');
  await new Promise(resolve => setTimeout(resolve, 400));
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === THREAD).length, 1);
});

test('a hand-off needs a one-line summary of at most 80 characters, shown in the prompt and kept on the run', async t => {
  const { app } = await leadChat(t);
  for (const [id, summary] of [['s-0', undefined], ['s-1', 'x'.repeat(81)], ['s-2', 'two\nlines'], ['s-3', '  ']]) {
    await delegate(app, id, { agent: 'Reviewer', task: 'Review.', summary });
    assert.match(text(await reply(app, id)), /summary/);
  }
  assert.equal((await snapshot(app)).delegationPrompt, null, 'nobody was asked');
  await delegate(app, 's-4', { agent: 'Reviewer', task: 'Review every file under src for licence headers.', summary: 'Check the licence headers' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  assert.equal(asked.delegationPrompt.summary, 'Check the licence headers');
  assert.equal(asked.delegations.at(-1).summary, 'Check the licence headers');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'reject' }));
});

test("a running agent's live line is its newest step, or the last line of what it is writing", async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Read the notes.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
  await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } });
  await app.emit(delta('Looking at the notes.\nChecking the headers', 'child-words', OTHER));
  await eventually(() => snapshot(app), value => value.delegations[0].live === 'Checking the headers', 'words line');
  // A step replaces it.
  await app.emit({ id: 'child-read', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: OTHER, turnId: TURN, callId: 'child-read', arguments: { action: 'read_file', path: deniedHostFile } } });
  await eventually(() => snapshot(app), value => value.delegations[0].live === `Read ${deniedHostFile}`, 'step line');
  // With the Windows gate closed nobody is asked; the run still ends with its answer.
  if (!await childGateClosed(app, 'child-read')) {
    const waiting = await eventually(() => snapshot(app), value => value.delegatedRequests?.length, 'approval');
    await json(await post(app, `/api/threads/${OTHER}/scoped-approvals/${waiting.delegatedRequests[0].request.id}`, { action: 'reject' }));
    await reply(app, 'child-read');
  }
  await childAnswers(app, 'Could not read it.');
  await reply(app, 'd-1');
});

test('a finished run can become a conversation of its own, and a restart closes unfinished runs', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Review.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'running' && value.delegations[0].thread, 'running');
  const run = (await snapshot(app)).delegations[0];
  // Requirement change: a running run may be opened as its own conversation.
  assert.equal((await post(app, `/api/delegations/${run.id}/detach`)).status, 200);
  assert.equal((await snapshot(app)).delegations[0].detached, true);
  await app.restart();
  const closed = (await snapshot(app)).delegations[0];
  assert.equal(closed.status, 'failed');
  assert.match(closed.result, /not run again/);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start' && r.params.threadId === OTHER).length, 0,
    'the run is not repeated after the restart');
  await json(await post(app, `/api/delegations/${run.id}/detach`));
  assert.equal((await snapshot(app)).delegations[0].detached, true);
});

test("a delegated agent's approvals and activity appear in the parent, named, and its own rules decide", async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Read the notes.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'
    && r.params.threadId === OTHER), 'run start');
  await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } });
  // The delegated agent asks to read a file nobody granted it.
  await app.emit({ id: 'child-read', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: OTHER, turnId: TURN, callId: 'child-read', arguments: { action: 'read_file', path: deniedHostFile } } });
  // With the Windows gate closed the call is refused without asking, and shows in the parent all the same.
  if (!await childGateClosed(app, 'child-read')) {
    const waiting = await eventually(() => snapshot(app), value => value.delegatedRequests?.find(item => item.kind === 'scoped'), 'approval in the parent');
    const approval = waiting.delegatedRequests.find(item => item.kind === 'scoped');
    assert.equal(approval.agent, 'Reviewer');
    assert.equal(approval.thread, OTHER);
    await json(await post(app, `/api/threads/${OTHER}/scoped-approvals/${approval.request.id}`, { action: 'reject' }));
    const refused = await reply(app, 'child-read');
    assert.equal(refused.success, false, "the parent's permissions are not lent to the delegated agent");
    const seen = await eventually(() => snapshot(app), value => value.activities.some(a => a.id === 'child-read'
      && a.status !== 'running'), 'activity in the parent');
    const entry = seen.activities.find(a => a.id === 'child-read');
    assert.deepEqual([entry.agent, entry.status, entry.runId], ['Reviewer', 'refused', seen.delegations[0].id]);
    assert.equal((await snapshot(app)).scopedApproval ?? null, null);
  }
  await childAnswers(app, 'Could not read it.');
  await reply(app, 'd-1');
});

test('deleting a conversation deletes the runs that lived inside it', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Review.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'
    && r.params.threadId === OTHER), 'run start');
  await childAnswers(app, 'Fine.');
  await reply(app, 'd-1');
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await eventually(() => snapshot(app), value => !value.activeTurn, 'turn end');
  await app.set('thread/delete', { result: {} });
  await app.set('thread/read', { cases: [
    { params: { threadId: OTHER }, handler: { result: { thread: { ...started(app.peer.cwd, { id: OTHER, sessionId: OTHER }).thread } } } }],
    result: { thread: started(app.peer.cwd).thread } });
  // The fake Codex must also say that the run has no child tasks of its own.
  await app.set('thread/list', { result: { data: [], nextCursor: null } });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  const deleted = (await app.inspect()).requests.filter(r => r.method === 'thread/delete').map(r => r.params.threadId);
  assert.deepEqual(deleted.sort(), [THREAD, OTHER].sort());
});

test('an account change ends a running delegation with that reason, and nothing keeps running', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'd-1', { agent: 'Reviewer', task: 'Review.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'running', 'running');
  // Another person signs in to Codex.
  await app.set('account/read', { result: { account: { type: 'chatgpt', email: 'someone-else@example.test', planType: 'plus' }, requiresOpenaiAuth: true } });
  await app.emit({ method: 'account/updated', params: { authMode: 'chatgpt' } });
  const ended = await eventually(() => snapshot(app).catch(() => null), value => value && value.delegations[0].status === 'failed',
    'the run to end', 8000);
  assert.match(ended.delegations[0].result, /account changed/i);
});

test('the parent can check live delegation progress without starting more work', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'progress-start', { agent: 'Reviewer', task: 'Check release notes.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt);
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await reply(app, 'progress-start');
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER));
  await app.emit({ method: 'item/agentMessage/delta', params: { threadId: OTHER, turnId: TURN,
    itemId: 'progress-note', delta: 'Checked the headings; checking examples next.' } });
  await app.emit({ id: 'progress-read', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: OTHER, turnId: TURN, callId: 'progress-read', arguments: { action: 'read_file', path: deniedHostFile } } });
  // With the Windows gate closed the run waits for nothing, and its progress shows the refused step.
  if (await childGateClosed(app, 'progress-read')) {
    await delegate(app, 'progress-status', { action: 'status' });
    const closed = await reply(app, 'progress-status');
    assert.equal(closed.success, true);
    const [run] = JSON.parse(text(closed)).runs;
    assert.deepEqual([run.id, run.status, run.waitingFor], [asked.delegationPrompt.id, 'running', null]);
    assert.match(run.latestUpdate, /checking examples next/);
    assert.deepEqual([run.activity.at(-1).title, run.activity.at(-1).status], [deniedHostFile, 'refused']);
    await childAnswers(app, 'Review complete.');
    await eventually(() => snapshot(app), value => value.delegations[0].status === 'done');
    return;
  }
  await eventually(() => snapshot(app), value => value.delegatedRequests?.length);
  await delegate(app, 'progress-status', { action: 'status' });
  const answer = await reply(app, 'progress-status');
  assert.equal(answer.success, true);
  const state = JSON.parse(text(answer));
  assert.equal(state.runs.length, 1);
  assert.equal(state.runs[0].id, asked.delegationPrompt.id);
  assert.equal(state.runs[0].status, 'running');
  assert.equal(state.runs[0].waitingFor, 'approval');
  assert.match(state.runs[0].latestUpdate, /checking examples next/);
  assert.equal(state.runs[0].activity.at(-1).title, deniedHostFile);
  assert.equal((await snapshot(app)).delegationPrompt, null);
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'thread/start').length, 2);
  // A guessed run from another conversation cannot disclose its progress.
  await delegate(app, 'progress-foreign', { action: 'status', runId: '00000000-0000-4000-8000-000000000009' });
  assert.equal((await reply(app, 'progress-foreign')).success, false);
  const waiting = (await snapshot(app)).delegatedRequests[0].request;
  await json(await post(app, `/api/threads/${OTHER}/scoped-approvals/${waiting.id}`, { action: 'reject' }));
  await reply(app, 'progress-read');
  await childAnswers(app, 'Review complete.');
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'done');
  await delegate(app, 'progress-done', { action: 'status', runId: asked.delegationPrompt.id });
  const done = JSON.parse(text(await reply(app, 'progress-done'))).runs[0];
  assert.equal(done.status, 'done');
  assert.equal(done.result, 'Review complete.');
});

test('a stopped child with partial text is not reported as finished successfully', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'stop-child', { agent: 'Reviewer', task: 'Review.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt);
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await reply(app, 'stop-child');
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER));
  await app.emit({ method: 'item/agentMessage/delta', params: { threadId: OTHER, turnId: TURN, itemId: 'partial', delta: 'Still reviewing.' } },
    { method: 'turn/completed', params: { threadId: OTHER, turn: { id: TURN, status: 'interrupted', items: [] } } });
  const final = await eventually(() => snapshot(app), value => value.delegations[0].status !== 'running');
  assert.equal(final.delegations[0].status, 'failed');
  assert.match(final.delegations[0].result, /interrupted/);
});

test('opening a waiting run preserves its work when the parent ends', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'open-wait', { agent: 'Reviewer', task: 'Review independently.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt);
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'wait' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER));
  await json(await post(app, `/api/delegations/${asked.delegationPrompt.id}/detach`));
  await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal((await snapshot(app)).delegations[0].status, 'running');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/interrupt' && r.params.threadId === OTHER), false);
  await app.emit({ id: 'no-nesting', method: 'item/tool/call', params: { tool: 'miniclaw_delegate',
    threadId: OTHER, turnId: TURN, callId: 'no-nesting', arguments: { agent: 'Outsider', task: 'Nest.' } } });
  assert.match(text(await reply(app, 'no-nesting')), /cannot delegate further/);
  await childAnswers(app, 'Independent review finished.');
  await eventually(() => snapshot(app), value => value.delegations[0].status === 'done');
});

// Background approvals are discoverable without opening or pinning their transcripts.
test('delegation approvals appear across conversations', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'global-prompt', { agent: 'Reviewer', task: 'Review from another chat.' });
  const working = await eventually(async () => json(await app.request('/api/threads/working')),
    value => value.delegationPrompts?.length, 'global delegation prompt');
  const prompt = working.delegationPrompts[0];
  assert.equal(prompt.thread, THREAD);
  assert.equal(prompt.agent, 'Reviewer');
  assert.equal(prompt.task, 'Review from another chat.');
  assert.ok(prompt.expiresAt > Date.now());
  await json(await post(app, `/api/threads/${prompt.thread}/delegations/${prompt.id}`, { mode: 'reject' }));
  await reply(app, 'global-prompt');
  assert.deepEqual((await json(await app.request('/api/threads/working'))).delegationPrompts, []);
});

// Parent and child requests must coexist, survive navigation, and retain separate owners.
test('parent queues child approvals and questions without replacing its own', async t => {
  const { app } = await leadChat(t);
  await delegate(app, 'queue-child', { agent: 'Reviewer', task: 'Read notes and ask.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'
    && r.params.threadId === OTHER), 'run start');
  await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } });
  for (const [threadId, id] of [[THREAD, 'parent-read'], [OTHER, 'child-read']]) {
    await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId,
      turnId: TURN, callId: id, arguments: { action: 'read_file', path: deniedHostFile } } });
  }
  await app.emit({ id: 'child-question', method: 'item/tool/call', params: { tool: 'miniclaw_ask',
    threadId: OTHER, turnId: TURN, callId: 'child-question', arguments: { questions: [{
      question: 'Which format?', header: 'Format', multiSelect: false,
      options: [{ label: 'Text', description: 'Plain text' }, { label: 'JSON', description: 'Structured' }],
    }] } } });
  // With the Windows gate closed both reads are refused without asking, so only the child's question
  // waits in the parent, and answering it clears the queue.
  if (await childGateClosed(app, 'child-read')) {
    assert.equal(text(await reply(app, 'parent-read')), await closedRefusal(app, TOOLS_UNAVAILABLE));
    const asking = await eventually(() => snapshot(app), value => value.delegatedRequests?.length === 1, 'child question');
    const [question] = asking.delegatedRequests;
    assert.deepEqual([question.kind, question.parent, question.thread, question.agent], ['question', THREAD, OTHER, 'Reviewer']);
    assert.equal(asking.scopedApproval, null);
    await json(await post(app, `/api/threads/${OTHER}/questions/${question.request.id}`, {
      answers: [{ selected: ['Text'], other: '' }],
    }));
    assert.equal((await reply(app, 'child-question')).success, true);
    await eventually(() => snapshot(app), value => value.delegatedRequests?.length === 0, 'child queue cleared');
    return childAnswers(app, 'Finished.');
  }
  // The three tool calls are handled concurrently, so the parent's own request may register after
  // both child requests; wait for all three before asserting on any of them.
  const waiting = await eventually(() => snapshot(app),
    value => value.delegatedRequests?.length === 2 && value.scopedApproval, 'parent request and child request queue');
  assert.equal(waiting.scopedApproval.thread, undefined, 'parent keeps its own permission request');
  const requests = waiting.delegatedRequests;
  for (const request of requests) {
    assert.equal(request.parent, THREAD);
    assert.equal(request.thread, OTHER);
    assert.equal(request.agent, 'Reviewer');
    assert.ok(request.expiresAt > Date.now());
  }
  const global = await json(await app.request('/api/threads/working'));
  assert.equal(global.delegatedRequests.length, 2, 'discoverable without following child SSE');
  const scoped = requests.find(item => item.kind === 'scoped');
  await json(await post(app, `/api/threads/${OTHER}/scoped-approvals/${scoped.request.id}`, { action: 'reject' }));
  assert.equal((await reply(app, 'child-read')).success, false);
  const question = requests.find(item => item.kind === 'question');
  await json(await post(app, `/api/threads/${OTHER}/questions/${question.request.id}`, {
    answers: [{ selected: ['Text'], other: '' }],
  }));
  assert.equal((await reply(app, 'child-question')).success, true);
  const cleared = await eventually(() => snapshot(app), value => value.delegatedRequests?.length === 0, 'child queue cleared');
  assert.equal(cleared.scopedApproval.id, waiting.scopedApproval.id, 'parent request remains');
  await json(await post(app, `/api/threads/${THREAD}/scoped-approvals/${waiting.scopedApproval.id}`, { action: 'reject' }));
  await childAnswers(app, 'Finished.');
});

// A coding-session review must be visible and decidable without opening the child chat.
test('child file reviews stay in the parent until the user decides', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-child-review-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'note.txt'), 'Original note');
  const { app, reviewer } = await leadChat(t);
  for (const action of ['read', 'write']) await json(await post(app, `/api/permissions/agent/${reviewer.id}`, {
    target: { target_type: 'folder', path: root, action }, effect: 'allow',
  }));
  await delegate(app, 'review-run', { agent: 'Reviewer', task: 'Create a note.' });
  const asked = await eventually(() => snapshot(app), value => value.delegationPrompt, 'prompt');
  await json(await post(app, `/api/threads/${THREAD}/delegations/${asked.delegationPrompt.id}`, { mode: 'background' }));
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'
    && r.params.threadId === OTHER), 'run start');
  await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } });
  await json(await post(app, `/api/permissions/conversation/${OTHER}`, {
    target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow',
  }));
  await app.emit({ id: 'child-write', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native',
    threadId: OTHER, turnId: TURN, callId: 'child-write', arguments: {
      action: 'write_file', path: join(root, 'note.txt'), text: 'Proposed child note',
    } } });
  // With the Windows gate closed the write is refused without a review, and the file stays as it was.
  if (await childGateClosed(app, 'child-write')) {
    assert.equal(await readFile(join(root, 'note.txt'), 'utf8'), 'Original note');
    return childAnswers(app, 'The file was not created.');
  }
  const waiting = await eventually(async () => {
    const ended = (await app.inspect()).responses.find(item => item.id === 'child-write');
    assert.equal(ended, undefined, ended ? text(ended.result) : 'review waits for user');
    return snapshot(app);
  }, value => value.delegatedRequests?.some(item => item.kind === 'review'), 'child review');
  const review = waiting.delegatedRequests.find(item => item.kind === 'review');
  assert.equal(review.agent, 'Reviewer');
  assert.equal(review.request.proposed, 'Proposed child note');
  await json(await post(app, `/api/threads/${review.thread}/reviews/${review.request.id}`, { action: 'reject', note: 'Do not create it.' }));
  const result = await reply(app, 'child-write');
  assert.match(text(result), /Do not create it/);
  await eventually(() => snapshot(app), value => value.delegatedRequests.length === 0, 'review removed');
  await childAnswers(app, 'The file was not created.');
});
