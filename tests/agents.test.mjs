// Compares public responses and observed protocol requests.
import assert from 'node:assert/strict';
// Runs isolated API behavior regression cases.
import { test } from 'node:test';
// Checks permissions on persisted profile storage.
import { stat } from 'node:fs/promises';
// Locates metadata inside each disposable home.
import { join } from 'node:path';
// Names an absolute folder on every platform.
import { tmpdir } from 'node:os';
// Starts fixtures and inspects backend responses.
import { launchApp, json, eventually } from './support/harness.mjs';
import { assertGate } from './support/sandbox.mjs';
// Supplies synthetic account and conversation identifiers.
import { signedOut, THREAD, TURN, CLIENT } from './support/fixtures.mjs';
// Checks private Windows storage instead of mode bits.
import { assertWindowsPrivate } from './support/windows-acl.mjs';

// Sends one local JSON mutation through the test harness.
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
// Supplies a test profile with optional field overrides.
const definition = (extra = {}) => ({
  name: 'Editor', instructions: 'Check clarity. Keep technical identifiers unchanged.',
  model: 'fixture-model', effort: 'high', ...extra,
});
// Creates a profile and requires its successful acknowledgement.
const save = async (app, extra = {}) =>
  (await json(await post(app, '/api/agents', definition(extra)), 201)).agent;
// Submits a complete revision-based edit without its response-only identifier.
const update = (app, agent, extra) => {
  const { id, ...body } = agent;
  return post(app, `/api/agents/${id}`, { ...body, ...extra });
};
// While Windows cannot enforce the sandbox, a host agent chat is offered no sandboxed tool: its
// dynamic tools lack miniclaw_scoped_native and keep exactly the MCP gateway and the question tool.
// Only that report selects the Windows list, so the full list is checked once the gate opens.
const GATE_CLOSED_TOOLS = ['miniclaw_mcp', 'miniclaw_ask'];
// Tells whether the offered tool names show the closed Windows sandbox gate.
const sandboxGateClosed = names => process.platform === 'win32' && !names.includes('miniclaw_scoped_native');

test('agent defaults are inactive and private', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, []);
  const agent = await save(app);
  assert.match(agent.id, /^[0-9a-f-]{36}$/);
  assert.equal(agent.active, false);
  assert.equal(agent.revision, 1);
  assert.deepEqual(agent.capabilities, []);
  assert.deepEqual(agent.access, { applications: [], files: [], network: [] });
  assert.deepEqual(agent.delegatesTo, []);
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, [agent]);
  // POSIX mode bits mean nothing on Windows; there the file's access list must be private instead.
  if (process.platform === 'win32') return assertWindowsPrivate(join(app.peer.dir, 'agents.sqlite3'));
  const file = await stat(join(app.peer.dir, 'agents.sqlite3'));
  assert.equal(file.mode & 0o777, 0o600);
});

test('agent routes require authentication and local mutation headers', async t => {
  const app = await launchApp({ 'account/read': { result: signedOut } });
  t.after(() => app.close());
  assert.equal((await app.request('/api/agents')).status, 401);
  assert.equal((await post(app, '/api/agents', definition())).status, 401);
});

test('cross-site agent writes do not persist', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  for (const headers of [
    { Origin: 'https://unrelated.example' },
    { 'X-MiniClaw-Request': '' },
    { 'Sec-Fetch-Site': 'cross-site' },
  ]) {
    assert.equal((await app.request('/api/agents', {
      method: 'POST', body: definition(), headers,
    })).status, 403);
  }
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, []);
});

test('agent input rejects unknown authority and invalid values', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  for (const extra of [
    { name: ' ' }, { name: 'x'.repeat(81) }, { instructions: 'x'.repeat(16385) },
    { instructions: '' }, { command: '/bin/sh' }, { cwd: '/' },
    { access: { applications: [], files: [], network: [], unrestricted: true } },
    { model: 'not-a-model' }, { effort: 'ultra' },
    { capabilities: ['arbitrary_exec'] }, { delegatesTo: ['invented-agent'] },
    { revision: 900 },
  ]) assert.equal((await post(app, '/api/agents', definition(extra))).status, 400);
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, []);
});

