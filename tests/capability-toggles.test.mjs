// Checks the two levels of a capability: an agent's configuration says whether it is allowed at
// all, and the conversation switches what is allowed on or off.
import assert from 'node:assert/strict';
// Registers the capability tests.
import { test } from 'node:test';
// Seeds retired rules in disposable storage.
import { DatabaseSync } from 'node:sqlite';
// Locates the test application's permission database.
import { join } from 'node:path';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
// Names the fake conversation.
import { THREAD, CLIENT, started } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const MAIN = '00000000-0000-0000-0000-000000000000';
const allow = (app, agent, target, effect = 'allow') => post(app, `/api/permissions/agent/${agent}`, { target, effect });
const feature = name => ({ target_type: 'feature', name });
const list = async (app, query = `thread=${THREAD}`) => (await json(await app.request(`/api/capabilities?${query}`))).capabilities;
const byKey = (capabilities, key) => capabilities.find(capability => capability.key === key);
const toggle = (app, key, on) => post(app, `/api/threads/${THREAD}/capabilities`, { key, on });

test('retired bundle rules cannot break saved permissions', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await allow(app, MAIN, feature('network')));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const db = new DatabaseSync(join(app.peer.dir, 'agents.sqlite3'));
  try {
    const retired = JSON.stringify(feature('web_tools'));
    db.prepare('INSERT INTO permission_rules VALUES (?, ?, ?, ?)').run('agent', MAIN, retired, 'allow');
    db.prepare('INSERT INTO permission_rules VALUES (?, ?, ?, ?)').run('conversation', THREAD, retired, 'allow');
    db.prepare('INSERT INTO permission_sessions VALUES (?, ?, ?, ?, ?, ?)')
      .run(THREAD, '', '', 'host', retired, Math.floor(Date.now() / 1000) + 3600);
  } finally { db.close(); }
  await app.restart();
  const agent = await json(await app.request(`/api/permissions/agent/${MAIN}`));
  assert.deepEqual(agent.rules.map(rule => rule.target), [feature('network')]);
  await json(await app.request(`/api/permissions/conversation/${THREAD}`));
  const saved = await list(app);
  assert.equal(byKey(saved, 'network').allowed, true);
  for (const key of ['web_fetch', 'subagents', 'task_tracking']) {
    assert.equal(byKey(saved, key).allowed, false);
    assert.equal(byKey(saved, key).on, false);
  }
});

test('Claude tool allowances and saved choices are independent', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const keys = ['web_fetch', 'subagents', 'task_tracking'];
  for (const key of keys) {
    await json(await toggle(app, key, true), 403);
    await json(await allow(app, MAIN, feature(key)));
    assert.equal(byKey(await list(app), key).on, false);
    await json(await toggle(app, key, true));
  }
  await json(await toggle(app, 'subagents', false));
  await app.restart();
  const saved = await list(app);
  assert.deepEqual(keys.map(key => byKey(saved, key).on), [true, false, true]);
  await json(await allow(app, MAIN, feature('web_fetch'), 'remove'));
  const revoked = await list(app);
  assert.deepEqual(keys.map(key => [byKey(revoked, key).allowed, byKey(revoked, key).on]),
    [[false, false], [true, false], [true, true]]);
  assert.equal(byKey(revoked, 'web_search').on, true);
  assert.equal(byKey(revoked, 'plan_mode').on, false);
  assert.equal(byKey(revoked, 'web_tools'), undefined);
});

