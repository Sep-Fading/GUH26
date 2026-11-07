// Checks durable notices against synthetic failures.
import assert from 'node:assert/strict';
// Runs isolated notification and ownership cases.
import { test } from 'node:test';
// Injects storage faults only into disposable databases.
import { DatabaseSync } from 'node:sqlite';
// Resolves paths inside disposable test fixtures.
import { join } from 'node:path';
// Starts disposable backends and observes asynchronous results.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies public protocol-shaped failure event fixtures.
import { THREAD, TURN, CLIENT, signedOut, signedIn, turnDone } from './support/fixtures.mjs';

// Sends a same-origin mutation with JSON.
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
// Reads notifications through the authenticated management endpoint.
const notices = async app => (await json(await app.request('/api/notifications'))).notifications;
// Starts a synthetic turn that can fail without inference.
async function start(app) {
  await json(await post(app, '/api/threads'), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Private prompt sentinel', clientMessageId: CLIENT,
  }), 202);
}

test('failure notice persists safely and deduplicates read events', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual(await notices(app), []);
  await start(app);
  const failure = turnDone('failed');
  failure.params.turn.error = { message: 'SECRET_TOKEN /private/sensitive.png screenshot sentinel' };
  await app.emit(failure, failure);
  const list = await eventually(() => notices(app), rows => rows.length === 1);
  assert.equal(list[0].kind, 'response_failed');
  assert.equal(list[0].threadId, THREAD);
  assert.equal(list[0].read, false);
  assert.match(list[0].message, /could not complete/);
  assert.doesNotMatch(JSON.stringify(list), /SECRET_TOKEN|private|screenshot|sentinel/);
  await json(await post(app, `/api/notifications/${list[0].id}/read`));
  await app.emit(failure);
  assert.equal((await notices(app))[0].read, true);
  await app.restart();
  const restored = await notices(app);
  assert.equal(restored.length, 1);
  assert.equal(restored[0].id, list[0].id);
  assert.equal(restored[0].read, true);
});

test('notices reject foreign origins, accounts and signed-out reads', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await start(app); await app.emit(turnDone('failed'));
  const list = await eventually(() => notices(app), rows => rows.length === 1);
  await json(await app.request(`/api/notifications/${list[0].id}/read`, {
    method: 'POST', body: {}, headers: { Origin: 'https://evil.test' },
  }), 403);
  await app.set('account/read', { result: { account: {
    ...signedIn.account, email: 'other@example.test',
  } } });
  assert.deepEqual(await notices(app), []);
  await json(await post(app, `/api/notifications/${list[0].id}/read`), 404);
  await app.set('account/read', { result: signedOut });
  await json(await app.request('/api/notifications'), 401);
  await json(await post(app, `/api/notifications/${list[0].id}/read`), 401);
});

test('completed and unrelated turns do not produce failures', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await start(app);
  await app.emit({ method: 'turn/completed', params: {
    threadId: THREAD, turn: { id: 'unrelated', status: 'failed' },
  } }, turnDone());
  await eventually(async () => (await json(await app.request(`/api/threads/${THREAD}`))).activeTurn,
    turn => turn === null);
  assert.deepEqual(await notices(app), []);
});

test('unexpected runtime exit persists one safe application notice', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await app.request('/api/account'));
  await app.peer.call({ op: 'shutdown' });
  await eventually(async () => (await json(await app.request('/api/health'))).codex,
    status => status === 'unavailable');
  await app.restart();
  const list = await notices(app);
  assert.equal(list.length, 1);
  assert.equal(list[0].kind, 'runtime_disconnected');
  assert.equal(list[0].threadId, null);
});

test('missing account identity cannot read previous notices', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await start(app); await app.emit(turnDone('failed'));
  await eventually(() => notices(app), rows => rows.length === 1);
  await app.set('account/read', { result: { account: { type: 'chatgpt', email: null } } });
  await json(await app.request('/api/notifications'), 503);
});


test('retention is bounded and storage write failures remain visible', async t => {
  const app = await launchApp();
  let db;
  t.after(async () => { db?.close(); await app.close(); });
  await start(app); await app.emit(turnDone('failed'));
  await eventually(() => notices(app), rows => rows.length === 1);
  db = new DatabaseSync(join(app.peer.dir, 'agents.sqlite3'));
  const owner = db.prepare('SELECT owner FROM notifications').get().owner;
  const insert = db.prepare(`INSERT INTO notifications
    (id, owner, event, kind, created_at) VALUES (?, ?, ?, 'response_failed', 1900000000)`);
  db.exec('BEGIN');
  for (let i = 0; i < 1005; i++) insert.run(`old-${i}`, owner, `old-${i}`);
  db.exec('COMMIT');
  await app.set('turn/start', { result: { turn: { id: 'next-turn', status: 'inProgress', items: [] } } });
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Another synthetic turn', clientMessageId: '019a5555-5555-7555-8555-555555555555',
  }), 202);
  await app.emit({ method: 'turn/completed', params: {
    threadId: THREAD, turn: { id: 'next-turn', status: 'failed' },
  } });
  await eventually(() => db.prepare('SELECT count(*) AS n FROM notifications').get().n, n => n === 1000);
  assert.equal((await notices(app)).length, 1000);
  db.exec(`CREATE TRIGGER fail_notice BEFORE INSERT ON notifications BEGIN SELECT RAISE(ABORT, 'PRIVATE_SENTINEL'); END`);
  await app.set('turn/start', { result: { turn: { id: 'third-turn', status: 'inProgress', items: [] } } });
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Storage failure', clientMessageId: '019a6666-6666-7666-8666-666666666666',
  }), 202);
  await app.emit({ method: 'turn/completed', params: {
    threadId: THREAD, turn: { id: 'third-turn', status: 'failed' },
  } });
  await eventually(async () => (await json(await app.request('/api/notifications'))).storageError, Boolean);
  assert.doesNotMatch(app.logs(), /PRIVATE_SENTINEL/);
  await app.set('account/read', { result: { account: { ...signedIn.account, email: 'other@example.test' } } });
  const other = await json(await app.request('/api/notifications'));
  assert.deepEqual(other.notifications, []);
  assert.equal(other.storageError, false);
});