test('unsupported capabilities and access cannot be activated', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  for (const extra of [
    { capabilities: ['computer_use'], access: {
      applications: [{ id: 'org.blenderfoundation.blender', actions: ['observe', 'control'] }],
      files: [], network: [],
    } },
    { capabilities: ['deep_research'] },
    // An absolute folder on every platform: '/tmp' is not absolute on Windows, so it was refused as invalid.
    { access: { applications: [], files: [{ path: join(tmpdir(), 'assets'), write: true }], network: [] } },
    { access: { applications: [], files: [], network: ['example.com'] } },
  ]) {
    const agent = await save(app, extra);
    const result = await json(await update(app, agent, { active: true }), 422);
    assert.equal(result.error.code, 'agent_policy_unsupported');
  }
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

test('only activated existing agents start direct chats', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app);
  assert.equal((await post(app, '/api/threads', { agentId: agent.id })).status, 409);
  assert.equal((await post(app, '/api/threads', { agentId: CLIENT })).status, 404);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
  const active = (await json(await update(app, agent, { active: true }))).agent;
  const { thread } = await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  assert.equal(thread.agentId, agent.id);
  assert.equal(thread.agentRevision, active.revision);
  assert.equal(thread.agentName, 'Editor');
  assert.equal(thread.computerUse, false);
  const request = (await app.inspect()).requests.find(r => r.method === 'thread/start');
  assert.equal(request.params.developerInstructions, agent.instructions);
  assert.equal(request.params.model, 'fixture-model');
  assert.equal(request.params.config.model_reasoning_effort, 'high');
  assert.deepEqual(request.params.environments, []);
  assert.deepEqual(request.params.selectedCapabilityRoots, []);
  assert.equal(request.params.approvalPolicy, 'never');
  assert.equal(request.params.sandbox, 'read-only');
  const names = (request.params.dynamicTools ?? []).map(tool => tool.name);
  if (await assertGate(app, sandboxGateClosed(names))) return assert.deepEqual(names, GATE_CLOSED_TOOLS);
  // Requirement change 2026-09-19: host agent chats get the one MiniClaw tool, enforced by Seatbelt.
  // Requirement change 2026-09-21: every conversation on this Mac also gets the MCP gateway, which reaches nothing until a server is switched on.
  assert.deepEqual((request.params.dynamicTools ?? []).map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask']);
});

test('agent chats cannot opt into broader computer access or delegation', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  assert.equal((await post(app, '/api/threads', {
    agentId: agent.id, computerUse: true,
  })).status, 403);
  assert.equal((await post(app, '/api/threads', {
    agentId: agent.id, delegation: true,
  })).status, 400);
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  assert.equal((await post(app, `/api/threads/${THREAD}/computer`, { enabled: true })).status, 403);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/archive'), false);
});

test('agent revision and chat binding survive restart', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  await json(await update(app, agent, { name: 'Changed', instructions: 'New instructions.' }));
  await app.restart();
  const listed = await json(await app.request('/api/threads'));
  assert.equal(listed.threads.find(x => x.id === THREAD).agentName, 'Editor');
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.thread.agentId, agent.id);
  assert.equal(snapshot.thread.agentRevision, 1);
  const request = (await app.inspect()).requests.find(r => r.method === 'thread/resume');
  assert.equal(request.params.developerInstructions, agent.instructions);
  const names = (request.params.dynamicTools ?? []).map(tool => tool.name);
  if (await assertGate(app, sandboxGateClosed(names))) return assert.deepEqual(names, GATE_CLOSED_TOOLS);
  // Requirement change 2026-09-19: host agent chats get the one MiniClaw tool, enforced by Seatbelt.
  // Requirement change 2026-09-21: every conversation on this Mac also gets the MCP gateway, which reaches nothing until a server is switched on.
  assert.deepEqual((request.params.dynamicTools ?? []).map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask']);
});

test('deactivation prevents later sends without hiding history', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  await json(await update(app, agent, { active: false }));
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  })).status, 409);
  assert.equal((await app.request(`/api/threads/${THREAD}`)).status, 200);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
});

