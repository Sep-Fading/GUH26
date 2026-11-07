// Checks status framing and console privacy.
import assert from 'node:assert/strict';
// Checks that deleted VM folders are gone.
import { existsSync } from 'node:fs';
// Runs lifecycle reporting without booting guests.
import { test } from 'node:test';
// Signs calls to the private host-side supervisor.
import { createHmac, randomBytes } from 'node:crypto';
// Connects only to the disposable control socket.
import { connect } from 'node:net';
// Builds isolated executable and guest fixtures.
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
// Locates disposable directories outside retained guests.
import { tmpdir } from 'node:os';
// Resolves fake supervisor and guest paths.
import { join } from 'node:path';
// Exercises the real paired process owner.
import { startPair, reportShutdown } from '../scripts/vm/session.mjs';
// Waits for observed asynchronous status delivery.
import { eventually } from './support/harness.mjs';

function control(path, key, method, vmId, requestId) {
  const nonce = randomBytes(32).toString('hex');
  const mac = createHmac('sha256', key)
    .update(`${nonce}:${method}${vmId ? `:${vmId}` : ''}`
      + `${requestId ? `:${requestId}` : ''}`).digest('hex');
  return new Promise((resolve, reject) => {
    const socket = connect(path);
    let text = '';
    socket.on('error', reject);
    socket.on('connect', () => socket.write(`${JSON.stringify({ nonce, mac, method,
      vmId, requestId })}\n`));
    socket.on('data', chunk => { text += chunk; });
    socket.on('end', () => { try { resolve(JSON.parse(text)); } catch (error) { reject(error); } });
  });
}

test('paired status excludes console data and survives split frames', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-status-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const guest = join(root, 'Guest');
  await mkdir(guest);
  const binary = join(root, 'supervisor');
  await writeFile(binary, `#!/usr/bin/env node
process.on('SIGTERM', () => process.exit(2));
setInterval(() => {}, 1000);
process.stderr.write('PRIVATE_CONSOLE\\n{"status":"vm_failed"}\\n');
process.stdout.write('x'.repeat(4096) + '\\n');
process.stdout.write('{"status":"unknown","secret":"PRIVATE_FIELD"}\\n');
process.stdout.write('{"status":"shut');
setTimeout(() => process.stdout.write('down_waiting"}\\n'
  + '{"status":"force_stopping"}\\n{"status":"forced_stopped"}\\n'
  + '{"status":"force_stop_failed"}\\n'), 10);
`, { mode: 0o700 });
  const statuses = [];
  await chmod(root, 0o700);
  const session = await startPair({ root, port: 19878, mac: { worker: join(root, 'worker.json') },
    master: { binary, guest, media: 'unused' },
  }, { admit: () => ({ settle() {} }), onStatus: (role, status) => statuses.push({ role, status }) });
  try {
    await session.startMaster();
    await eventually(() => statuses.length, count => count === 4, 'native shutdown warnings');
    assert.equal(session.egressMode, 'public-web');
    assert.deepEqual(statuses, [
      { role: 'master', status: 'shutdown_waiting' },
      { role: 'master', status: 'force_stopping' },
      { role: 'master', status: 'forced_stopped' },
      { role: 'master', status: 'force_stop_failed' },
    ]);
  } finally { assert.equal(await session.close(), 2, 'Forced stop must retain its exit code'); }
});

