// Checks group isolation and effective instructions.
import assert from 'node:assert/strict';
// Runs disposable backend behavior acceptance cases.
import { test } from 'node:test';
// Generates independent message identifiers for retries.
import { randomUUID } from 'node:crypto';
// Drives real HTTP routes with synthetic Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies owned threads and synthetic completion events.
import { THREAD, OTHER, started, signedOut } from './support/fixtures.mjs';

// Sends a guarded local mutation and checks its response status.
const post = async (app, path, body, status = 200) =>
  json(await app.request(path, { method: 'POST',
    body: path === '/api/threads' || path === '/api/projects'
      ? { environment: { kind: 'host' }, ...body } : body }), status);
// Creates a skill without activating its independent selection.
const skill = async (app, name, active = false) =>
  (await post(app, '/api/skills', { name, instructions: `${name}_MARKER`, active }, 201)).skill;
// Creates an enabled group with explicitly enabled members.
const group = async (app, name, ids) => (await post(app, '/api/skill-groups', {
  name, active: true, members: ids.map(skillId => ({ skillId, enabled: true })),
}, 201)).group;
// Saves one observed group revision without modifying its skills.
const edit = async (app, value, changes) => {
  const { id, ...body } = value;
  return (await post(app, `/api/skill-groups/${id}`, { ...body, ...changes })).group;
};
// Starts another message and returns its per-turn skill instructions.
async function send(app, id = THREAD) {
  const { turnId } = await post(app, `/api/threads/${id}/messages`, {
    text: 'Work', clientMessageId: randomUUID(),
  }, 202);
  const request = (await app.inspect()).requests.filter(r => r.method === 'turn/start').at(-1);
  await app.emit({ method: 'turn/completed', params: {
    threadId: id, turn: { id: turnId, status: 'completed', items: [] },
  } });
  await eventually(() => app.request(`/api/threads/${id}`).then(json), value => !value.activeTurn);
  return request.params.additionalContext?.miniclaw_skills.value ?? '';
}

test('group revisions, members, and independent activation persist', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await json(await app.request('/api/skill-groups'))).groups, []);
  const s = await skill(app, 'Shared');
  const a = await group(app, 'Writing', [s.id]);
  const b = await group(app, 'Review', [s.id]);
  const changed = await edit(app, a, { members: [{ skillId: s.id, enabled: false }] });
  await post(app, `/api/skill-groups/${a.id}`, a, 400);
  const { id, ...old } = a;
  await post(app, `/api/skill-groups/${id}`, old, 409);
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/skill-groups'))).groups, [changed, b]);
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, [s]);
});

test('overlapping groups and individual selections retain their own contributions', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Shared', true);
  const a = await group(app, 'A', [s.id]), b = await group(app, 'B', [s.id]);
  await post(app, '/api/threads', {}, 201);
  let selection = (await post(app, `/api/threads/${THREAD}/skills`, {
    skillIds: [s.id], skillGroupIds: [a.id, b.id], revision: 0,
  })).selection;
  assert.equal((await send(app)).split('Shared_MARKER').length - 1, 1);
  await edit(app, a, { members: [] });
  const off = await edit(app, b, { active: false });
  assert.match(await send(app), /Shared_MARKER/);
  selection = (await post(app, `/api/threads/${THREAD}/skills`, {
    ...selection, skillIds: [],
  })).selection;
  assert.doesNotMatch(await send(app), /Shared_MARKER/);
  await edit(app, off, { active: true });
  assert.match(await send(app), /Shared_MARKER/);
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual((await json(await app.request(`/api/threads/${THREAD}/skills`))).selection, selection);
  assert.match(await send(app), /Shared_MARKER/);
});

