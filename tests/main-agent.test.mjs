// Checks the main agent: the configuration of conversations that have no agent. It holds
// capabilities (switches and MCP servers) and a default model and effort, and nothing else.
import assert from 'node:assert/strict';
// Registers the main agent tests.
import { test } from 'node:test';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
// Builds the fake's answer for a second conversation.
import { started } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const MAIN = '/api/permissions/agent/00000000-0000-0000-0000-000000000000';
const OTHER = '019a1111-1111-7111-8111-111111111112';
// Requirement change 2026-09-21: the main agent also holds skills and skill groups.
// Requirement change 2026-10-11: it also holds its delegation limits, by default 3 per message and 10 minutes.
const NONE = { skillIds: [], skillGroupIds: [], delegation: { maxPerTurn: 3, minutes: 10 } };
const last = async (app, method) => (await app.inspect()).requests.filter(request => request.method === method).at(-1);

test('the main agent switches a capability for plain conversations only', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await json(await app.request(MAIN))).rules, []);
  await json(await post(app, MAIN, { target: { target_type: 'feature', name: 'no_web_search' }, effect: 'allow' }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  assert.equal((await last(app, 'thread/start')).params.config?.web_search, undefined, 'a plain conversation follows the main agent');
  // The conversation still has the last word, and its settings say where the choice came from.
  const { thread } = await json(await app.request('/api/threads')).then(list => ({ thread: list.threads[0] }));
  const summary = await json(await app.request(`/api/permissions/conversation/${thread.id}`));
  assert.deepEqual(summary.effective.map(row => [row.target.name, row.decision, row.source]), [['no_web_search', 'allow', 'agent']]);
  // A named agent has its own configuration and does not inherit the main agent's.
  const { agent } = await json(await post(app, '/api/agents', { name: 'Scout', instructions: 'Look things up.', active: true }), 201);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  assert.equal((await last(app, 'thread/start')).params.config.web_search, 'live');
});

test('the main agent takes switches and MCP servers, not file, command or host rules', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio', command: process.execPath }), 201);
  await json(await post(app, MAIN, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  for (const target of [{ target_type: 'folder', path: '/private/tmp', action: 'read' }, { target_type: 'host', host: 'example.com' },
    { target_type: 'command', kind: 'shell', text: 'ls', cwd: '/private/tmp' }, { target_type: 'application', id: 'com.apple.TextEdit', action: 'control' }]) {
    assert.equal((await post(app, MAIN, { target, effect: 'allow' })).status, 400, target.target_type);
  }
  assert.equal((await post(app, MAIN, { target: { target_type: 'feature', name: 'network' }, effect: 'block' })).status, 400, 'only a conversation blocks');
  assert.equal((await json(await app.request(MAIN))).rules.length, 1);
});

test('the main agent\'s default model and effort start a plain conversation and survive a restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual(await json(await app.request('/api/main-agent')), { model: null, effort: null, speed: null, ...NONE });
  for (const bad of [{ model: '', effort: null }, { model: 'x'.repeat(129), effort: null }, { model: null, effort: 'low' }, { model: 'fixture-model', effort: 'x'.repeat(33) }, { model: 'fixture-model' , extra: 1 },
    { model: null, speed: 'priority' }, { model: 'fixture-model', speed: '' }]) {
    assert.equal((await post(app, '/api/main-agent', bad)).status, 400, JSON.stringify(bad));
  }
  assert.deepEqual(await json(await post(app, '/api/main-agent', { model: 'fixture-model', effort: 'low' })), { model: 'fixture-model', effort: 'low', speed: null, ...NONE });
  // A default speed is kept beside them; like the model it is checked when a conversation starts.
  assert.deepEqual(await json(await post(app, '/api/main-agent', { model: 'fixture-model', effort: 'low', speed: 'priority' })), { model: 'fixture-model', effort: 'low', speed: 'priority', ...NONE });
  await app.restart();
  assert.deepEqual(await json(await app.request('/api/main-agent')), { model: 'fixture-model', effort: 'low', speed: 'priority', ...NONE });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const start = (await last(app, 'thread/start')).params;
  assert.deepEqual([start.model, start.config.model_reasoning_effort, start.serviceTier], ['fixture-model', 'low', 'priority']);
  // A choice made in the composer wins over the default.
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: 'fixture-model', effort: 'high' }), 201);
  assert.equal((await last(app, 'thread/start')).params.config.model_reasoning_effort, 'high');
  await json(await post(app, '/api/main-agent', { model: null, effort: null }));
  assert.deepEqual(await json(await app.request('/api/main-agent')), { model: null, effort: null, speed: null, ...NONE });
});

