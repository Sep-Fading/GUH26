// Checks a project's issues: agents file, claim, hand over and finish them through the project
// tool; the project's review level decides which finished issues close and which wait for the user.
import assert from 'node:assert/strict';
// Registers the issue tests.
import { test } from 'node:test';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually, events } from './support/harness.mjs';
// Names the fake conversations and builds their start answers.
import { THREAD, OTHER, TURN, started, turnDone } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const text = result => result.contentItems.map(item => item.text).join('\n');

/** Starts a project with two agents, Builder in THREAD and Reviewer in OTHER. */
async function fixture(t) {
  const app = await launchApp();
  t.after(() => app.close());
  const builder = (await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build.', active: true }), 201)).agent;
  const reviewer = (await json(await post(app, '/api/agents', { name: 'Reviewer', instructions: 'Review.', active: true }), 201)).agent;
  const { project } = await json(await post(app, '/api/projects',
    { name: 'Game', environment: { kind: 'host' }, agentIds: [builder.id, reviewer.id] }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: builder.id }), 201);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: reviewer.id }), 201);
  return Object.assign(app, { project });
}

/** Calls the project tool as the model in one conversation would, and returns its result. */
async function call(app, thread, args, id = crypto.randomUUID()) {
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_project', threadId: thread, turnId: TURN, callId: id, arguments: args } });
  return (await eventually(() => app.inspect(), value => value.responses.some(response => response.id === id), `tool reply ${id}`, 10000))
    .responses.find(response => response.id === id).result;
}