test('group edits wait for the next message and do not interrupt a response', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const one = await skill(app, 'One'), two = await skill(app, 'Two');
  let g = await group(app, 'Writing', [one.id]);
  const { project } = await post(app, '/api/projects', {
    name: 'Project', skillGroupIds: [g.id],
  }, 201);
  await post(app, '/api/threads', { projectId: project.id }, 201);
  const { turnId } = await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'First', clientMessageId: randomUUID(),
  }, 202);
  g = await edit(app, g, { members: [{ skillId: two.id, enabled: true }] });
  const requests = (await app.inspect()).requests;
  assert.equal(requests.some(r => r.method === 'turn/interrupt'), false);
  assert.match(requests.find(r => r.method === 'turn/start').params
    .additionalContext.miniclaw_skills.value, /One_MARKER/);
  await app.emit({ method: 'turn/completed', params: {
    threadId: THREAD, turn: { id: turnId, status: 'completed', items: [] },
  } });
  await eventually(() => app.request(`/api/threads/${THREAD}`).then(json), value => !value.activeTurn);
  const next = await send(app);
  assert.match(next, /Two_MARKER/); assert.doesNotMatch(next, /One_MARKER/);
  await post(app, `/api/skill-groups/${g.id}/delete`, { revision: g.revision });
  assert.doesNotMatch(await send(app), /Two_MARKER/);
  assert.equal((await json(await app.request('/api/skills'))).skills.length, 2);
});

test('project groups exclude named agents and deduplicate pinned individual skills', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Project', true), own = await skill(app, 'Own', true);
  const g = await group(app, 'Shared', [s.id]);
  const { agent } = await post(app, '/api/agents', { name: 'Specialist', active: true,
    instructions: 'Specialist instructions', skillIds: [own.id] }, 201);
  const { project } = await post(app, '/api/projects', { name: 'Game', agentIds: [agent.id],
    skillIds: [s.id], skillGroupIds: [g.id] }, 201);
  await post(app, '/api/threads', { projectId: project.id }, 201);
  assert.doesNotMatch(await send(app), /Project_MARKER/);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await post(app, '/api/threads', { projectId: project.id, agentId: agent.id }, 201);
  assert.doesNotMatch(await send(app, OTHER), /Project_MARKER/);
});

test('group routes reject invalid references, bounds, foreign tasks, and cross-origin writes', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Skill');
  for (const body of [{ name: '' }, { name: 'x'.repeat(81) }, { name: 'x', access: {} },
    { name: 'x', members: [{ skillId: randomUUID(), enabled: true }] },
    { name: 'x', members: Array(2).fill({ skillId: s.id, enabled: true }) }]) {
    assert.ok([400, 404].includes((await app.request('/api/skill-groups', { method: 'POST', body })).status));
  }
  assert.equal((await app.request('/api/skill-groups', { method: 'POST', body: { name: 'x' },
    headers: { Origin: 'https://foreign.example' } })).status, 403);
  await post(app, `/api/threads/${randomUUID()}/skills`, { skillIds: [], skillGroupIds: [], revision: 0 }, 404);
  await app.set('account/read', { result: signedOut });
  await json(await app.request('/api/skill-groups'), 401);
});

test('member switches survive master cycling and removed skills leave other groups intact', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const a = await skill(app, 'A'), b = await skill(app, 'B');
  let g = await group(app, 'Group', [a.id, b.id]);
  g = await edit(app, g, { members: [{ skillId: a.id, enabled: false }, { skillId: b.id, enabled: true }] });
  await post(app, '/api/threads', { skillSelection: {
    skillIds: [], skillGroupIds: [g.id], revision: 0,
  } }, 201);
  let text = await send(app);
  assert.match(text, /B_MARKER/); assert.doesNotMatch(text, /A_MARKER/);
  g = await edit(app, g, { active: false });
  assert.doesNotMatch(await send(app), /B_MARKER/);
  g = await edit(app, g, { active: true });
  text = await send(app);
  assert.match(text, /B_MARKER/); assert.doesNotMatch(text, /A_MARKER/);
  await post(app, `/api/skills/${b.id}/delete`, { revision: b.revision });
  assert.doesNotMatch(await send(app), /B_MARKER/);
  assert.equal((await json(await app.request('/api/skills'))).skills[0].id, a.id);
});

