// Checks authenticated supervisor calls without booting macOS.
import assert from 'node:assert/strict';
// Runs private control protocol cases.
import { test } from 'node:test';
// Signs one request per connection.
import { createHmac, randomBytes } from 'node:crypto';
// Owns and cleans disposable control storage.
import { chmod, mkdtemp, rm } from 'node:fs/promises';
// Opens only a local Unix socket.
import { connect } from 'node:net';
// Names the private fixture directory.
import { tmpdir } from 'node:os';
// Resolves the control endpoint.
import { join } from 'node:path';
// Starts the real control service.
import { startControl } from '../scripts/vm/control.mjs';

function request(path, key, method, nonce = randomBytes(32).toString('hex'), sign = true,
  vmId = undefined, requestId = undefined) {
  const mac = createHmac('sha256', key).update(`${nonce}:${method}${vmId ? `:${vmId}` : ''}`
    + `${requestId ? `:${requestId}` : ''}`)
    .digest('hex');
  return new Promise((resolve, reject) => {
    const socket = connect(path);
    let data = '';
    socket.setTimeout(2000, () => socket.destroy(new Error('Control timed out')));
    socket.on('error', reject);
    socket.on('connect', () => socket.write(`${JSON.stringify({ nonce, method, vmId, requestId,
      mac: sign ? mac : '0'.repeat(64) })}\n`));
    socket.on('data', value => { data += value; });
    socket.on('end', () => {
      try { resolve(JSON.parse(data.trim())); } catch (error) { reject(error); }
    });
  });
}

test('supervisor status is read-only and only signed start reaches Master', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  let starts = 0;
  const service = await startControl(dir, key, {
    status: () => ({ state: starts ? 'starting' : 'off' }),
    start: async () => { starts++; return { state: 'starting' }; },
  });
  t.after(() => service.close());
  const path = join(dir, 'control.sock');
  assert.deepEqual(await request(path, key, 'status'),
    { ok: true, status: { state: 'off' } });
  assert.equal(starts, 0);
  const denied = await request(path, key, 'start', undefined, false);
  assert.equal(denied.ok, false);
  assert.equal(starts, 0);
  const nonce = randomBytes(32).toString('hex');
  assert.deepEqual(await request(path, key, 'start', nonce),
    { ok: true, status: { state: 'starting' } });
  assert.equal(starts, 1);
  assert.equal((await request(path, key, 'start', nonce)).ok, false);
  assert.equal(starts, 1);
  assert.equal((await request(path, key, 'unknown')).ok, false);
});

test('dedicated provisioning and retry require their own signed targets', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-provision-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const vmId = '019a1111-1111-7111-8111-111111111111';
  const requestId = '019a2222-2222-7222-8222-222222222222';
  const fullRequestId = '019a3333-3333-7333-8333-333333333333';
  let provisions = 0, retries = 0, cancellations = 0;
  const service = await startControl(dir, key, {
    status: () => ({ state: 'failed' }), start: async () => ({ state: 'starting' }),
    provision: async id => { provisions++;
      if (id === fullRequestId) throw new Error('Project VM limit reached');
      assert.equal(id, requestId);
      return { vmId, state: 'starting', stage: 'preparing' }; },
    retry: async id => { assert.equal(id, vmId); retries++; return { state: 'starting' }; },
    cancel: async id => { assert.equal(id, vmId); cancellations++; return { state: 'failed' }; },
  });
  t.after(() => service.close());
  const path = join(dir, 'control.sock');
  assert.equal((await request(path, key, 'provision', undefined, false,
    undefined, requestId)).ok, false);
  assert.equal(provisions, 0);
  const created = await request(path, key, 'provision', undefined, true,
    undefined, requestId);
  assert.equal(created.ok, true);
  assert.equal(created.machine.vmId, vmId);
  assert.equal(provisions, 1);
  const full = await request(path, key, 'provision', undefined, true,
    undefined, fullRequestId);
  assert.equal(full.error.message, 'Project VM limit reached.');
  assert.equal((await request(path, key, 'retry', undefined, true, vmId)).ok, true);
  assert.equal((await request(path, key, 'cancel', undefined, true, vmId)).ok, true);
  assert.equal(retries, 1);
  assert.equal(cancellations, 1);
  assert.equal((await request(path, key, 'retry', undefined, true, 'master')).ok, false);
});

