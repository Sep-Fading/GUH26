// Checks durable admission and failure boundaries.
import assert from 'node:assert/strict';
// Runs isolated host resource contract cases.
import { test } from 'node:test';
// Creates disposable ledgers and sparse disks.
import { chmod, lstat, mkdir, mkdtemp, open, readFile, rm, symlink, writeFile } from 'node:fs/promises';
// Locates disposable fixtures outside retained guests.
import { tmpdir } from 'node:os';
// Resolves shared ledger and fixture paths.
import { join } from 'node:path';
// Exercises the trusted host reservation owner.
import { ResourceStore } from '../scripts/vm/resource-store.mjs';
// Exercises admission before the actual process owner.
import { startPair } from '../scripts/vm/session.mjs';
// Exercises the production host inventory collector.
import { configureResourceLimits, readResourceLimits, reservePair } from '../scripts/vm/resource-admission.mjs';
// Starts independent disposable owners for contention.
import { spawn } from 'node:child_process';

const GiB = 1024 ** 3;
const host = { cpuCount: 10, memoryBytes: 24 * GiB,
  diskBytes: 1024 * GiB, freeBytes: 512 * GiB, volumeId: 'disk1' };

// Returns synthetic host inventory with independent guest identities.
function vm(id, cpuCount = 2) {
  return { id, cpuCount, memoryBytes: 4 * GiB };
}

// Returns one retained storage object with its maximum growth.
function disk(id, bytes = 64 * GiB) {
  return { id, bytes, allocatedBytes: GiB, volumeId: 'disk1' };
}

// Opens a new private ledger without touching the user's registry.
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-resource-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'Resources');
  const store = new ResourceStore(path);
  t.after(() => store.close());
  return { root, path, store };
}

test('reservations survive reopening and block another start', async t => {
  const { path, store } = await fixture(t);
  store.reserve([vm('one', 4)], [disk('one')], host);
  store.close();
  const reopened = new ResourceStore(path);
  try {
    assert.equal(reopened.snapshot(host).used.cpuCount, 4);
    assert.throws(() => reopened.reserve([vm('two')], [disk('two')], host),
      error => error.code === 'resource_limit' && error.resources.includes('cpu'));
    assert.equal(reopened.snapshot(host).vms.length, 1);
  } finally { reopened.close(); }
});

test('defaults persist and fractional cores round down', async t => {
  const { store } = await fixture(t);
  const snapshot = store.snapshot({ ...host, cpuCount: 7 });
  assert.deepEqual(snapshot.limits, { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 });
  assert.deepEqual(snapshot.capacity, { cpuCount: 3, memoryBytes: 12 * GiB,
    diskBytes: 256 * GiB });
});

test('first initialization honors approved limits and later opens preserve them', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-initial-limits-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'Resources');
  const lower = { cpuPercent: 20, memoryPercent: 40, diskPercent: 10 };
  const first = new ResourceStore(path, { initialLimits: lower });
  try { assert.deepEqual(first.snapshot(host).limits, lower); }
  finally { first.close(); }
  const second = new ResourceStore(path, { initialLimits: { cpuPercent: 90, memoryPercent: 90, diskPercent: 90 } });
  try { assert.deepEqual(second.snapshot(host).limits, lower); }
  finally { second.close(); }
});

test('both guests reserve together or neither starts', async t => {
  const { store } = await fixture(t);
  assert.throws(() => store.reserve([vm('master', 4), vm('controller')],
    [disk('master'), disk('controller')], host), /cpu/);
  assert.equal(store.snapshot(host).vms.length, 0);
  assert.equal(store.snapshot(host).used.diskBytes, 128 * GiB,
    'Already retained disks must count even when compute admission is refused');
});