test('initial tools reach the first prompt and survive restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: process.execPath }), 201);
  await json(await allow(app, MAIN, feature('network')));
  await json(await allow(app, MAIN, { target_type: 'mcp_server', server: server.id }));
  const capabilities = { network: true, [`mcp:${server.id}`]: true, web_search: false,
    questions: false, coding_session: true, full_access: true, ask_commands: true };
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities }), 201);
  for (const [key, on] of Object.entries(capabilities)) assert.equal(byKey(await list(app), key).on, on, key);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start').params;
  assert.notEqual(start.config?.web_search, 'live', 'search must be off when Codex creates the thread');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Use my tools', clientMessageId: CLIENT }), 202);
  const first = (await app.inspect()).requests.find(r => r.method === 'turn/start').params;
  const note = first.additionalContext.miniclaw_environment.value;
  for (const expected of [/Network for commands: on/, /MCP servers: notes/, /Web search: off/,
    /Asking the user questions: off/, /Coding session.*: on/, /Full access.*: on/]) assert.match(note, expected);
  await app.restart();
  for (const [key, on] of Object.entries(capabilities)) assert.equal(byKey(await list(app), key).on, on, key);
});

test('initial tools cannot exceed the chosen agent permissions', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await allow(app, MAIN, feature('network')));
  const { agent } = await json(await post(app, '/api/agents', { name: 'Scout', instructions: 'Look.', active: true }), 201);
  const refused = await json(await post(app, '/api/threads', { environment: { kind: 'host' },
    agentId: agent.id, capabilities: { network: true } }), 403);
  assert.equal(refused.error.code, 'capability_not_allowed');
  await json(await allow(app, MAIN, feature('no_web_search')));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { web_search: true } }), 403);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, capabilities: { nonsense: true } }), 400);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

test('initial tools honor project allowances for a named agent', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build.', active: true }), 201);
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' }, agentIds: [agent.id] }), 201);
  await json(await post(app, `/api/permissions/project/${project.id}`, { target: feature('network'), effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id, projectId: project.id,
    capabilities: { network: true, project_memory: false } }), 201);
  assert.equal(byKey(await list(app), 'network').on, true);
  assert.equal(byKey(await list(app), 'project_memory').on, false);
});

test('a plain conversation lists its capabilities: search on, the rest not allowed until the main agent allows them', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: process.execPath }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const before = await list(app);
  assert.deepEqual(before.map(capability => [capability.key, capability.allowed, capability.on]), [
    ['web_search', true, true], ['image_generation', false, false], ['unsandboxed_shell', false, false], ['network', false, false], ['web_fetch', false, false],
    ['subagents', false, false], ['task_tracking', false, false], [`mcp:${server.id}`, false, false],
    ['ask_commands', true, false], ['full_access', true, false], ['questions', true, true], ['plan_mode', true, false], ['coding_session', true, false]]);
  assert.deepEqual([byKey(before, `mcp:${server.id}`).name, byKey(before, 'network').name], ['notes', 'Network for commands']);
  // What is not allowed cannot be switched on from the conversation, by the toggle or by a rule.
  const refused = await json(await toggle(app, 'network', true), 403);
  assert.equal(refused.error.code, 'capability_not_allowed');
  await json(await post(app, `/api/permissions/conversation/${THREAD}`, { target: feature('network'), effect: 'allow' }), 403);
  assert.equal(byKey(await list(app), 'network').on, false);

  // Requirement change 2026-09-22: allowed by the main agent, it is available, and off until the conversation
  // switches it on; nothing the agent does later moves the conversation's switch, except taking the capability away.
  await json(await allow(app, MAIN, feature('network')));
  await json(await allow(app, MAIN, { target_type: 'mcp_server', server: server.id }));
  assert.deepEqual([byKey(await list(app), 'network'), byKey(await list(app), `mcp:${server.id}`)].map(c => [c.allowed, c.on]), [[true, false], [true, false]]);
  await json(await toggle(app, 'network', true));
  assert.equal(byKey(await list(app), 'network').on, true);
  await json(await toggle(app, 'network', false));
  assert.equal(byKey(await list(app), 'network').on, false);
  await json(await toggle(app, 'network', true));
  await json(await toggle(app, `mcp:${server.id}`, true));
  // The agent taking a capability away takes it from the conversation that had it on.
  await json(await allow(app, MAIN, feature('network'), 'remove'));
  assert.deepEqual([byKey(await list(app), 'network').allowed, byKey(await list(app), 'network').on, byKey(await list(app), `mcp:${server.id}`).on], [false, false, true]);
  await json(await allow(app, MAIN, feature('network')));
  assert.equal(byKey(await list(app), 'network').on, true, 'its own switch was kept while the capability was away');
  // Restrictions need nobody's permission.
  await json(await toggle(app, 'coding_session', true));
  assert.equal(byKey(await list(app), 'coding_session').on, true);
  assert.equal((await toggle(app, 'nonsense', true)).status, 400);
});

