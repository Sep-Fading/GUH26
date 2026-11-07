// Reads and repairs the shared reservation ledger.
import { ResourceStore } from './resource-store.mjs';
// Checks the ledger exists before opening it.
import { copyFileSync, existsSync, lstatSync } from 'node:fs';
// Names the backup beside the ledger.
import { join } from 'node:path';
// Asks the OS which files and processes are live.
import { execFile } from 'node:child_process';

// Runs one inspection command and returns its output lines; a non-zero exit means none.
function lines(binary, args) {
  return new Promise(resolve => execFile(binary, args, { timeout: 10_000, maxBuffer: 1 << 20 },
    (_error, stdout) => resolve(String(stdout).split('\n').map(line => line.trim()).filter(Boolean))));
}

// Returns the process ids that hold any of these files open.
const openedBy = files => files.length ? lines('/usr/sbin/lsof', ['-t', '--', ...files]) : [];

// Returns running commands that name this guest directory.
const running = async id => (await lines('/bin/ps', ['-axo', 'pid=,command=']))
  .filter(line => line.includes(id) && !line.includes('ledger'));

// Returns the ledger with budgets and current use, or null when none exists yet.
export function readLedger(directory, measureHost) {
  try { lstatSync(directory); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  const store = new ResourceStore(directory, { create: false });
  try { return store.snapshot(measureHost(directory)); }
  finally { store.close(); }
}

// Releases compute held by guests whose supervisor died. A guest qualifies only when it
// is `unknown`, no process names it and none of its files are open; the user confirms
// each one. The ledger is copied first. Disks stay reserved and nothing is deleted.
export async function reconcileLedger(directory, { confirm, now = Date.now,
  openFiles = openedBy, processes = running } = {}) {
  const store = new ResourceStore(directory, { create: false });
  try {
    const data = store.contents();
    const released = [], refused = [];
    for (const vm of data.vms.filter(row => row.state === 'unknown')) {
      const files = data.files.filter(file => file.path?.startsWith(`${vm.id}/`)).map(file => file.path);
      const holders = await openFiles(files);
      const owners = await processes(vm.id);
      if (holders.length || owners.length) {
        refused.push({ id: vm.id, reason: `in use by ${[...holders.map(pid => `pid ${pid}`), ...owners].join('; ')}` });
        continue;
      }
      if (await confirm({ id: vm.id, cpuCount: vm.cpuCount, memoryBytes: vm.memoryBytes, files })) {
        released.push(vm.id);
      }
    }
    let backup = null;
    if (released.length) {
      backup = join(directory, `reservations.before-${now()}.sqlite`);
      copyFileSync(join(directory, 'reservations.sqlite'), backup);
      store.reconcile(released);
    }
    return { released, refused, backup };
  } finally { store.close(); }
}

// Forgets stopped guests whose folder or disk was deleted, with their recorded files and
// every other recorded file that is gone.
// A guest that still exists, or is not stopped, is left alone; nothing on disk is touched.
export async function forgetLedger(directory, { confirm, now = Date.now, exists = existsSync } = {}) {
  const store = new ResourceStore(directory, { create: false });
  try {
    const data = store.contents();
    const gone = data.files.filter(file => file.path && !exists(file.path));
    const forgotten = [], paths = [];
    // A guest is gone when its folder is, or when its disk is: without a disk it cannot boot.
    const deleted = vm => !exists(vm.id) || !exists(join(vm.id, 'Disk.img'));
    for (const vm of data.vms.filter(row => row.state === 'stopped' && deleted(row))) {
      // Its remaining small files lose their reservation with it.
      const files = data.files.filter(file => file.path?.startsWith(`${vm.id}/`));
      if (await confirm({ id: vm.id, cpuCount: vm.cpuCount, memoryBytes: vm.memoryBytes,
        files: files.map(file => file.path), bytes: files.reduce((sum, file) => sum + file.bytes, 0) })) {
        forgotten.push(vm.id);
        paths.push(...files.map(file => file.path));
      }
    }
    // Files outside any guest folder, such as deleted setup media, go with the guests.
    if (forgotten.length) {
      paths.push(...gone.filter(file => !data.vms.some(vm => file.path.startsWith(`${vm.id}/`)))
        .map(file => file.path));
    }
    let backup = null;
    if (forgotten.length) {
      backup = join(directory, `reservations.before-${now()}.sqlite`);
      copyFileSync(join(directory, 'reservations.sqlite'), backup);
      store.forget(forgotten, paths);
    }
    return { forgotten, backup };
  } finally { store.close(); }
}
