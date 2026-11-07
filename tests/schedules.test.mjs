// Checks scheduled runs against the fake Codex: no paid inference, no real waiting.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { win32 } from 'node:path';
import { launchApp, json, eventually } from './support/harness.mjs';
import { TOOLS_UNAVAILABLE, closedRefusal } from './support/sandbox.mjs';
import { THREAD, TURN, message } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const list = async app => (await json(await app.request('/api/schedules'))).schedules;
// A file nobody granted, named as an absolute path of this platform.
const deniedHostFile = process.platform === 'win32'
  ? win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'drivers', 'etc', 'hosts') : '/etc/hosts';
// While Windows cannot enforce the sandbox, a scheduled agent run is offered no sandboxed tool: its
// dynamic tools lack miniclaw_scoped_native, and a call that names it anyway is refused at once with
// SANDBOX_UNAVAILABLE, so no approval exists to time out. Only those reports select the Windows
// expectations, so the timeout is checked once the gate opens.

// Saves one active agent and one schedule for it.
async function fixture(t, extra = {}, env = {}) {
  const app = await launchApp({}, { env });
  t.after(() => app.close());
  const { agent } = await json(await post(app, '/api/agents', { name: 'Reporter', instructions: 'Report.', active: true }), 201);
  const { schedule } = await json(await post(app, '/api/schedules',
    { name: 'Morning report', agentId: agent.id, prompt: 'Summarise the open issues.', every: 'daily', ...extra }), 201);
  return { app, agent, schedule };
}
const finish = (app, words) => app.emit(
  { method: 'turn/started', params: { threadId: THREAD, turn: { id: TURN, status: 'inProgress', items: [] } } },
  { method: 'item/completed', params: { threadId: THREAD, turnId: TURN, completedAtMs: 1900000000000, item: message(words) } },
  { method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });

test('a schedule is saved switched off, validates its fields and starts nothing by itself', async t => {
  const { app, agent, schedule } = await fixture(t);
  assert.deepEqual([schedule.enabled, schedule.every, schedule.approvalMinutes, schedule.lastStatus], [false, 'daily', 5, null]);
  for (const bad of [{ every: 'minutely' }, { prompt: '' }, { approvalMinutes: 0 }, { approvalMinutes: 61 },
    { agentId: '019a0000-0000-7000-8000-000000000000' }, { name: '' }]) {
    const status = (await post(app, '/api/schedules', { name: 'x', agentId: agent.id, prompt: 'p', every: 'daily', ...bad })).status;
    assert.ok([400, 404].includes(status), `${JSON.stringify(bad)} -> ${status}`);
  }
  await post(app, '/api/schedules/tick');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false, 'off means off');
  await app.restart();
  assert.equal((await list(app))[0].name, 'Morning report');
});

test('"Run now" runs the agent with the saved prompt and records a visible result', async t => {
  const { app, schedule } = await fixture(t);
  await json(await post(app, `/api/schedules/${schedule.id}/run`), 202);
  const started = await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'), 'run start');
  assert.match(started.requests.find(r => r.method === 'thread/start').params.developerInstructions, /Report\./);
  assert.match(JSON.stringify(started.requests.find(r => r.method === 'turn/start').params.input), /Summarise the open issues\./);
  assert.equal((await list(app))[0].lastStatus, 'running');
  assert.equal((await post(app, `/api/schedules/${schedule.id}/run`)).status, 409, 'one run of a schedule at a time');
  await finish(app, '3 open issues.');
  const done = await eventually(() => list(app), value => value[0].lastStatus === 'done', 'run done');
  assert.deepEqual([done[0].lastResult, done[0].lastThread], ['3 open issues.', THREAD]);
  // The run is an ordinary conversation, so its whole transcript can be read.
  const { threads } = await json(await app.request('/api/threads'));
  assert.ok(threads.some(task => task.id === THREAD));
});

test('an enabled schedule runs when due, not again until its next period', async t => {
  const { app, schedule } = await fixture(t, { every: 'hourly' });
  await json(await post(app, `/api/schedules/${schedule.id}`, { ...strip(schedule), enabled: true }));
  await post(app, '/api/schedules/tick');
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'), 'due run');
  await finish(app, 'Done.');
  await eventually(() => list(app), value => value[0].lastStatus === 'done', 'done');
  await post(app, '/api/schedules/tick');
  assert.equal((await app.inspect()).requests.filter(r => r.method === 'turn/start').length, 1, 'not due again yet');
});
const strip = ({ id, lastRun, lastStatus, lastResult, lastThread, ...rest }) => rest;

test('an approval nobody answers within the set time terminates the run', async t => {
  // The test shortens a "minute" so the configured one-minute wait passes in a moment.
  const { app, schedule } = await fixture(t, { approvalMinutes: 1 }, { MINICLAW_TEST_MINUTE_MS: '300' });
  await json(await post(app, `/api/schedules/${schedule.id}/run`), 202);
  await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start'), 'run start');
  await app.emit({ method: 'turn/started', params: { threadId: THREAD, turn: { id: TURN, status: 'inProgress', items: [] } } });
  await app.emit({ id: 'needs-ok', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: THREAD,
    turnId: TURN, callId: 'needs-ok', arguments: { action: 'read_file', path: deniedHostFile } } });
  const names = (await app.inspect()).requests.find(r => r.method === 'thread/start').params.dynamicTools.map(tool => tool.name);
  if (process.platform === 'win32' && !names.includes('miniclaw_scoped_native')) {
    assert.deepEqual(names, ['miniclaw_mcp', 'miniclaw_ask']);
    const refused = (await eventually(() => app.inspect(), value => value.responses.some(r => r.id === 'needs-ok'), 'refusal'))
      .responses.find(r => r.id === 'needs-ok').result;
    assert.deepEqual([refused.success, refused.contentItems.map(item => item.text).join('\n')], [false, await closedRefusal(app, TOOLS_UNAVAILABLE)]);
    assert.equal((await json(await app.request(`/api/threads/${THREAD}`))).scopedApproval, null, 'nobody is asked');
    assert.equal((await list(app))[0].lastStatus, 'running', 'nothing waits, so nothing is terminated');
    // The model carries on with the refusal and the run ends normally.
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN, completedAtMs: 1900000000000, item: message('Could not read it.') } },
      { method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
    const done = await eventually(() => list(app), value => value[0].lastStatus === 'done', 'run done');
    assert.equal(done[0].lastResult, 'Could not read it.');
    return;
  }
  const ended = await eventually(() => list(app), value => value[0].lastStatus === 'terminated', 'terminated', 8000);
  assert.match(ended[0].lastResult, /approval/i);
  // The request is quoted as JSON, so a Windows path appears with its backslashes escaped and in
  // the spelling Windows gives the folder.
  assert.ok(ended[0].lastResult.toLowerCase().includes(JSON.stringify(deniedHostFile).slice(1, -1).toLowerCase()), ended[0].lastResult);
  const log = await app.inspect();
  assert.ok(log.requests.some(r => r.method === 'turn/interrupt'), 'the turn is stopped, not left waiting');
  assert.equal(log.responses.find(r => r.id === 'needs-ok').result.success, false);
  const { notifications } = await json(await app.request('/api/notifications'));
  assert.ok(notifications.some(notice => notice.kind === 'scheduled_run_terminated'));
});