test('web search is allowed unless the agent switches it off, and then the conversation cannot bring it back', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await toggle(app, 'web_search', false));
  assert.deepEqual([byKey(await list(app), 'web_search')].map(c => [c.allowed, c.on]), [[true, false]]);
  await json(await toggle(app, 'web_search', true));
  await json(await allow(app, MAIN, feature('no_web_search')));
  assert.deepEqual([byKey(await list(app), 'web_search')].map(c => [c.allowed, c.on]), [[false, false]]);
  assert.equal((await toggle(app, 'web_search', true)).status, 403);
});

test('before a conversation exists, the list follows the chosen agent; a named agent does not inherit the main agent', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await allow(app, MAIN, feature('network')));
  const { agent } = await json(await post(app, '/api/agents', { name: 'Scout', instructions: 'Look.', active: true }), 201);
  await json(await allow(app, agent.id, feature('web_fetch')));
  assert.deepEqual((await list(app, '')).filter(c => c.allowed).map(c => c.key), ['web_search', 'network', 'ask_commands', 'full_access', 'questions', 'plan_mode', 'coding_session']);
  assert.deepEqual((await list(app, `agentId=${agent.id}`)).filter(c => c.allowed).map(c => c.key), ['web_search', 'web_fetch', 'ask_commands', 'full_access', 'questions', 'plan_mode', 'coding_session']);
  // What is allowed will start off; search and questions start on.
  assert.deepEqual((await list(app, '')).filter(c => c.on).map(c => c.key), ['web_search', 'questions']);
  // A project can allow a capability for its conversations too.
  const { project } = await json(await post(app, '/api/projects', { name: 'Research', environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/permissions/project/${project.id}`, { target: feature('web_fetch'), effect: 'allow' }));
  assert.ok(byKey(await list(app, `projectId=${project.id}`), 'web_fetch').allowed);
});

// Requirement change 2026-09-22: both may be on; together they mean "ask me about file changes only".
test('full access and the coding session can both be on', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await toggle(app, 'coding_session', true));
  await json(await toggle(app, 'full_access', true));
  const both = await list(app);
  assert.deepEqual([byKey(both, 'full_access').on, byKey(both, 'coding_session').on], [true, true]);
});

test('a switch flipped during a turn does not end the turn; the next call sees the new state', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await allow(app, MAIN, feature('network')));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Go', clientMessageId: CLIENT }), 202);
  await json(await toggle(app, 'network', true));
  await json(await toggle(app, 'network', false));
  const during = await json(await app.request(`/api/threads/${THREAD}`));
  assert.ok(during.activeTurn, 'the turn is still running');
  assert.equal((await app.inspect()).requests.some(request => request.method === 'turn/interrupt'), false, 'nothing was interrupted');
});

test('agent full access defaults seed new conversations and allow an explicit off', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build.', active: true }), 201);
  await json(await allow(app, agent.id, feature('default_full_access')));
  await app.restart();
  assert.equal(byKey(await list(app, `agentId=${agent.id}`), 'full_access').on, true);
  assert.equal(byKey(await list(app, ''), 'full_access').on, false, 'named defaults stay with their agent');
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  assert.equal(byKey(await list(app), 'full_access').on, true);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Work', clientMessageId: CLIENT }), 202);
  const first = (await app.inspect()).requests.find(r => r.method === 'turn/start').params;
  assert.match(first.additionalContext.miniclaw_environment.value, /Full access.*: on/);
  await json(await allow(app, agent.id, feature('default_full_access'), 'remove'));
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/interrupt'), false);
  assert.equal(byKey(await list(app), 'full_access').on, true, 'changing defaults leaves existing chats alone');
  await json(await toggle(app, 'full_access', false));
  await app.restart();
  assert.equal(byKey(await list(app), 'full_access').on, false);
  await json(await allow(app, agent.id, feature('default_full_access')));
  await json(await allow(app, MAIN, feature('default_full_access')));
  assert.equal(byKey(await list(app, ''), 'full_access').on, true);
  // A fresh provider thread allows us to verify an explicit draft off overrides the saved default.
  const other = '88888888-8888-4888-8888-888888888888';
  await app.set('thread/start', { result: started(app.peer.cwd, { id: other }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id,
    capabilities: { full_access: false } }), 201);
  assert.equal(byKey(await list(app, `thread=${other}`), 'full_access').on, false);
  await json(await post(app, `/api/permissions/conversation/${other}`, {
    target: feature('default_full_access'), effect: 'allow' }), 400);
});

test('agent capability defaults seed only new conversations within their allowances', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { agent } = await json(await post(app, '/api/agents', { name: 'Artist', instructions: 'Draw.', active: true }), 201);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'blender', kind: 'stdio', command: process.execPath }), 201);
  const key = `mcp:${server.id}`;
  const endpoint = `/api/agents/${agent.id}/capability-defaults`;
  const defaults = async () => (await json(await app.request(endpoint))).capabilities;
  assert.equal(byKey(await defaults(), key).allowed, false);
  await json(await post(app, endpoint, { key, on: true }), 403);
  await json(await allow(app, agent.id, { target_type: 'mcp_server', server: server.id }));
  assert.equal(byKey(await defaults(), key).on, false, 'allowing alone does not activate');
  await json(await post(app, endpoint, { key, on: true }));
  await json(await post(app, endpoint, { key: 'web_search', on: false }));
  await app.restart();
  assert.equal(byKey(await defaults(), key).on, true);
  assert.equal(byKey(await list(app, `agentId=${agent.id}`), key).on, true);
  assert.equal(byKey(await list(app, ''), key).on, false);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  assert.equal(byKey(await list(app), key).on, true);
  assert.equal(byKey(await list(app), 'web_search').on, false);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start').params;
  assert.notEqual(start.config?.web_search, 'live');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Work', clientMessageId: CLIENT }), 202);
  const turn = (await app.inspect()).requests.find(r => r.method === 'turn/start').params;
  assert.match(turn.additionalContext.miniclaw_environment.value, /MCP servers: blender/);
  await json(await post(app, endpoint, { key, on: false }));
  assert.equal(byKey(await list(app), key).on, true, 'existing conversation keeps its switch');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/interrupt'), false);
  await json(await post(app, endpoint, { key, on: true }));
  const other = '88888888-8888-4888-8888-888888888888';
  await app.set('thread/start', { result: started(app.peer.cwd, { id: other }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id,
    capabilities: { [key]: false, web_search: true } }), 201);
  assert.equal(byKey(await list(app, `thread=${other}`), key).on, false);
  assert.equal(byKey(await list(app, `thread=${other}`), 'web_search').on, true);
  await json(await allow(app, agent.id, { target_type: 'mcp_server', server: server.id }, 'remove'));
  assert.equal(byKey(await list(app, `agentId=${agent.id}`), key).on, false, 'default cannot bypass revoked access');
  await json(await post(app, endpoint, { key: 'not-a-capability', on: true }), 400);
  await json(await post(app, `/api/agents/${MAIN}/capability-defaults`, { key: 'questions', on: false }));
  assert.equal(byKey(await list(app, ''), 'questions').on, false);
  assert.equal(byKey(await list(app, `agentId=${agent.id}`), 'questions').on, true);
});