test('stopped Master keeps disk but does not reserve compute', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-role-reserve-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const GiB = 1024 ** 3;
  async function guestFiles(role, allocation, names) {
    const guest = join(root, role);
    await mkdir(guest, { mode: 0o700 });
    await writeFile(join(guest, 'Resources.json'), JSON.stringify(allocation));
    const disk = await open(join(guest, 'Disk.img'), 'wx', 0o600);
    try { await disk.truncate(allocation.disk * GiB); } finally { await disk.close(); }
    for (const name of names) await writeFile(join(guest, name), 'fixture');
    return guest;
  }
  const master = await guestFiles('master', { cpu: 2, memory: 4, disk: 64 },
    ['AuxiliaryStorage', 'HardwareModel', 'MachineIdentifier', 'LabOwner']);
  const controller = await guestFiles('controller', { cpu: 1, memory: 1, disk: 16 },
    ['EFI', 'Seed.iso', 'MachineIdentifier', 'LinuxOwner']);
  const media = join(root, 'Master.dmg');
  await writeFile(media, 'fixture');
  const pair = { master: { guest: master, media }, controller: { guest: controller } };
  const directory = join(root, 'Resources');
  const volumeId = String((await lstat(root)).dev);
  const options = { directory, roles: ['controller'],
    measureHost: () => ({ ...host, volumeId }) };
  const controllerLease = reservePair(pair, options);
  const first = new ResourceStore(directory, { create: false });
  assert.equal(first.snapshot({ ...host, volumeId }).used.cpuCount, 1);
  assert.ok(first.snapshot({ ...host, volumeId }).used.diskBytes >= 80 * GiB);
  assert.ok(first.snapshot({ ...host, volumeId }).used.diskBytes < 81 * GiB);
  first.close();
  const masterLease = reservePair(pair, { ...options, roles: ['master'] });
  const second = new ResourceStore(directory, { create: false });
  assert.equal(second.snapshot({ ...host, volumeId }).used.cpuCount, 3);
  second.close();
  masterLease.settle(['master']);
  const third = new ResourceStore(directory, { create: false });
  assert.equal(third.snapshot({ ...host, volumeId }).used.cpuCount, 1);
  third.close();
  controllerLease.settle(['controller']);
});

test('confirmed stop releases compute but retains every disk', async t => {
  const { store } = await fixture(t);
  const lease = store.reserve([vm('master', 4), vm('controller', 1)],
    [disk('master'), disk('controller')], host);
  store.settle(lease, ['master']);
  const snapshot = store.snapshot(host);
  assert.equal(snapshot.used.cpuCount, 1);
  assert.equal(snapshot.used.diskBytes, 128 * GiB);
  assert.equal(snapshot.vms.find(row => row.id === 'controller').state, 'unknown');
  assert.throws(() => store.reserve([vm('controller', 1)], [], host), /unresolved/i);
});

test('lower limits preserve reservations and allow stopping', async t => {
  const { store } = await fixture(t);
  const lease = store.reserve([vm('one', 4)], [disk('one')], host);
  store.configure({ cpuPercent: 20, memoryPercent: 20, diskPercent: 1 });
  assert.deepEqual(store.snapshot(host).exceeded, ['cpu', 'disk']);
  assert.throws(() => store.reserve([vm('two', 1)], [], host), /resource/i);
  store.settle(lease, ['one']);
  assert.equal(store.snapshot(host).used.cpuCount, 0);
  assert.equal(store.snapshot(host).used.diskBytes, 64 * GiB);
});

test('disk identities count once and stopped storage stays global', async t => {
  const { store } = await fixture(t);
  const lease = store.reserve([vm('one')], [disk('shared')], host);
  store.settle(lease, ['one']);
  store.reserve([vm('two')], [disk('shared')], host);
  assert.equal(store.snapshot(host).used.diskBytes, 64 * GiB);
});

test('low free space and unknown volumes refuse compute', async t => {
  const { store } = await fixture(t);
  assert.throws(() => store.reserve([vm('one')], [disk('one')],
    { ...host, freeBytes: GiB }), error => error.resources.includes('free disk'));
  assert.throws(() => store.reserve([vm('one')],
    [{ ...disk('one'), volumeId: 'other' }], host), /volume/i);
  assert.equal(store.snapshot(host).vms.length, 0);
});