test('account changes invalidate pending notification reads', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await start(app); await app.emit(turnDone('failed'));
  await eventually(() => notices(app), rows => rows.length === 1);
  await app.set('account/read', { hold: true });
  const pending = app.request('/api/notifications');
  const state = await eventually(() => app.inspect(), state => state.held.some(r => r.method === 'account/read'));
  await app.emit({ method: 'account/updated', params: { authMode: null } });
  const held = state.held.find(r => r.method === 'account/read');
  await app.peer.call({ op: 'release', id: held.id, reply: { result: signedIn } });
  await json(await pending, 503);
});

test('"mark all as read" reads every notice of this account at once and survives a restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await start(app);
  const first = turnDone('failed');
  await app.emit(first);
  await eventually(() => notices(app), rows => rows.length === 1);
  // A second failure, from a lost runtime connection.
  await app.emit({ method: 'miniclaw/disconnected', params: {} }).catch(() => {});
  const before = await notices(app);
  assert.ok(before.length >= 1 && before.every(row => !row.read));
  const done = await json(await post(app, '/api/notifications/read-all'));
  assert.equal(done.read, before.length);
  assert.ok((await notices(app)).every(row => row.read));
  assert.equal((await json(await post(app, '/api/notifications/read-all'))).read, 0, 'nothing left to mark');
  await app.restart();
  assert.ok((await notices(app)).every(row => row.read));
  assert.equal((await app.request('/api/notifications/read-all', { method: 'POST', body: {}, headers: { Origin: 'https://evil.example' } })).status, 403);
});

test('clear read preserves unread and other accounts across restarts', async t => {
  const app = await launchApp();
  let db;
  t.after(async () => { db?.close(); await app.close(); });
  await start(app); await app.emit(turnDone('failed'));
  const [unread] = await eventually(() => notices(app), rows => rows.length === 1);
  db = new DatabaseSync(join(app.peer.dir, 'agents.sqlite3'));
  const owner = db.prepare('SELECT owner FROM notifications').get().owner;
  const insert = db.prepare(`INSERT INTO notifications
    (id, owner, event, kind, created_at, read) VALUES (?, ?, ?, 'response_failed', 1900000000, 1)`);
  insert.run('read-1', owner, 'read-1');
  insert.run('read-2', owner, 'read-2');
  insert.run('other-read', 'other-owner', 'other-read');
  await json(await app.request('/api/notifications/clear-read', {
    method: 'POST', body: {}, headers: { Origin: 'https://evil.test' },
  }), 403);
  const missingHeader = await fetch(`${app.url}/api/notifications/clear-read`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}',
  });
  assert.equal(missingHeader.status, 403);
  assert.equal((await json(await post(app, '/api/notifications/clear-read'))).cleared, 2);
  assert.deepEqual(await notices(app), [unread]);
  assert.equal(db.prepare('SELECT read FROM notifications WHERE id = ?').get('other-read').read, 1);
  assert.equal((await json(await post(app, '/api/notifications/clear-read'))).cleared, 0);
  await app.restart();
  assert.deepEqual(await notices(app), [unread]);
  await json(await post(app, `/api/notifications/${unread.id}/read`));
  await app.set('account/read', { result: signedOut });
  await json(await post(app, '/api/notifications/clear-read'), 401);
  assert.equal(db.prepare('SELECT count(*) AS n FROM notifications').get().n, 2);
});

test('failed clear keeps read notices and hides storage details', async t => {
  const app = await launchApp();
  let db;
  t.after(async () => { db?.close(); await app.close(); });
  await start(app); await app.emit(turnDone('failed'));
  const [notice] = await eventually(() => notices(app), rows => rows.length === 1);
  await json(await post(app, `/api/notifications/${notice.id}/read`));
  db = new DatabaseSync(join(app.peer.dir, 'agents.sqlite3'));
  db.exec(`CREATE TRIGGER fail_clear BEFORE DELETE ON notifications
    BEGIN SELECT RAISE(ABORT, 'PRIVATE_SENTINEL'); END`);
  const response = await post(app, '/api/notifications/clear-read');
  assert.equal(response.status, 503);
  assert.doesNotMatch(await response.text(), /PRIVATE_SENTINEL/);
  assert.deepEqual(await notices(app), [{ ...notice, read: true }]);
  assert.doesNotMatch(app.logs(), /PRIVATE_SENTINEL/);
});
