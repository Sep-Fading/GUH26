// Measures host hardware without trusting guests.
import { cpus, homedir, totalmem } from 'node:os';
// Inspects retained storage without following links.
import { constants, closeSync, fstatSync, lstatSync, mkdirSync, openSync,
  readFileSync, realpathSync, statfsSync } from 'node:fs';
// Resolves the single user-wide host ledger.
import { dirname, join } from 'node:path';
// Commits allocations before native processes start.
import { ResourceStore, defaultLimits } from './resource-store.mjs';

const GiB = 1024 ** 3;

// Resolves the common registry independently of individual installations.
export function registry() { return join(homedir(), 'Library/Application Support/MiniClaw/Resources'); }

// Reads current global settings without creating a ledger during wizard review.
export function readResourceLimits(directory = registry()) {
  try { lstatSync(directory); }
  catch (error) { if (error.code === 'ENOENT') return { ...defaultLimits }; throw error; }
  const store = new ResourceStore(directory, { create: false });
  try { return store.limits(); }
  finally { store.close(); }
}

// Applies percentages only after the wizard explicitly approves the global change.
export function configureResourceLimits(limits, directory = registry()) {
  const parent = dirname(directory);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  owned(parent, true);
  const store = new ResourceStore(directory, { initialLimits: limits });
  try { store.configure(limits); }
  finally { store.close(); }
}

// Checks owned host paths before reading configuration or attached storage.
function owned(path, directory = false) {
  const info = lstatSync(path);
  if ((directory ? !info.isDirectory() : !info.isFile())
    || info.uid !== process.getuid() || (info.mode & 0o022)) {
    throw new Error('VM resources require owned files without writable shared access');
  }
  return info;
}

// Returns the maximum logical size and measured allocation of one storage object.
function storage(path) {
  const info = owned(path);
  return { id: `${info.dev}:${info.ino}`, path: realpathSync(path),
    bytes: info.size, allocatedBytes: info.blocks * 512, volumeId: String(info.dev) };
}

// Rechecks every retained object; missing or replaced files keep their full reservation.
function refresh(file) {
  try {
    const current = storage(file.path);
    if (current.id === file.id) return { ...current, bytes: Math.max(file.bytes, current.bytes) };
  } catch { /* Uncertain storage is retained until an owned deletion is proven. */ }
  return { ...file, allocatedBytes: 0 };
}

// Reads bounded native allocations and verifies the actual sparse disk maximum.
function guest(role, path) {
  owned(path, true);
  const root = realpathSync(path);
  const config = join(root, 'Resources.json');
  owned(config);
  const fd = openSync(config, constants.O_RDONLY | constants.O_NOFOLLOW);
  let resources;
  try {
    const info = fstatSync(fd);
    if (!info.isFile() || info.size > 4096) throw new Error('Invalid VM resource configuration');
    resources = JSON.parse(readFileSync(fd, 'utf8'));
  } finally { closeSync(fd); }
  const minimums = role === 'master' ? { cpu: 2, memory: 4, disk: 64 }
    : { cpu: 1, memory: 1, disk: 4 };
  for (const field of ['cpu', 'memory', 'disk']) {
    if (!Number.isSafeInteger(resources[field]) || resources[field] < minimums[field]) {
      throw new Error('Invalid VM resource configuration');
    }
  }
  const disk = storage(join(root, 'Disk.img'));
  if (disk.bytes !== resources.disk * GiB) throw new Error('VM disk differs from its resource allocation');
  const names = role === 'master' ? ['AuxiliaryStorage', 'HardwareModel', 'MachineIdentifier', 'LabOwner']
    : ['EFI', 'Seed.iso', 'MachineIdentifier', 'LinuxOwner'];
  return { vm: { id: root, cpuCount: resources.cpu, memoryBytes: resources.memory * GiB },
    files: [disk, storage(config), ...names.map(name => storage(join(root, name)))] };
}

// Reads capacity and free bytes from the host filesystem holding the global registry.
export function measure(path) {
  const info = statfsSync(path, { bigint: true });
  return { cpuCount: cpus().length, memoryBytes: totalmem(),
    diskBytes: Number(info.blocks * info.bsize), freeBytes: Number(info.bavail * info.bsize),
    volumeId: String(lstatSync(path).dev) };
}

// Reserves both guests using host files; only this host caller can settle ownership.
export function reservePair(pair, {
  directory = registry(), initialLimits = defaultLimits, measureHost = measure,
  roles = ['master', 'controller'],
} = {}) {
  if (!Array.isArray(roles) || !roles.length || new Set(roles).size !== roles.length
    || roles.some(role => !['master', 'controller'].includes(role))) {
    throw new Error('Invalid VM reservation roles');
  }
  const parent = dirname(directory);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  owned(parent, true);
  // A Mac-backend pair has no Linux controller, so only the guests it has are inventoried.
  const missing = roles.find(role => !pair[role]?.guest);
  if (missing) throw new Error(`This pair has no ${missing} guest to reserve`);
  const all = ['master', 'controller'].filter(role => pair[role]?.guest)
    .map(role => [role, guest(role, pair[role].guest)]);
  const entries = all.filter(([role]) => roles.includes(role));
  const store = new ResourceStore(directory, { initialLimits });
  try {
    const files = all.flatMap(([, entry]) => entry.files);
    files.push(storage(pair.master.media));
    const lease = store.reserve(entries.map(([, entry]) => entry.vm), files,
      () => measureHost(parent), refresh);
    return {
      // Releases compute only for guests the owned native supervisors confirmed stopped.
      settle(roles) {
        try { store.settle(lease, entries.filter(([role]) => roles.includes(role)).map(([, entry]) => entry.vm.id)); }
        finally { store.close(); }
      },
    };
  } catch (error) { store.close(); throw error; }
}
