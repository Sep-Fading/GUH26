// Asserts permanent denial and explicit recovery results.
import assert from 'node:assert/strict';
// Exercises isolated journal and transaction failures.
import { test } from 'node:test';
// Assigns valid fixture-only creation and chat identifiers.
import { randomUUID } from 'node:crypto';
// Runs synthetic Codex peers with observable requests.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies owned histories without any paid inference.
import { THREAD, OTHER, CLIENT, thread, started, signedIn, signedOut } from './support/fixtures.mjs';

// Sends same-origin explicit mutations without real account changes.
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
// Reads the current confirmation snapshot without changing denial records.
const preview = async app => json(await app.request('/api/creation-recovery'));
// Seeds one unknown creation with its required provisional admission.
function unknown(db, baseline) {
  const id = randomUUID();
  db.prepare('INSERT INTO chat_creations (id, baseline) VALUES (?, ?)').run(id, JSON.stringify(baseline));
  db.prepare('INSERT INTO chat_admissions (thread_id, required) VALUES (?, 0)').run(id);
  return id;
}
// Confirms exactly the state returned by the preview endpoint.
const compact = (app, state) => post(app, '/api/creation-recovery', { token: state.token, confirmed: true });
// Every test ends within a bound, so a stuck backend or database fails CI instead of hanging it
// (a native Windows run once hung here for 31 minutes without output).
const BOUNDED = { timeout: 60000 };
// Starts the backend; `app.database()` handles close before its folder is removed (harness), and
// the teardown has a bound of its own.
async function start(t, overrides) {
  const app = await launchApp(overrides);
  t.after(() => app.close(), { timeout: 30000 });
  return app;
}

test('initial tools are saved before creation and recovered with its receipt', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  const capabilities = { web_search: false, coding_session: true };
  db.exec(`CREATE TRIGGER refuse_initial_tools BEFORE INSERT ON permission_rules
    BEGIN SELECT RAISE(ABORT, 'test write failure'); END`);
  await json(await post(app, '/api/threads', { capabilities }), 503);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 0);
  db.exec('DROP TRIGGER refuse_initial_tools');
  db.exec(`CREATE TRIGGER refuse_move_tools BEFORE UPDATE ON permission_rules
    BEGIN SELECT RAISE(ABORT, 'test move failure'); END`);
  await json(await post(app, '/api/threads', { capabilities }), 503);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations WHERE thread_id IS NOT NULL').get().n, 1);
  db.exec('DROP TRIGGER refuse_move_tools');
  await app.restart();
  await json(await post(app, `/api/threads/${THREAD}/activate`));
  const { capabilities: recovered } = await json(await app.request(`/api/capabilities?thread=${THREAD}`));
  assert.equal(recovered.find(c => c.key === 'web_search').on, false);
  assert.equal(recovered.find(c => c.key === 'coding_session').on, true);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 0);
});

test('full journal compacts without admitting unknown history', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  for (let i = 0; i < 64; i++) unknown(db, [OTHER]);
  await json(await post(app, '/api/threads'), 409);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
  const state = await preview(app);
  assert.equal(state.unknownCount, 64);
  assert.equal(state.knownCount, 0);
  await json(await post(app, '/api/creation-recovery', { token: state.token, confirmed: false }), 400);
  const recovered = await json(await compact(app, state));
  assert.equal(recovered.released, 64);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 0);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_admissions').get().n, 0);
  await json(await post(app, `/api/threads/${THREAD}/activate`), 409);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { id: OTHER }) } });
  await app.set('thread/resume', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, `/api/threads/${OTHER}/activate`));
  const fresh = randomUUID();
  await app.set('thread/start', { result: started(app.peer.cwd, { id: fresh }) });
  await json(await post(app, '/api/threads'), 201);
  await app.restart();
  await json(await post(app, `/api/threads/${THREAD}/activate`), 409);
  const restored = await preview(app);
  assert.equal(restored.quarantined, true);
  assert.equal(restored.unknownCount, 0);
});

test('intersections remain monotonic and known receipts survive', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  unknown(db, [THREAD, OTHER]); unknown(db, [OTHER, CLIENT]);
  const known = unknown(db, []);
  db.prepare('UPDATE chat_creations SET thread_id=? WHERE id=?').run(CLIENT, known);
  await json(await compact(app, await preview(app)));
  assert.deepEqual(JSON.parse(db.prepare('SELECT baseline FROM creation_quarantine').get().baseline), [OTHER]);
  assert.equal(db.prepare('SELECT id FROM chat_creations WHERE thread_id=?').get(CLIENT).id, known);
  unknown(db, [THREAD]);
  await json(await compact(app, await preview(app)));
  assert.deepEqual(JSON.parse(db.prepare('SELECT baseline FROM creation_quarantine').get().baseline), []);
  unknown(db, [OTHER]);
  await json(await compact(app, await preview(app)));
  assert.deepEqual(JSON.parse(db.prepare('SELECT baseline FROM creation_quarantine').get().baseline), []);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { id: CLIENT }) } });
  await app.set('thread/resume', { result: started(app.peer.cwd, { id: CLIENT }) });
  await json(await post(app, `/api/threads/${CLIENT}/activate`));
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 0);
});

test('stale confirmation and failed writes preserve the journal', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  unknown(db, [OTHER]);
  const stale = await preview(app);
  unknown(db, []);
  await json(await compact(app, stale), 409);
  const current = await preview(app);
  db.exec(`CREATE TRIGGER refuse_quarantine BEFORE UPDATE ON creation_quarantine
    BEGIN SELECT RAISE(ABORT, 'PRIVATE_RECOVERY_SENTINEL'); END`);
  await json(await compact(app, current), 503);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 2);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_admissions').get().n, 2);
  assert.doesNotMatch(app.logs(), /PRIVATE_RECOVERY_SENTINEL/);
  db.exec('DROP TRIGGER refuse_quarantine');
  await json(await compact(app, current));
  await json(await compact(app, current), 409);
});

