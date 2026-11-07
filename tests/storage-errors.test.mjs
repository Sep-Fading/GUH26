// Checks error classification and private-data redaction.
import assert from 'node:assert/strict';
// Runs storage failures against disposable databases.
import { test } from 'node:test';
// Starts a real backend with fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies owned history and synthetic identifiers.
import { THREAD, OTHER, thread, started } from './support/fixtures.mjs';

test('binding failure never creates an upstream chat', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  const db = app.database();
  db.exec(`CREATE TRIGGER fail_binding BEFORE INSERT ON agent_chats
    BEGIN SELECT RAISE(ABORT, 'fixture failure'); END`);
  await json(await app.request('/api/threads', {
    method: 'POST', body: { agentId: agent.id, environment: { kind: 'host' } },
  }), 503);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

test('failed binding move recovers the same chat after restart', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  const db = app.database();
  db.exec(`CREATE TRIGGER fail_move BEFORE UPDATE OF thread_id ON agent_chats
    BEGIN SELECT RAISE(ABORT, 'fixture failure'); END`);
  await json(await app.request('/api/threads', {
    method: 'POST', body: { agentId: agent.id, environment: { kind: 'host' } },
  }), 503);
  db.exec('DROP TRIGGER fail_move');
  await app.restart();
  const listed = await json(await app.request('/api/threads'));
  assert.equal(listed.threads.find(t => t.id === THREAD).agentId, agent.id);
  await json(await app.request(`/api/threads/${THREAD}/activate`, { method: 'POST', body: {} }));
  assert.equal((await app.request(`/api/threads/${THREAD}/computer`, {
    method: 'POST', body: { enabled: true },
  })).status, 403);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
  // Recovery restores the binding only; nothing is sent on the user's behalf.
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
  assert.deepEqual((await json(await app.request(`/api/threads/${THREAD}`))).messages, []);
});

test('lost creation acknowledgement quarantines unknown history', async t => {
  const app = await launchApp({ 'thread/start': { result: {} } });
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd, { id: OTHER })], nextCursor: null } });
  await json(await app.request('/api/threads', { method: 'POST',
    body: { agentId: agent.id, environment: { kind: 'host' } } }), 503);
  await app.restart();
  const listed = await json(await app.request('/api/threads'));
  assert.equal(listed.threads.find(t => t.id === THREAD).recoveryRequired, true);
  const blocked = await json(await app.request(`/api/threads/${THREAD}/activate`, {
    method: 'POST', body: {},
  }), 409);
  assert.equal(blocked.error.code, 'chat_creation_uncertain');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/resume'), false);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { id: OTHER }) } });
  await app.set('thread/resume', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await app.request(`/api/threads/${OTHER}/activate`, { method: 'POST', body: {} }));
  await app.set('thread/start', { result: started(app.peer.cwd, { id: '019a5555-5555-7555-8555-555555555555' }) });
  await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } }), 201);
});

test('lost binding rows never become ordinary chats', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  await json(await app.request('/api/threads', { method: 'POST',
    body: { agentId: agent.id, environment: { kind: 'host' } } }), 201);
  const db = app.database();
  db.exec('PRAGMA foreign_keys = OFF; DELETE FROM agent_chat_skills; DELETE FROM agent_chats;');
  await json(await app.request(`/api/threads/${THREAD}/computer`, {
    method: 'POST', body: { enabled: true },
  }), 503);
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}/activate`, { method: 'POST', body: {} }), 503);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/resume'), false);
});

test('disconnecting creation still commits its binding', async t => {
  const app = await launchApp({ 'thread/start': { hold: true } });
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  const controller = new AbortController();
  const response = app.request('/api/threads', {
    method: 'POST', body: { agentId: agent.id, environment: { kind: 'host' } },
    signal: controller.signal,
  }).catch(() => null);
  const state = await eventually(app.inspect, state => state.held.length === 1);
  controller.abort();
  await response;
  await app.peer.call({ op: 'release', id: state.held[0].id,
    reply: { result: started(app.peer.cwd) } });
  await eventually(async () => {
    const data = await json(await app.request('/api/threads'));
    return data.threads.find(t => t.id === THREAD)?.agentId;
  }, id => id === agent.id, 'completed binding after disconnect');
});

test('missing provisional policy cannot complete creation', async t => {
  const app = await launchApp({ 'thread/start': { hold: true } });
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Restricted', instructions: 'Keep this profile', active: true },
  }), 201);
  const db = app.database();
  const response = app.request('/api/threads', { method: 'POST',
    body: { agentId: agent.id, environment: { kind: 'host' } } });
  const state = await eventually(app.inspect, state => state.held.length === 1);
  db.exec('DELETE FROM chat_admissions; DELETE FROM agent_chat_skills; DELETE FROM agent_chats;');
  await app.peer.call({ op: 'release', id: state.held[0].id,
    reply: { result: started(app.peer.cwd) } });
  await json(await response, 503);
  await json(await app.request(`/api/threads/${THREAD}/activate`, { method: 'POST', body: {} }), 503);
});

test('upstream creation errors do not authorize unfamiliar history', async t => {
  const app = await launchApp({
    'thread/list': { result: { data: [], nextCursor: null } },
    'thread/start': { error: { code: -32603, message: 'Creation interrupted' } },
  });
  t.after(() => app.close());
  await json(await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } }), 502);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd) } });
  const response = await json(await app.request(`/api/threads/${THREAD}/activate`, {
    method: 'POST', body: {},
  }), 409);
  assert.equal(response.error.code, 'chat_creation_uncertain');
});

test('storage failures are diagnosed without leaking records', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const secret = 'PRIVATE_STORAGE_CANARY';
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Saved profile', instructions: secret },
  }), 201);
  const db = app.database();
  db.exec('ALTER TABLE agents RENAME TO retained_agents');
  db.exec(`CREATE VIEW agents AS SELECT * FROM ${secret}`);
  const response = await json(await app.request('/api/agents'), 503);
  assert.equal(response.error.code, 'agent_storage_unavailable');
  assert.doesNotMatch(JSON.stringify(response), new RegExp(secret));
  db.exec('DROP VIEW agents; ALTER TABLE retained_agents RENAME TO agents');
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, [agent]);
  await eventually(app.logs, logs => logs.includes('storage_failure kind=sqlite code=1'),
    'safe storage diagnosis');
  assert.doesNotMatch(app.logs(), new RegExp(secret));
  assert.ok(!app.logs().includes(app.peer.dir), 'Logs must omit storage paths');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'turn/start'), false);
});

test('invalid saved definitions have a separate diagnosis', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { agent } = await json(await app.request('/api/agents', {
    method: 'POST', body: { name: 'Saved profile', instructions: 'Keep this record' },
  }), 201);
  const db = app.database();
  const saved = db.prepare('SELECT definition FROM agents WHERE id=?').get(agent.id).definition;
  db.prepare('UPDATE agents SET definition=? WHERE id=?').run('PRIVATE_INVALID_JSON', agent.id);
  await json(await app.request('/api/agents'), 503);
  db.prepare('UPDATE agents SET definition=? WHERE id=?').run(saved, agent.id);
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, [agent]);
  await eventually(app.logs, logs => logs.includes('storage_failure kind=json'),
    'saved definition diagnosis');
  assert.doesNotMatch(app.logs(), /PRIVATE_INVALID_JSON/);
});
