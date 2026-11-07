// Checks listing and reconciling the shared VM resource ledger.
import assert from 'node:assert/strict';
// Runs each ledger case in isolation.
import { test } from 'node:test';
// Creates disposable ledgers and reads their backups.
import { mkdtemp, readdir, rm } from 'node:fs/promises';
// Locates temporary storage outside the real registry.
import { tmpdir } from 'node:os';
// Resolves fixture paths.
import { join } from 'node:path';
// Builds ledgers the way real sessions do.
import { ResourceStore } from '../scripts/vm/resource-store.mjs';
// Loads the ledger tool under test.
import { forgetLedger, readLedger, reconcileLedger } from '../scripts/vm/ledger.mjs';

const GiB = 1024 ** 3;
const host = { cpuCount: 10, memoryBytes: 24 * GiB, diskBytes: 1024 * GiB, freeBytes: 512 * GiB, volumeId: 'disk1' };
const disk = (id, path) => ({ id, path, bytes: 64 * GiB, allocatedBytes: GiB, volumeId: 'disk1' });

// Leaves one guest `unknown` (its owner died), one `stopped` and one still `starting`.
async function ledger(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-ledger-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = join(root, 'Resources');
  const store = new ResourceStore(directory);
  const lost = store.reserve([{ id: '/vms/lost', cpuCount: 2, memoryBytes: 8 * GiB }],
    [disk('1:1', '/vms/lost/Disk.img')], host);
  store.settle(lost, []);
  const clean = store.reserve([{ id: '/vms/clean', cpuCount: 1, memoryBytes: 2 * GiB }],
    [disk('1:2', '/vms/clean/Disk.img')], host);
  store.settle(clean, ['/vms/clean']);
  store.reserve([{ id: '/vms/live', cpuCount: 2, memoryBytes: 4 * GiB }], [disk('1:3', '/vms/live/Disk.img')], host);
  store.close();
  return { root, directory };
}
const idle = { openFiles: async () => [], processes: async () => [] };

test('the listing shows each guest, its state and the budget it counts against', async t => {
  const { directory } = await ledger(t);
  const view = readLedger(directory, () => host);
  assert.deepEqual(view.vms.map(vm => [vm.id, vm.state, vm.cpuCount, vm.memoryBytes / GiB]),
    [['/vms/lost', 'unknown', 2, 8], ['/vms/clean', 'stopped', 1, 2], ['/vms/live', 'starting', 2, 4]]);
  assert.equal(view.used.memoryBytes, 12 * GiB, 'unknown and starting guests both count');
  assert.equal(view.capacity.memoryBytes, 12 * GiB);
  assert.equal(readLedger(join(directory, 'missing'), () => host), null);
});

test('an unknown guest with no process and no open disk is released after a backup', async t => {
  const { directory } = await ledger(t);
  const asked = [];
  const result = await reconcileLedger(directory, { ...idle, now: () => 1789900000000,
    confirm: async evidence => { asked.push(evidence); return true; } });
  assert.deepEqual(result.released, ['/vms/lost']);
  assert.deepEqual(asked.map(item => item.id), ['/vms/lost'], 'only unknown guests are offered');
  assert.deepEqual(asked[0].files, ['/vms/lost/Disk.img']);
  const view = readLedger(directory, () => host);
  assert.deepEqual(view.vms.map(vm => vm.state), ['stopped', 'stopped', 'starting']);
  assert.equal(view.used.memoryBytes, 4 * GiB);
  assert.equal(view.files.length, 3, 'disks stay reserved');
  assert.ok((await readdir(directory)).includes('reservations.before-1789900000000.sqlite'));
});

test('a guest whose disk is open or whose supervisor runs is refused', async t => {
  const { directory } = await ledger(t);
  for (const busy of [{ ...idle, openFiles: async () => ['4242'] },
    { ...idle, processes: async () => ['777 /x/master-vm run /vms/lost --background'] }]) {
    const result = await reconcileLedger(directory, { ...busy, confirm: () => assert.fail('asked about a busy guest') });
    assert.deepEqual(result.released, []);
    assert.match(result.refused[0].reason, /in use/);
  }
  assert.equal(readLedger(directory, () => host).vms[0].state, 'unknown');
  assert.equal((await readdir(directory)).some(name => name.includes('before-')), false, 'no change, no backup');
});

test('declining changes nothing, and only unknown rows can ever be released', async t => {
  const { directory } = await ledger(t);
  const result = await reconcileLedger(directory, { ...idle, confirm: async () => false });
  assert.deepEqual(result.released, []);
  assert.equal(readLedger(directory, () => host).vms[0].state, 'unknown');
  const store = new ResourceStore(directory, { create: false });
  try {
    assert.throws(() => store.reconcile(['/vms/live']), /unknown/);
    assert.throws(() => store.reconcile(['/vms/none']), /unknown/);
  } finally { store.close(); }
});

test('a stopped guest whose folder was deleted is forgotten with its files, after a backup', async t => {
  const { directory } = await ledger(t);
  // Only the clean guest's folder is gone; the lost and live guests still exist on disk.
  const exists = path => !path.startsWith('/vms/clean');
  const asked = [];
  const result = await forgetLedger(directory, { exists, now: () => 1789900000001,
    confirm: async evidence => { asked.push(evidence); return true; } });
  assert.deepEqual(result.forgotten, ['/vms/clean']);
  assert.deepEqual(asked.map(item => item.id), ['/vms/clean']);
  assert.deepEqual(asked[0].files, ['/vms/clean/Disk.img']);
  const view = readLedger(directory, () => host);
  assert.deepEqual(view.vms.map(vm => vm.id), ['/vms/lost', '/vms/live']);
  assert.deepEqual(view.files.map(file => file.path), ['/vms/lost/Disk.img', '/vms/live/Disk.img']);
  assert.ok((await readdir(directory)).includes('reservations.before-1789900000001.sqlite'));
});

test('a guest that still exists, or is not stopped, is never forgotten', async t => {
  const { directory } = await ledger(t);
  // Everything is "gone", but only a stopped guest may be forgotten: an unknown or starting
  // guest may still own its compute.
  const result = await forgetLedger(directory, { exists: () => false, confirm: async () => true });
  assert.deepEqual(result.forgotten, ['/vms/clean']);
  assert.deepEqual(readLedger(directory, () => host).vms.map(vm => vm.state), ['unknown', 'starting']);
  const untouched = await ledger(t);
  const kept = await forgetLedger(untouched.directory, { exists: () => true, confirm: async () => true });
  assert.deepEqual(kept.forgotten, []);
  assert.equal(kept.backup, null);
  assert.equal(readLedger(untouched.directory, () => host).files.length, 3);
  const declined = await forgetLedger(untouched.directory, { exists: () => false, confirm: async () => false });
  assert.deepEqual(declined.forgotten, []);
  assert.equal(readLedger(untouched.directory, () => host).vms.length, 3);
});

test('a stopped guest whose disk was deleted counts as gone even if its folder remains', async t => {
  const { directory } = await ledger(t);
  // The folder and its small files are still there; only the disk image was removed.
  const exists = path => path !== '/vms/clean/Disk.img';
  const result = await forgetLedger(directory, { exists, confirm: async () => true });
  assert.deepEqual(result.forgotten, ['/vms/clean']);
  assert.deepEqual(readLedger(directory, () => host).files.map(file => file.path),
    ['/vms/lost/Disk.img', '/vms/live/Disk.img']);
});