test('the main agent\'s skills and skill groups reach every conversation that has no agent, read fresh each turn', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const skill = async (name, extra = {}) => (await json(await post(app, '/api/skills', { name, instructions: `Rule of ${name}.`, active: true, ...extra }), 201)).skill;
  const [direct, member, asleep] = [await skill('Direct'), await skill('Member'), await skill('Asleep', { active: false })];
  const { group } = await json(await post(app, '/api/skill-groups', { name: 'Universal', active: true,
    members: [{ skillId: member.id, enabled: true }] }), 201);
  const saved = await json(await post(app, '/api/main-agent', { model: null, effort: null, skillIds: [direct.id, asleep.id], skillGroupIds: [group.id] }));
  assert.deepEqual([saved.skillIds, saved.skillGroupIds], [[direct.id, asleep.id], [group.id]]);
  // Saving the model alone leaves the skills as they are.
  const kept = await json(await post(app, '/api/main-agent', { model: 'fixture-model', effort: null }));
  assert.deepEqual(kept.skillIds, [direct.id, asleep.id]);
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/main-agent'))).skillGroupIds, [group.id]);

  const context = async () => (await last(app, 'turn/start')).params.additionalContext.miniclaw_skills.value;
  const { THREAD, CLIENT, turnDone } = await import('./support/fixtures.mjs');
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Hello', clientMessageId: CLIENT }), 202);
  const first = await context();
  assert.match(first, /Rule of Direct\./);
  assert.match(first, /Rule of Member\./);
  assert.doesNotMatch(first, /Rule of Asleep\./, 'an inactive skill is left out and does not block the message');
  // A skill added to the group later reaches the same conversation on its next message.
  const late = await skill('Late');
  await json(await post(app, `/api/skill-groups/${group.id}`, { name: 'Universal', active: true, revision: group.revision,
    members: [{ skillId: member.id, enabled: true }, { skillId: late.id, enabled: true }] }));
  await app.emit(turnDone());
  const { eventually } = await import('./support/harness.mjs');
  await eventually(async () => json(await app.request(`/api/threads/${THREAD}`)), value => value.activeTurn === null, 'turn end');
  await json(await post(app, `/api/threads/${THREAD}/messages`, { text: 'Again', clientMessageId: crypto.randomUUID() }), 202);
  assert.match(await context(), /Rule of Late\./);

  // A named agent takes nothing from the main agent.
  const { agent } = await json(await post(app, '/api/agents', { name: 'Scout', instructions: 'Look.', active: true }), 201);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: agent.id }), 201);
  await json(await post(app, `/api/threads/${OTHER}/messages`, { text: 'Hi', clientMessageId: crypto.randomUUID() }), 202);
  assert.doesNotMatch(await context(), /Rule of Direct\./);

  for (const bad of [{ skillIds: ['not-a-uuid'] }, { skillIds: Array(9).fill(direct.id) }, { skillIds: [direct.id, direct.id] },
    { skillGroupIds: ['019a0000-0000-7000-8000-000000000000'] }, { skillIds: ['019a0000-0000-7000-8000-000000000000'] }]) {
    const status = (await post(app, '/api/main-agent', { model: null, effort: null, ...bad })).status;
    assert.ok([400, 404].includes(status), `${JSON.stringify(bad)} -> ${status}`);
  }
});

// Requirement 2026-10-11 (user decision): "add the ability to configure delegation settings on main agent so we
// are not locked into a value for the time limit". The same bounds as a named agent: 1 to 10, 1 to 1440 minutes.
test('the main agent\'s delegation limits are saved, kept when other settings change, and survive a restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await json(await app.request('/api/main-agent'))).delegation, { maxPerTurn: 3, minutes: 10 }, 'unchanged default');
  for (const delegation of [{ maxPerTurn: 0, minutes: 10 }, { maxPerTurn: 11, minutes: 10 }, { maxPerTurn: 3, minutes: 0 },
    { maxPerTurn: 3, minutes: 1441 }, { maxPerTurn: 3 }, { maxPerTurn: 3, minutes: 10, extra: 1 }, { maxPerTurn: 3, minutes: 1.5 }]) {
    assert.equal((await post(app, '/api/main-agent', { model: null, effort: null, delegation })).status, 400, JSON.stringify(delegation));
  }
  const saved = await json(await post(app, '/api/main-agent', { model: null, effort: null, delegation: { maxPerTurn: 10, minutes: 1440 } }));
  assert.deepEqual(saved.delegation, { maxPerTurn: 10, minutes: 1440 });
  // Saving the model alone leaves the limits as they are.
  const kept = await json(await post(app, '/api/main-agent', { model: 'fixture-model', effort: null }));
  assert.deepEqual(kept.delegation, { maxPerTurn: 10, minutes: 1440 });
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/main-agent'))).delegation, { maxPerTurn: 10, minutes: 1440 });
  // A named agent keeps its own limits.
  const { agent } = await json(await post(app, '/api/agents', { name: 'Scout', instructions: 'Look.', active: true }), 201);
  assert.deepEqual(agent.delegation, { maxPerTurn: 3, minutes: 10 });
});