test('dedicated requests bind the VM identity into the signed control call', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-dedicated-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const vmId = '019a1111-1111-7111-8111-111111111111';
  const started = [];
  const service = await startControl(dir, key, {
    status: id => ({ state: id === vmId ? 'off' : 'missing' }),
    start: async id => { started.push(id); return { state: 'starting' }; },
  });
  t.after(() => service.close());
  const path = join(dir, 'control.sock');
  const result = await request(path, key, 'status', undefined, true, vmId);
  assert.deepEqual(result.status, { state: 'off' });
  const nonce = randomBytes(32).toString('hex');
  const masterProof = createHmac('sha256', key).update(`${nonce}:start`).digest('hex');
  const forged = await new Promise((resolve, reject) => {
    const socket = connect(path);
    let data = '';
    socket.on('error', reject);
    socket.on('connect', () => socket.write(`${JSON.stringify({ nonce, method: 'start',
      vmId, mac: masterProof })}\n`));
    socket.on('data', chunk => { data += chunk; });
    socket.on('end', () => resolve(JSON.parse(data.trim())));
  });
  assert.equal(forged.ok, false);
  assert.deepEqual(started, []);
  assert.equal((await request(path, key, 'start', undefined, true, vmId)).ok, true);
  assert.deepEqual(started, [vmId]);
});

test('signed inventory returns only registered worker routes', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-inventory-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const machines = [{ vmId: '019a1111-1111-7111-8111-111111111111',
    slot: 0, key: '2a'.repeat(32), cwd: '/Users/Shared/MiniClaw/Workspace' }];
  const service = await startControl(dir, key, {
    status: () => ({ state: 'off' }), start: async () => ({ state: 'starting' }),
    list: () => machines,
  });
  t.after(() => service.close());
  assert.deepEqual(await request(join(dir, 'control.sock'), key, 'list'),
    { ok: true, machines });
});

test('failed startup returns the bounded cause and leaves status failed', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-failure-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const service = await startControl(dir, key, {
    status: () => ({ state: 'failed', reason: 'VM resource limit exceeded.' }),
    start: async () => { throw new Error('private host path /Users/example'); },
  });
  t.after(() => service.close());
  const result = await request(join(dir, 'control.sock'), key, 'start');
  assert.equal(result.ok, false);
  assert.equal(result.error.message, 'VM resource limit exceeded.');
  assert.doesNotMatch(JSON.stringify(result), /Users\/example/);
  assert.deepEqual((await request(join(dir, 'control.sock'), key, 'status')).status,
    { state: 'failed', reason: 'VM resource limit exceeded.' });
});

test('signed desktop actions affect only the selected running VM', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-desktop-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const vmId = '019a1111-1111-7111-8111-111111111111';
  const actions = [];
  const service = await startControl(dir, key, {
    status: id => ({ state: id === vmId ? 'running' : 'off' }),
    start: async () => ({ state: 'starting' }),
    show: async id => { actions.push(['show', id]); return { state: 'running' }; },
    stop: async id => { actions.push(['stop', id]); return { state: 'stopping' }; },
  });
  t.after(() => service.close());
  const path = join(dir, 'control.sock');
  assert.equal((await request(path, key, 'show', undefined, false, vmId)).ok, false);
  assert.equal((await request(path, key, 'stop', undefined, true, 'master')).ok, false);
  assert.deepEqual(actions, []);
  assert.deepEqual((await request(path, key, 'show', undefined, true, vmId)).status,
    { state: 'running' });
  assert.deepEqual((await request(path, key, 'stop', undefined, true, vmId)).status,
    { state: 'stopping' });
  assert.deepEqual(actions, [['show', vmId], ['stop', vmId]]);
});