/** Files an issue from a conversation and returns its number. */
async function file(app, thread, severity, title = `A ${severity} bug`) {
  const result = await call(app, thread, { action: 'issue_create', kind: 'bug', title, text: 'Steps and what happens.', severity, tags: ['queue'] });
  assert.equal(result.success, true, text(result));
  return Number(text(result).match(/#(\d+)/)[1]);
}

const issue = async (app, number) => (await json(await app.request(`/api/projects/${app.project.id}/issues/${number}`))).issue;

test('agents are told about issues: the tool, the briefing and the review level', async t => {
  const app = await fixture(t);
  const start = (await app.inspect()).requests.filter(request => request.method === 'thread/start').at(-1).params;
  const tool = start.dynamicTools.find(tool => tool.name === 'miniclaw_project');
  for (const action of ['issue_list', 'issue_read', 'issue_create', 'issue_comment', 'issue_claim', 'issue_release', 'issue_done']) {
    assert.ok(tool.inputSchema.properties.action.enum.includes(action), action);
  }
  assert.match(start.baseInstructions, /issue_create/);
  await json(await post(app, `/api/threads/${OTHER}/messages`, { text: 'Hello', clientMessageId: crypto.randomUUID() }), 202);
  const state = await eventually(() => app.inspect(), value => value.requests.some(request => request.method === 'turn/start'), 'turn');
  const note = state.requests.find(request => request.method === 'turn/start').params.additionalContext.miniclaw_environment.value;
  assert.match(note, /Issues: you may close Low issues yourself; Medium and High wait for the user/);
  await app.emit(turnDone());
});

test('a finished Low issue closes; a finished Medium one waits for the user', async t => {
  const app = await fixture(t);
  const low = await file(app, THREAD, 'low');
  const medium = await file(app, THREAD, 'medium');
  for (const number of [low, medium]) assert.equal((await call(app, THREAD, { action: 'issue_claim', issue: number })).success, true);

  const closed = await call(app, THREAD, { action: 'issue_done', issue: low, text: 'Fixed the label.' });
  assert.equal(closed.success, true);
  assert.match(text(closed), /closed/i);
  assert.equal((await issue(app, low)).status, 'closed');

  const waiting = await call(app, THREAD, { action: 'issue_done', issue: medium, text: 'Fixed the queue.' });
  assert.equal(waiting.success, true);
  assert.match(text(waiting), /review/i);
  assert.equal((await issue(app, medium)).status, 'review');

  // Only the user closes what waits for review.
  await json(await post(app, `/api/projects/${app.project.id}/issues/${medium}/close`, {}));
  assert.equal((await issue(app, medium)).status, 'closed');

  // A project that reviews nothing makes even a Low issue wait; one that reviews nothing above High closes Medium.
  await json(await post(app, `/api/projects/${app.project.id}/issue-settings`, { closeWithoutReview: 'none' }));
  const next = await file(app, THREAD, 'low', 'Another low bug');
  await call(app, THREAD, { action: 'issue_claim', issue: next });
  await call(app, THREAD, { action: 'issue_done', issue: next, text: 'Done.' });
  assert.equal((await issue(app, next)).status, 'review');
  await json(await post(app, `/api/projects/${app.project.id}/issue-settings`, { closeWithoutReview: 'medium' }));
  const third = await file(app, THREAD, 'medium', 'Another medium bug');
  await call(app, THREAD, { action: 'issue_claim', issue: third });
  await call(app, THREAD, { action: 'issue_done', issue: third, text: 'Done.' });
  assert.equal((await issue(app, third)).status, 'closed');
});

test('one owner at a time: claims, handovers and who may finish', async t => {
  const app = await fixture(t);
  const number = await file(app, THREAD, 'low');
  assert.equal((await call(app, THREAD, { action: 'issue_claim', issue: number })).success, true);
  assert.equal((await issue(app, number)).owner, 'Builder');

  const taken = await call(app, OTHER, { action: 'issue_claim', issue: number });
  assert.equal(taken.success, false);
  assert.match(text(taken), /Builder/);
  assert.equal((await call(app, OTHER, { action: 'issue_done', issue: number, text: 'Done.' })).success, false);
  assert.equal((await call(app, OTHER, { action: 'issue_release', issue: number, text: 'Not mine.' })).success, false);

  assert.equal((await call(app, THREAD, { action: 'issue_release', issue: number })).success, false, 'a release needs a handover');
  const released = await call(app, THREAD, { action: 'issue_release', issue: number, text: 'Cause found in queue.rs; fix not started.' });
  assert.equal(released.success, true);
  assert.equal((await issue(app, number)).owner, null);

  assert.equal((await call(app, OTHER, { action: 'issue_claim', issue: number })).success, true);
  const read = text(await call(app, OTHER, { action: 'issue_read', issue: number }));
  assert.match(read, /Cause found in queue\.rs/);
  assert.match(read, /Owner: Reviewer/);
  assert.equal((await call(app, OTHER, { action: 'issue_done', issue: number, text: 'Fixed.' })).success, true);
});

test('agents may raise severity but not lower it; the user may do either', async t => {
  const app = await fixture(t);
  const number = await file(app, THREAD, 'medium');
  assert.equal((await call(app, OTHER, { action: 'issue_severity', issue: number, severity: 'low' })).success, false);
  assert.equal((await call(app, OTHER, { action: 'issue_severity', issue: number, severity: 'high' })).success, true);
  assert.equal((await issue(app, number)).severity, 'high');
  await json(await post(app, `/api/projects/${app.project.id}/issues/${number}/severity`, { severity: 'low' }));
  assert.equal((await issue(app, number)).severity, 'low');
});

test('the list is searchable, the user can comment and take ownership away, and refusals say why', async t => {
  const app = await fixture(t);
  await file(app, THREAD, 'low', 'Sidebar dot stays after reopening');
  const queue = await file(app, THREAD, 'high', 'Queue paused after restart');
  const found = (await json(await app.request(`/api/projects/${app.project.id}/issues?q=restart`))).issues;
  assert.deepEqual(found.map(entry => entry.number), [queue]);
  assert.match(text(await call(app, OTHER, { action: 'issue_list', query: 'sidebar' })), /Sidebar dot/);

  await call(app, THREAD, { action: 'issue_claim', issue: queue });
  await json(await post(app, `/api/projects/${app.project.id}/issues/${queue}/comment`, { text: 'Seen again today.' }));
  await json(await post(app, `/api/projects/${app.project.id}/issues/${queue}/owner`, { owner: null }));
  const detail = await issue(app, queue);
  assert.equal(detail.owner, null);
  assert.ok(detail.events.some(event => event.kind === 'comment' && event.author === 'You' && event.text === 'Seen again today.'));

  assert.equal((await call(app, THREAD, { action: 'issue_create', kind: 'bug', title: '', text: 'x', severity: 'low' })).success, false);
  assert.equal((await call(app, THREAD, { action: 'issue_create', kind: 'bug', title: 'x', text: 'x', severity: 'urgent' })).success, false);
  assert.match(text(await call(app, THREAD, { action: 'issue_read', issue: 999 })), /No issue/);
  assert.equal((await app.request(`/api/projects/${app.project.id}/issues/999`)).status, 404);
});

test('issue calls show in Activity as issues, not memory', async t => {
  const app = await fixture(t);
  const number = await file(app, THREAD, 'low', 'Queue paused');
  await call(app, THREAD, { action: 'issue_claim', issue: number });
  const activities = (await json(await app.request(`/api/threads/${THREAD}`))).activities;
  const titles = activities.filter(entry => entry.kind === 'issue').map(entry => entry.title);
  assert.deepEqual(titles, ['Filed #1: Queue paused', 'Claimed #1']);
  assert.equal(activities.filter(entry => entry.kind === 'memory').length, 0);
});

test('an issue moves Backlog, In Dev, QA, Review, Human Review, and open pages hear each change live', async t => {
  const app = await fixture(t);
  const live = await events(app, `/api/projects/${app.project.id}/issues/live`);
  t.after(() => live.close());
  await live.next('ready');
  const number = await file(app, THREAD, 'medium');
  assert.equal((await issue(app, number)).status, 'backlog');
  await eventually(() => live.seen.filter(event => event.event === 'issues').length >= 1, Boolean, 'a change announced');

  await call(app, THREAD, { action: 'issue_claim', issue: number });
  assert.equal((await issue(app, number)).status, 'dev');
  assert.equal((await call(app, OTHER, { action: 'issue_stage', issue: number, stage: 'qa' })).success, false, 'only the owner moves it');
  assert.equal((await call(app, THREAD, { action: 'issue_stage', issue: number, stage: 'review' })).success, false, 'the user\'s stage is not an agent\'s');
  for (const stage of ['qa', 'agent_review']) {
    assert.equal((await call(app, THREAD, { action: 'issue_stage', issue: number, stage })).success, true, stage);
    assert.equal((await issue(app, number)).status, stage);
  }
  const before = live.seen.filter(event => event.event === 'issues').length;
  await call(app, THREAD, { action: 'issue_done', issue: number, text: 'Fixed and reviewed.' });
  assert.equal((await issue(app, number)).status, 'review');
  await eventually(() => live.seen.filter(event => event.event === 'issues').length > before, Boolean, 'done announced');
  assert.equal((await call(app, THREAD, { action: 'issue_stage', issue: number, stage: 'dev' })).success, false, 'waiting for the user');

  // Reopening an owned issue puts it back in development; releasing one in development returns it to the backlog.
  await json(await post(app, `/api/projects/${app.project.id}/issues/${number}/reopen`, {}));
  assert.equal((await issue(app, number)).status, 'dev');
  await call(app, THREAD, { action: 'issue_release', issue: number, text: 'Back to the queue.' });
  assert.equal((await issue(app, number)).status, 'backlog');
  const filtered = (await json(await app.request(`/api/projects/${app.project.id}/issues?status=backlog`))).issues;
  assert.deepEqual(filtered.map(entry => entry.number), [number]);
});