test('missing quarantine state rejects history and new creation', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  unknown(db, []); await json(await compact(app, await preview(app)));
  db.exec('DELETE FROM creation_quarantine');
  await json(await app.request('/api/creation-recovery'), 503);
  await json(await post(app, `/api/threads/${THREAD}/activate`), 503);
  await json(await post(app, '/api/threads'), 503);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start' || r.method === 'thread/resume'), false);
});

test('recovery rejects hostile origins and stale account reads', BOUNDED, async t => {
  const app = await start(t);
  await json(await app.request('/api/creation-recovery', { headers: { Origin: 'https://hostile.test' } }), 403);
  await app.set('account/read', { hold: true });
  const pending = app.request('/api/creation-recovery');
  const state = await eventually(app.inspect, state => state.held.some(r => r.method === 'account/read'));
  await app.emit({ method: 'account/updated', params: {} });
  await app.peer.call({ op: 'release', id: state.held.find(r => r.method === 'account/read').id, reply: { result: signedIn } });
  await json(await pending, 503);
  await app.set('account/read', { result: signedOut });
  await json(await app.request('/api/creation-recovery'), 401);
  await json(await post(app, '/api/creation-recovery', { token: '0'.repeat(64), confirmed: true }), 401);
});

test('failed deletion rolls back the fence and every provisional row', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  unknown(db, []);
  const state = await preview(app);
  db.exec(`CREATE TRIGGER refuse_cleanup BEFORE DELETE ON chat_admissions
    BEGIN SELECT RAISE(ABORT, 'cleanup failed'); END`);
  await json(await compact(app, state), 503);
  assert.deepEqual({ ...db.prepare('SELECT revision, baseline FROM creation_quarantine').get() }, { revision: 0, baseline: null });
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 1);
  db.exec('DROP TRIGGER refuse_cleanup');
  await json(await compact(app, state));
  const fence = db.prepare('SELECT revision, baseline FROM creation_quarantine').get();
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  assert.deepEqual(db.prepare('SELECT revision, baseline FROM creation_quarantine').get(), fence);
});

test('held creations finish before compaction and invalidate its preview', BOUNDED, async t => {
  const app = await start(t, { 'thread/start': { hold: true } });
  const db = app.database();
  unknown(db, [OTHER]);
  const create = post(app, '/api/threads');
  const held = await eventually(app.inspect, state => state.held.some(r => r.method === 'thread/start'));
  const state = await preview(app);
  assert.equal(state.unknownCount, 2);
  let completed = false;
  const recovering = compact(app, state).then(response => { completed = true; return response; });
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(completed, false);
  const fresh = randomUUID();
  await app.peer.call({ op: 'release', id: held.held.find(r => r.method === 'thread/start').id,
    reply: { result: started(app.peer.cwd, { id: fresh }) } });
  await json(await create, 201);
  await json(await recovering, 409);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 1);
  assert.ok(db.prepare('SELECT thread_id FROM chat_admissions WHERE thread_id=?').get(fresh));
  await json(await compact(app, await preview(app)));
});

test('ignored quarantine writes cannot release denial records', BOUNDED, async t => {
  const app = await start(t);
  const db = app.database();
  unknown(db, []);
  const state = await preview(app);
  db.exec(`CREATE TRIGGER ignore_quarantine BEFORE UPDATE ON creation_quarantine
    BEGIN SELECT RAISE(IGNORE); END`);
  await json(await compact(app, state), 503);
  assert.equal(db.prepare('SELECT count(*) n FROM chat_creations').get().n, 1);
  await json(await post(app, `/api/threads/${THREAD}/activate`), 409);
});

test('confirmed compaction removes only unknown provisional policy copies', BOUNDED, async t => {
  const app = await start(t, {
    'thread/list': { result: { data: [], nextCursor: null } },
    'thread/start': { result: {} },
  });
  const { skill } = await json(await post(app, '/api/skills', { name: 'Fixture skill', instructions: 'PRIVATE_COPY', active: true }), 201);
  const { agent } = await json(await post(app, '/api/agents', {
    name: 'Fixture agent', instructions: 'Private profile', active: true, skillIds: [skill.id],
  }), 201);
  const { project } = await json(await post(app, '/api/projects', { name: 'Fixture project',
    agentIds: [agent.id], environment: { kind: 'host' } }), 201);
  await json(await post(app, '/api/threads', { agentId: agent.id, projectId: project.id,
    skillSelection: { skillIds: [skill.id], skillGroupIds: [], revision: 0 },
  }), 503);
  const db = app.database();
  assert.equal(db.prepare('SELECT required FROM chat_admissions').get().required, 15);
  const state = await preview(app);
  assert.doesNotMatch(JSON.stringify(state), /PRIVATE_COPY|Private profile|Fixture/);
  await json(await compact(app, state));
  for (const table of ['chat_creations', 'chat_admissions', 'agent_chats', 'agent_chat_skills', 'project_chats', 'chat_skill_selections']) {
    assert.equal(db.prepare(`SELECT count(*) n FROM ${table}`).get().n, 0, table);
  }
  for (const table of ['agents', 'skills', 'projects']) assert.equal(db.prepare(`SELECT count(*) n FROM ${table}`).get().n, 1, table);
  await json(await post(app, `/api/threads/${THREAD}/activate`), 409);
});