// Sends one signed ledger request; `limits` is part of what is signed.
function ledgerRequest(path, key, method, limits, tamper) {
  const nonce = randomBytes(32).toString('hex');
  const mac = createHmac('sha256', key).update(`${nonce}:${method}${limits ? `:${limits}` : ''}`).digest('hex');
  return new Promise((resolve, reject) => {
    const socket = connect(path);
    let data = '';
    socket.setTimeout(2000, () => socket.destroy(new Error('Control timed out')));
    socket.on('error', reject);
    socket.on('connect', () => socket.write(`${JSON.stringify({ nonce, method, mac,
      limits: tamper ?? limits })}\n`));
    socket.on('data', value => { data += value; });
    socket.on('end', () => { try { resolve(JSON.parse(data.trim())); } catch (error) { reject(error); } });
  });
}

test('signed ledger requests read, repair and set the shared VM budget', async t => {
  const dir = await mkdtemp('/private/tmp/mc-ledger-control-');
  t.after(() => rm(dir, { recursive: true, force: true }));
  const key = randomBytes(32), calls = [];
  const view = { vms: [], used: { cpuCount: 0 }, limits: { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 } };
  const control = await startControl(dir, key, {
    status: () => ({ state: 'off' }), start: () => ({ state: 'starting' }),
    ledger: () => view,
    limits: values => { calls.push(['limits', values]); return { ...view, limits: values }; },
    reconcile: () => { calls.push(['reconcile']); return { ...view, released: ['/vms/lost'] }; },
    forget: () => { calls.push(['forget']); return { ...view, forgotten: [] }; },
  });
  t.after(() => control.close());
  const path = join(dir, 'control.sock');
  assert.deepEqual(await ledgerRequest(path, key, 'ledger'), { ok: true, ledger: view });
  assert.deepEqual((await ledgerRequest(path, key, 'reconcile')).ledger.released, ['/vms/lost']);
  assert.deepEqual((await ledgerRequest(path, key, 'forget')).ledger.forgotten, []);
  const set = await ledgerRequest(path, key, 'limits', '40,60,30');
  assert.deepEqual(set.ledger.limits, { cpuPercent: 40, memoryPercent: 60, diskPercent: 30 });
  // The percentages are signed: changing them after signing is refused, as is a bad value.
  assert.equal((await ledgerRequest(path, key, 'limits', '40,60,30', '90,90,90')).ok, false);
  for (const bad of ['0,50,25', '50,101,25', '50,50', '50,50,25,1', 'a,b,c', '50.5,50,25']) {
    assert.equal((await ledgerRequest(path, key, 'limits', bad)).ok, false, bad);
  }
  assert.equal((await ledgerRequest(path, key, 'ledger', '50,50,25')).ok, false, 'limits only with limits');
  assert.deepEqual(calls, [['reconcile'], ['forget'],
    ['limits', { cpuPercent: 40, memoryPercent: 60, diskPercent: 30 }]]);
});

test('signed delete reaches only a stopped project VM, never Master', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-control-delete-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await chmod(dir, 0o700);
  const key = randomBytes(32);
  const stopped = '019a1111-1111-7111-8111-111111111111';
  const running = '019a2222-2222-7222-8222-222222222222';
  const deleted = [];
  const service = await startControl(dir, key, {
    status: id => ({ state: id === running ? 'running' : id === 'master' ? 'off' : 'stopped' }),
    start: async () => ({ state: 'starting' }),
    delete: async id => { deleted.push(id); return { state: 'missing' }; },
  });
  t.after(() => service.close());
  const path = join(dir, 'control.sock');
  assert.equal((await request(path, key, 'delete', undefined, true, 'master')).ok, false);
  assert.equal((await request(path, key, 'delete')).ok, false, 'a VM must be named');
  assert.equal((await request(path, key, 'delete', undefined, false, stopped)).ok, false);
  assert.equal((await request(path, key, 'delete', undefined, true, running)).ok, false);
  assert.deepEqual(deleted, []);
  assert.deepEqual((await request(path, key, 'delete', undefined, true, stopped)).status,
    { state: 'missing' });
  assert.deepEqual(deleted, [stopped]);
});