test('stale completion cannot stop a newer owner', async t => {
  const { store } = await fixture(t);
  const old = store.reserve([vm('one')], [disk('one')], host);
  store.settle(old, ['one']);
  store.reserve([vm('one')], [disk('one')], host);
  store.settle(old, ['one']);
  assert.equal(store.snapshot(host).used.cpuCount, 2);
});

test('corrupt ledger and missing initialized database fail closed', async t => {
  const { path, store } = await fixture(t);
  store.reserve([vm('one')], [disk('one')], host);
  store.close();
  const database = join(path, 'reservations.sqlite');
  await writeFile(database, 'broken');
  assert.throws(() => new ResourceStore(path), /database|ledger/i);
  await rm(database);
  assert.throws(() => new ResourceStore(path), /missing|ledger/i);
});

test('unsafe values do not change reservations', async t => {
  const { store } = await fixture(t);
  for (const invalid of [NaN, 0, -1, Infinity, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => store.reserve([vm('one', invalid)], [], host), /invalid/i);
  }
  assert.throws(() => store.configure({ cpuPercent: 101, memoryPercent: 50, diskPercent: 25 }), /invalid/i);
  assert.equal(store.snapshot(host).vms.length, 0);
});

// Creates inert supervisors that report optional shutdown confirmation.
async function pairFixture(t, terminal = 'stopped') {
  const root = await mkdtemp(join(tmpdir(), 'mc-resource-pair-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const guest = join(root, 'Guest');
  await mkdir(guest);
  const binary = join(root, 'supervisor');
  await writeFile(binary, `#!/usr/bin/env node
process.on('SIGTERM', () => {
  if (${JSON.stringify(terminal)}) console.log(JSON.stringify({ status: ${JSON.stringify(terminal)} }));
  process.exit(0);
});
console.log(JSON.stringify({ status: 'vm_running' }));
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  await chmod(root, 0o700);
  return { root, port: 19878, mac: { worker: join(root, 'worker.json') },
    master: { guest, binary, media: 'unused' }, controller: { guest, binary } };
}

test('admission refusal starts no VM', async t => {
  const pair = await pairFixture(t);
  const started = [];
  const session = await startPair(pair, { admit() { throw new Error('Resource admission refused'); },
    onStatus: (role, status) => started.push([role, status]) });
  try {
    await assert.rejects(session.startMaster(), /Resource admission refused/);
    assert.deepEqual(started, [], 'a refused VM never boots');
  } finally { await session.close(); }
});

for (const terminal of ['stopped', 'forced_stopped', 'force_stop_failed', null]) {
  test(`resource release requires native stop confirmation: ${terminal}`, async t => {
    const pair = await pairFixture(t, terminal);
    let settled, resolveReady;
    const running = new Promise(resolve => { resolveReady = resolve; });
    const session = await startPair(pair, {
      admit(_pair, { roles }) { assert.deepEqual(roles, ['master']);
        return { settle(confirmed) { settled = confirmed; } }; },
      onStatus(role, status) { if (status === 'vm_running') resolveReady(); },
    });
    try { await session.startMaster(); await running; }
    finally { await session.close(); }
    assert.deepEqual(settled, ['stopped', 'forced_stopped'].includes(terminal) ? ['master'] : []);
  });
}

// Starts one isolated child and captures readiness without logging private paths.
function owner(t, path, id, mode = 'reserve') {
  const module = new URL('../scripts/vm/resource-store.mjs', import.meta.url).href;
  const child = spawn(process.execPath, ['--input-type=module', '-e', `
    import { ResourceStore } from ${JSON.stringify(module)};
    import { DatabaseSync } from 'node:sqlite';
    const store = new ResourceStore(process.argv[1]);
    console.log('ready');
    process.stdin.once('data', () => {
      if (process.argv[3] === 'transaction') {
        const db = new DatabaseSync(process.argv[1] + '/reservations.sqlite');
        db.exec("BEGIN IMMEDIATE; UPDATE ledger SET value='broken' WHERE id=1");
        console.log('uncommitted');
        setInterval(() => {}, 1000);
        return;
      }
      try {
        store.reserve([{ id: process.argv[2], cpuCount: 4, memoryBytes: ${4 * GiB} }], [], ${JSON.stringify(host)});
        console.log('reserved');
        if (process.argv[3] === 'crash') setInterval(() => {}, 1000);
        else process.exit(0);
      } catch (error) {
        console.log(error.code ?? 'unexpected_error');
        process.exit(2);
      }
    });
  `, path, id, mode], { stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => { if (child.exitCode === null) child.kill('SIGKILL'); });
  let output = '', notify = () => {};
  child.stdout.on('data', chunk => { output += chunk; notify(); });
  const done = new Promise((resolve, reject) => {
    child.once('error', reject); child.once('close', code => resolve(code));
  });
  // Waits until this controlled child reaches the requested transaction stage.
  async function wait(text) {
    if (output.includes(text)) return;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Resource child did not reach its barrier')), 5000);
      notify = () => { if (output.includes(text)) { clearTimeout(timer); resolve(); } };
    });
  }
  return { child, done, wait, output: () => output };
}

test('independent supervisors cannot spend the same remaining capacity', async t => {
  const { store, path } = await fixture(t);
  const first = owner(t, path, 'first'), second = owner(t, path, 'second');
  await Promise.all([first.wait('ready'), second.wait('ready')]);
  first.child.stdin.end('start'); second.child.stdin.end('start');
  assert.deepEqual((await Promise.all([first.done, second.done])).sort(), [0, 2]);
  assert.equal(store.snapshot(host).used.cpuCount, 4);
  assert.match(first.output() + second.output(), /resource_limit/);
});

test('killed owner retains committed compute reservations', async t => {
  const { store, path } = await fixture(t);
  const child = owner(t, path, 'crashed', 'crash');
  await child.wait('ready'); child.child.stdin.end('start');
  await child.wait('reserved'); child.child.kill('SIGKILL'); await child.done;
  assert.throws(() => store.reserve([vm('another')], [], host), /cpu/);
  assert.equal(store.snapshot(host).used.cpuCount, 4);
});

test('crashed transaction rolls back without losing earlier reservations', async t => {
  const { path, store } = await fixture(t);
  store.reserve([vm('retained')], [], host);
  const child = owner(t, path, 'crashed', 'transaction');
  await child.wait('ready'); child.child.stdin.end('start');
  await child.wait('uncommitted'); child.child.kill('SIGKILL'); await child.done;
  assert.equal(store.snapshot(host).used.cpuCount, 2);
  store.reserve([vm('next')], [], host);
  assert.equal(store.snapshot(host).used.cpuCount, 4);
});

test('shared paths and linked databases are refused', async t => {
  const { root, store, path } = await fixture(t);
  store.close();
  await chmod(path, 0o755);
  assert.throws(() => new ResourceStore(path), /private/);
  await chmod(path, 0o700);
  const database = join(path, 'reservations.sqlite');
  const bytes = await readFile(database);
  await writeFile(join(root, 'outside.sqlite'), bytes, { mode: 0o600 });
  await rm(database);
  await symlink(join(root, 'outside.sqlite'), database);
  assert.throws(() => new ResourceStore(path), /private/);
  assert.deepEqual(await readFile(join(root, 'outside.sqlite')), bytes);
});

// Writes minimum native resource files and sparse disks without booting guests.
async function nativePair(t) {
  const pair = await pairFixture(t);
  for (const role of ['master', 'controller']) {
    const guest = join(pair.root, role);
    await mkdir(guest, { mode: 0o700 });
    pair[role].guest = guest;
    const resources = role === 'master' ? { cpu: 2, memory: 4, disk: 64 }
      : { cpu: 1, memory: 1, disk: 4 };
    await writeFile(join(guest, 'Resources.json'), JSON.stringify(resources), { mode: 0o600 });
    const file = await open(join(guest, 'Disk.img'), 'wx', 0o600);
    try { await file.truncate(resources.disk * GiB); }
    finally { await file.close(); }
    const names = role === 'master' ? ['AuxiliaryStorage', 'HardwareModel', 'MachineIdentifier', 'LabOwner']
      : ['EFI', 'Seed.iso', 'MachineIdentifier', 'LinuxOwner'];
    for (const name of names) await writeFile(join(guest, name), 'fixture', { mode: 0o600 });
  }
  pair.master.media = join(pair.root, 'Media.iso');
  await writeFile(pair.master.media, 'fixture', { mode: 0o600 });
  return pair;
}

test('native disk mismatch refuses startup without creating a registry', async t => {
  const pair = await nativePair(t);
  const config = join(pair.master.guest, 'Resources.json');
  await writeFile(config, JSON.stringify({ cpu: 2, memory: 4, disk: 128 }));
  assert.throws(() => reservePair(pair, { directory: join(pair.root, 'Resources') }), /disk differs/);
  await assert.rejects(readFile(join(pair.root, 'Resources/reservations.sqlite')), { code: 'ENOENT' });
});

test('native collector inventories storage and honors one authoritative global limit', async t => {
  const pair = await nativePair(t);
  const directory = join(pair.root, 'Resources');
  const volumeId = String((await lstat(pair.root)).dev);
  const measured = { ...host, volumeId };
  const approved = { cpuPercent: 70, memoryPercent: 60, diskPercent: 30 };
  assert.deepEqual(readResourceLimits(directory), { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 });
  await assert.rejects(lstat(directory), { code: 'ENOENT' });
  const reservation = reservePair(pair, { directory, initialLimits: approved, measureHost: () => measured });
  const store = new ResourceStore(directory);
  try {
    const reserved = store.snapshot(measured);
    assert.deepEqual(reserved.limits, approved);
    assert.equal(reserved.used.cpuCount, 3);
    assert.equal(reserved.used.memoryBytes, 5 * GiB);
    assert.ok(reserved.used.diskBytes > 68 * GiB);
    assert.equal(reserved.files.length, 13);
    reservation.settle(['master', 'controller']);
    assert.equal(store.snapshot(measured).used.cpuCount, 0);
    assert.equal(store.snapshot(measured).used.diskBytes, reserved.used.diskBytes);
    const lower = { ...approved, cpuPercent: 20 };
    configureResourceLimits(lower, directory);
    assert.deepEqual(readResourceLimits(directory), lower);
    assert.throws(() => reservePair(pair, { directory, initialLimits: approved,
      measureHost: () => measured }), error => error.code === 'resource_limit');
    assert.deepEqual(store.snapshot(measured).limits, lower);
  } finally { store.close(); }
});

test('a Mac-backend pair reserves its VM with no Linux controller to inventory', async t => {
  const pair = await nativePair(t);
  await rm(pair.controller.guest, { recursive: true });
  delete pair.controller;
  pair.mac = { worker: join(pair.root, 'worker.json') };
  const directory = join(pair.root, 'Resources');
  const measured = { ...host, volumeId: String((await lstat(pair.root)).dev) };
  const reservation = reservePair(pair, { directory, measureHost: () => measured, roles: ['master'] });
  const store = new ResourceStore(directory);
  try {
    const reserved = store.snapshot(measured);
    assert.equal(reserved.used.cpuCount, 2);
    assert.equal(reserved.used.memoryBytes, 4 * GiB);
    assert.equal(reserved.vms.length, 1);
    reservation.settle(['master']);
    assert.equal(store.snapshot(measured).used.cpuCount, 0);
  } finally { store.close(); }
  assert.throws(() => reservePair(pair, { directory, measureHost: () => measured }),
    /controller/i, 'asking for a controller this pair does not have is refused');
});