test('oversized effective instructions fail before reserving a message', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const ids = [];
  for (let index = 0; index < 9; index++) {
    const { skill } = await post(app, '/api/skills', { name: `Skill ${index}`,
      instructions: 'x'.repeat(16384) }, 201);
    ids.push(skill.id);
  }
  const g = await group(app, 'Too much', ids);
  await post(app, '/api/threads', { skillSelection: {
    skillIds: [], skillGroupIds: [g.id], revision: 0,
  } }, 201);
  const result = await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Work', clientMessageId: randomUUID(),
  }, 409);
  assert.equal(result.error.code, 'skill_group_limit');
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.pending, false); assert.deepEqual(snapshot.messages, []);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
});

test('changing group selections does not repin an individually selected skill', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Pinned', true);
  const g = await group(app, 'Empty', []);
  await post(app, '/api/threads', { skillSelection: {
    skillIds: [s.id], skillGroupIds: [g.id], revision: 0,
  } }, 201);
  const { id, ...definition } = s;
  await post(app, `/api/skills/${id}`, { ...definition, instructions: 'NEW_TEXT_MARKER' });
  const { selection } = await json(await app.request(`/api/threads/${THREAD}/skills`));
  await post(app, `/api/threads/${THREAD}/skills`, { ...selection, skillGroupIds: [] });
  const text = await send(app);
  assert.match(text, /Pinned_MARKER/); assert.doesNotMatch(text, /NEW_TEXT_MARKER/);
});

test('conversation skills open for a saved conversation that is not loaded yet, such as after a restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await post(app, '/api/threads', { environment: { kind: 'host' } }, 201);
  const own = await skill(app, 'Later', true);
  await app.restart();
  // Nothing has opened the conversation since the restart.
  const read = await json(await app.request(`/api/threads/${THREAD}/skills`));
  assert.deepEqual(read.selection.skillIds ?? [], []);
  const saved = await post(app, `/api/threads/${THREAD}/skills`, { ...read.selection, skillIds: [own.id] });
  assert.deepEqual(saved.selection.skillIds, [own.id]);
  // A conversation MiniClaw has never seen is still refused.
  assert.equal((await app.request(`/api/threads/${OTHER}/skills`)).status, 404);
});

test('agents save groups and resolve live members from their pinned selection', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const one = await skill(app, 'AgentOne'), two = await skill(app, 'AgentTwo');
  let g = await group(app, 'Agent group', [one.id]);
  const { agent } = await post(app, '/api/agents', { name: 'Grouped', active: true,
    instructions: 'Use selected skills.', skillGroupIds: [g.id] }, 201);
  assert.deepEqual(agent.skillGroupIds, [g.id]);
  await post(app, '/api/threads', { agentId: agent.id }, 201);
  assert.match(await send(app), /AgentOne_MARKER/);
  const { id, ...definition } = agent;
  await post(app, `/api/agents/${id}`, { ...definition, skillGroupIds: [] });
  g = await edit(app, g, { members: [{ skillId: two.id, enabled: true }] });
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  const text = await send(app);
  assert.match(text, /AgentTwo_MARKER/); assert.doesNotMatch(text, /AgentOne_MARKER/);
  assert.deepEqual((await json(await app.request('/api/agents'))).agents[0].skillGroupIds, []);
  g = await edit(app, g, { active: false });
  assert.doesNotMatch(await send(app), /AgentTwo_MARKER/);
  g = await edit(app, g, { active: true });
  await post(app, `/api/skill-groups/${g.id}/delete`, { revision: g.revision });
  assert.doesNotMatch(await send(app), /AgentTwo_MARKER/);
  for (const skillGroupIds of [[randomUUID()], [g.id, g.id], Array.from({ length: 17 }, () => randomUUID())]) {
    const response = await app.request('/api/agents', { method: 'POST', body: {
      name: 'Invalid', instructions: 'Work.', skillGroupIds } });
    assert.ok([400, 404].includes(response.status));
  }
});