test('deactivation still works when model discovery is unavailable', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await app.set('model/list', { error: { code: -32000, message: 'offline' } });
  const changed = (await json(await update(app, agent, { active: false }))).agent;
  assert.equal(changed.active, false);
});

test('deactivation interrupts an active response', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT,
  }), 202);
  await json(await update(app, agent, { active: false }));
  const state = await eventually(() => app.inspect(), s =>
    s.requests.some(r => r.method === 'turn/interrupt'));
  assert.deepEqual(state.requests.find(r => r.method === 'turn/interrupt').params,
    { threadId: THREAD, turnId: TURN });
});

test('malformed agent identifiers return a public JSON error', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const result = await json(await post(app, '/api/agents/not-a-uuid', definition()), 400);
  assert.equal(typeof result.error.code, 'string');
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, []);
});

test('agent sends cannot replace the binding or supply raw execution settings', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  for (const extra of [
    { agentId: CLIENT }, { computerUse: true }, { cwd: '/' },
    { config: { 'features.shell_tool': true } }, { developerInstructions: 'Override' },
  ]) assert.equal((await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Hello', clientMessageId: CLIENT, ...extra,
  })).status, 400);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
});

test('stale agent updates cannot overwrite newer settings', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app);
  const changed = (await json(await update(app, agent, { name: 'New name' }))).agent;
  assert.equal(changed.revision, 2);
  assert.equal((await update(app, agent, { active: true })).status, 409);
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, [changed]);
});

test('model tool requests cannot manage agents', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = await save(app, { active: true });
  await json(await post(app, '/api/threads', { agentId: agent.id }), 201);
  await app.emit({ id: 'forged-management', method: 'item/tool/call', params: {
    threadId: THREAD, tool: 'create_agent', arguments: definition({ active: true }),
  } });
  const state = await eventually(() => app.inspect(), value =>
    value.responses.some(r => r.id === 'forged-management'));
  assert.equal(state.responses.find(r => r.id === 'forged-management').result.success, false);
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, [agent]);
});

test('an agent may name existing agents to delegate to, with its own limits', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const reviewer = await save(app, { name: 'Reviewer', active: true });
  const plain = await save(app, { name: 'Lead' });
  assert.deepEqual(plain.delegation, { maxPerTurn: 3, minutes: 10 }, 'defaults');
  const lead = (await json(await update(app, plain, { active: true, delegatesTo: [reviewer.id],
    delegation: { maxPerTurn: 2, minutes: 5 } }))).agent;
  assert.deepEqual(lead.delegatesTo, [reviewer.id]);
  assert.deepEqual(lead.delegation, { maxPerTurn: 2, minutes: 5 });
  // A model cannot invent a recipient: only saved agents are accepted, never the agent itself.
  for (const extra of [{ delegatesTo: ['019a0000-0000-7000-8000-000000000000'] }, { delegatesTo: [lead.id] },
    { delegation: { maxPerTurn: 0, minutes: 5 } }, { delegation: { maxPerTurn: 11, minutes: 5 } },
    // Requirement change 2026-10-11 (user decision): up to 1440 minutes (a day), no longer 60.
    { delegation: { maxPerTurn: 3, minutes: 1441 } }, { delegation: { maxPerTurn: 3, minutes: 0 } }]) {
    const status = (await update(app, lead, extra)).status;
    assert.ok([400, 404, 422].includes(status), `${JSON.stringify(extra)} -> ${status}`);
  }
  const day = (await json(await update(app, lead, { delegation: { maxPerTurn: 10, minutes: 1440 } }))).agent;
  assert.deepEqual(day.delegation, { maxPerTurn: 10, minutes: 1440 });
  await app.restart();
  const { agents } = await json(await app.request('/api/agents'));
  assert.deepEqual(agents.find(a => a.id === lead.id).delegatesTo, [reviewer.id]);
  assert.deepEqual(agents.find(a => a.id === lead.id).delegation, { maxPerTurn: 10, minutes: 1440 });
});