test('project VM provisioning starts only its new guest on its fixed route', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-project-session-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const masterGuest = join(root, 'Master');
  const controllerGuest = join(root, 'Controller');
  await mkdir(masterGuest); await mkdir(controllerGuest);
  const native = join(root, 'native');
  const nativeLog = join(root, 'native-started');
  await writeFile(native, `#!/usr/bin/env node
import { appendFileSync } from 'node:fs';
appendFileSync(${JSON.stringify(nativeLog)}, JSON.stringify(process.argv.slice(2)) + '\\n');
if (process.argv[2] === 'run') process.stdout.write('{"status":"vm_running"}\\n');
process.on('SIGTERM', () => process.stdout.write('{"status":"stopped"}\\n', () => process.exit(0)));
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  const host = join(root, 'host-worker');
  await writeFile(host, '#!/usr/bin/env node\nsetInterval(() => {}, 1000);\n', { mode: 0o700 });
  const settings = join(root, 'host-worker.json');
  await writeFile(settings, '{}', { mode: 0o600 });
  let dedicatedGuest;
  const pair = { root, port: 19878, mac: { worker: settings },
    master: { binary: native, guest: masterGuest, media: 'unused' },
    provisioning: { resources: {} },
  };
  const session = await startPair(pair, { admit: () => ({ settle() {} }),
    prepareDedicated: async (_pair, machine, { report }) => {
      dedicatedGuest = join(root, 'dedicated-vms', machine.vmId,
        `Attempt-${machine.attempt}`, 'Guest');
      await mkdir(dedicatedGuest, { recursive: true, mode: 0o700 });
      await report('installing', 100);
      return { guest: dedicatedGuest, media: 'fresh-media',
        lease: { settle() {} }, machine };
    } });
  try {
    const key = Buffer.from(await readFile(session.controlKeyPath, 'utf8'), 'hex');
    const requestId = '019a2222-2222-7222-8222-222222222222';
    const created = await control(session.controlPath, key, 'provision', undefined, requestId);
    assert.equal(created.ok, true, JSON.stringify(created));
    const vmId = created.machine.vmId;
    await eventually(async () => {
      const text = await readFile(nativeLog, 'utf8').catch(() => '');
      return text.split('\n').filter(Boolean).map(JSON.parse)
        .some(args => args[0] === 'run' && args[1] === dedicatedGuest);
    }, Boolean, 'dedicated guest start');
    const calls = (await readFile(nativeLog, 'utf8')).split('\n').filter(Boolean).map(JSON.parse);
    assert.equal(calls.some(args => args[0] === 'run' && args[1] === masterGuest), false);
    const inventory = await control(session.controlPath, key, 'list');
    assert.equal(inventory.machines[0].vmId, vmId);
    assert.equal(JSON.stringify(created).includes(inventory.machines[0].key), false);
    const status = await control(session.controlPath, key, 'status', vmId);
    assert.equal(status.status.stage, 'awaiting_setup');
    await session.close();
    const resumed = await startPair(pair, { admit: () => ({ settle() {} }),
      prepareDedicated: async () => { throw new Error('Fresh install must not repeat'); } });
    try {
      const resumeKey = Buffer.from(await readFile(resumed.controlKeyPath, 'utf8'), 'hex');
      assert.equal((await control(resumed.controlPath, resumeKey, 'status', vmId)).status.state, 'off');
      const retry = await control(resumed.controlPath, resumeKey, 'retry', vmId);
      assert.equal(retry.ok, true, JSON.stringify(retry));
      await eventually(async () => (await readFile(nativeLog, 'utf8')).split('\n')
        .filter(Boolean).map(JSON.parse)
        .filter(args => args[0] === 'run' && args[1] === dedicatedGuest).length,
      count => count === 2, 'same dedicated guest resume');
      // A running VM keeps its disk. Once shut down, delete removes its folder and record.
      await eventually(async () => (await control(resumed.controlPath, resumeKey, 'status', vmId)).status.state,
        state => state === 'running', 'project VM running');
      assert.equal((await control(resumed.controlPath, resumeKey, 'delete', vmId)).ok, false);
      assert.equal(existsSync(dedicatedGuest), true, 'a running VM keeps its disk');
      assert.equal((await control(resumed.controlPath, resumeKey, 'stop', vmId)).ok, true);
      await eventually(async () => (await control(resumed.controlPath, resumeKey, 'status', vmId)).status.state,
        state => state === 'off', 'project VM off');
      const gone = await control(resumed.controlPath, resumeKey, 'delete', vmId);
      assert.deepEqual(gone.status, { state: 'missing', reason: 'Assigned VM does not exist.' }, JSON.stringify(gone));
      assert.equal(existsSync(join(root, 'dedicated-vms', vmId)), false, 'the VM folder is deleted');
      assert.deepEqual((await control(resumed.controlPath, resumeKey, 'list')).machines, []);
      assert.equal(existsSync(masterGuest), true, 'Master is untouched');
    } finally { await resumed.close(); }
  } finally { await session.close(); }
});

// Saved network typos cannot start guests under a broader fallback policy.
test('invalid egress prevents VM startup', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-policy-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await assert.rejects(startPair({ root, egress: 'invalid', mac: { worker: join(root, 'worker.json') } }),
    /egress mode/i);
  await assert.rejects(startPair({ root, controller: {} }), /Mac-backend pair/);
});

test('network disconnection has a visible diagnosis', () => {
  const output = [];
  const previous = console.error;
  console.error = value => output.push(value);
  try { reportShutdown('master', 'network_failed'); }
  finally { console.error = previous; }
  assert.match(output.join('\n'), /Master VM network disconnected/);
});

// A disconnected NIC is reported as the Master's failure, not as a healthy VM.
test('network failure shows in the Master status and preserves cleanup', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-nic-failure-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const guest = join(root, 'Guest');
  await mkdir(guest);
  const binary = join(root, 'supervisor');
  await writeFile(binary, `#!/usr/bin/env node
process.on('SIGTERM', () => process.exit(0));
if (process.argv[2] === 'run') {
  setTimeout(() => { process.stdout.write('{"status":"network_failed"}\\n'); setTimeout(() => process.exit(1), 20); }, 20);
} else setInterval(() => {}, 1000);
`, { mode: 0o700 });
  const statuses = [];
  const session = await startPair({ root, port: 19878, mac: { worker: join(root, 'worker.json') },
    master: { binary, guest, media: 'unused' },
  }, { admit: () => ({ settle() {} }), onStatus: (role, status) => statuses.push({ role, status }) });
  try {
    await session.startMaster();
    const key = Buffer.from(await readFile(session.controlKeyPath, 'utf8'), 'hex');
    const failed = await eventually(() => control(session.controlPath, key, 'status'),
      reply => reply.status?.state === 'failed', 'Master failure status');
    assert.equal(failed.status.reason, 'Master VM network disconnected.');
    assert.deepEqual(statuses, [{ role: 'master', status: 'network_failed' }]);
  } finally { await session.close(); }
});

test('a session leaves Master off and gives the VM only its own routes', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-host-route-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const masterGuest = join(root, 'Master');
  await mkdir(masterGuest);
  const native = join(root, 'native');
  const nativeLog = join(root, 'native-started');
  await writeFile(native, `#!/usr/bin/env node
import { appendFileSync } from 'node:fs';
appendFileSync(${JSON.stringify(nativeLog)}, JSON.stringify(process.argv.slice(2)) + '\\n');
process.on('SIGTERM', () => process.exit(0));
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  await writeFile(nativeLog, '');
  const session = await startPair({ root, port: 19878, mac: { worker: join(root, 'worker.json') },
    master: { binary: native, guest: masterGuest, media: 'unused' },
  }, { admit: () => ({ settle() {} }) });
  try {
    const routes = JSON.parse(await readFile(join(root, 'ActiveSession/MasterRoutes.json'), 'utf8'));
    assert.deepEqual(routes.map(route => route.port), [10242, 10243],
      'the VM gets its worker and egress routes; the scoped route is gone');
    const bridge = await readFile(new URL('../scripts/vm/guest-bridge.swift', import.meta.url), 'utf8');
    assert.match(bridge, /routes\.count <= 67/, 'the native supervisor accepts a subset of routes');
    assert.equal((await readFile(nativeLog, 'utf8')).includes('"run"'), false,
      'Master must stay off until an approved action starts it');
    const controlPath = session.controlPath;
    const controlKey = Buffer.from(await readFile(session.controlKeyPath, 'utf8'), 'hex');
    const missingId = '019a1111-1111-7111-8111-111111111111';
    assert.deepEqual((await control(controlPath, controlKey, 'status', missingId)).status,
      { state: 'missing', reason: 'Assigned VM does not exist.' });
    const missingStart = await control(controlPath, controlKey, 'start', missingId);
    assert.equal(missingStart.ok, false);
    assert.equal(missingStart.error.message, 'Assigned VM does not exist.');
    assert.equal((await readFile(nativeLog, 'utf8')).includes('"run"'), false);
    assert.deepEqual((await control(controlPath, controlKey, 'status')).status,
      { state: 'off' });
    assert.equal((await control(controlPath, controlKey, 'start')).ok, true);
    await eventually(async () => (await readFile(nativeLog, 'utf8')).includes('"run"'),
      Boolean, 'approved Master startup');
  } finally { await session.close(); }
});
