// Serializes reservations across independent host supervisors.
import { DatabaseSync } from 'node:sqlite';
// Checks private ownership before opening databases.
import { constants, closeSync, lstatSync, mkdirSync, openSync } from 'node:fs';
// Creates opaque ownership tokens for starts.
import { randomUUID } from 'node:crypto';
// Resolves files within the private ledger.
import { join } from 'node:path';

export const defaultLimits = Object.freeze({ cpuPercent: 50, memoryPercent: 50, diskPercent: 25 });
const states = new Set(['starting', 'running', 'paused', 'stopping', 'stopped', 'unknown']);

// Rejects missing, fractional, negative and imprecise resource values.
function integer(value, minimum = 1) {
  if (!Number.isSafeInteger(value) || value < minimum) throw new Error('Invalid resource value');
  return value;
}

// Requires an opaque nonempty identity without rendering it in errors.
function identity(value) {
  if (typeof value !== 'string' || !value.length || value.length > 4096) {
    throw new Error('Invalid resource identity');
  }
}

// Checks configurable percentages without accepting partial settings.
function limits(value) {
  for (const field of Object.keys(defaultLimits)) {
    if (integer(value?.[field]) > 100) throw new Error('Invalid resource limit');
  }
}

// Adds reservations without allowing integer overflow to approve an operation.
function sum(left, right) {
  return integer(left + right, 0);
}

// Calculates exact percentages without floating-point overflow.
function portion(total, percent) {
  return Number(BigInt(total) * BigInt(percent) / 100n);
}

// Checks both trusted input and persisted records before calculation.
function validate(data) {
  if (data?.schemaVersion !== 1 || !Array.isArray(data.vms) || !Array.isArray(data.files)) {
    throw new Error('Invalid resource ledger');
  }
  limits(data.limits);
  if (data.volumeId !== null) identity(data.volumeId);
  for (const [records, isVM] of [[data.vms, true], [data.files, false]]) {
    const seen = new Set();
    for (const entry of records) {
      identity(entry.id);
      if (seen.has(entry.id)) throw new Error('Invalid duplicate resource identity');
      seen.add(entry.id);
      if (isVM) {
        integer(entry.cpuCount); integer(entry.memoryBytes); identity(entry.lease);
        if (!states.has(entry.state)) throw new Error('Invalid resource state');
      } else {
        integer(entry.bytes, 0); integer(entry.allocatedBytes, 0); identity(entry.volumeId);
      }
    }
  }
}

// Reports allocation ceilings and sparse growth separately from measured free space.
function evaluate(data, host) {
  validate(data);
  for (const field of ['cpuCount', 'memoryBytes', 'diskBytes']) integer(host?.[field]);
  integer(host?.freeBytes, 0); identity(host?.volumeId);
  if (data.volumeId !== null && data.volumeId !== host.volumeId) {
    throw new Error('Resource volume identity changed');
  }
  const capacity = {
    cpuCount: portion(host.cpuCount, data.limits.cpuPercent),
    memoryBytes: portion(host.memoryBytes, data.limits.memoryPercent),
    diskBytes: portion(host.diskBytes, data.limits.diskPercent),
  };
  const used = { cpuCount: 0, memoryBytes: 0, diskBytes: 0 };
  let growthBytes = 0;
  for (const vm of data.vms) {
    if (vm.state !== 'stopped') {
      used.cpuCount = sum(used.cpuCount, vm.cpuCount);
      used.memoryBytes = sum(used.memoryBytes, vm.memoryBytes);
    }
  }
  for (const file of data.files) {
    if (file.volumeId !== host.volumeId) throw new Error('Unsupported resource volume');
    used.diskBytes = sum(used.diskBytes, file.bytes);
    growthBytes = sum(growthBytes, Math.max(0, file.bytes - file.allocatedBytes));
  }
  const exceeded = [];
  for (const [name, field] of [['cpu', 'cpuCount'], ['memory', 'memoryBytes'], ['disk', 'diskBytes']]) {
    if (used[field] > capacity[field]) exceeded.push(name);
  }
  if (growthBytes > host.freeBytes) exceeded.push('free disk');
  return { ...data, capacity, used, exceeded, growthBytes, freeBytes: host.freeBytes };
}

// Refuses shared, linked or foreign ledger storage without changing permissions.
function privatePath(path, directory) {
  const info = lstatSync(path);
  if ((directory ? !info.isDirectory() : !info.isFile() || info.nlink !== 1)
    || info.uid !== process.getuid() || (info.mode & 0o077)) {
    throw new Error('Resource ledger must be private and owned by this user');
  }
}

export class ResourceStore {
  #db;

  // Opens one private host ledger; an incomplete or missing existing ledger fails closed.
  constructor(directory, { initialLimits = defaultLimits, create = true } = {}) {
    limits(initialLimits);
    let created = false;
    if (create) {
      try { mkdirSync(directory, { mode: 0o700 }); created = true; }
      catch (error) { if (error.code !== 'EEXIST') throw error; }
    }
    privatePath(directory, true);
    const path = join(directory, 'reservations.sqlite');
    if (created) closeSync(openSync(path, constants.O_CREAT | constants.O_EXCL
      | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600));
    try { privatePath(path, false); }
    catch (error) {
      if (error.code === 'ENOENT') throw new Error('Resource ledger database is missing');
      throw error;
    }
    for (const suffix of ['-journal', '-wal', '-shm']) {
      try { privatePath(path + suffix, false); }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    try {
      this.#db = new DatabaseSync(path);
      this.#db.exec('PRAGMA busy_timeout=5000; PRAGMA journal_mode=DELETE;'
        + ' PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON;');
      this.#db.exec('BEGIN IMMEDIATE');
      if (created) {
        this.#db.exec('CREATE TABLE ledger (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL) STRICT');
        this.#db.prepare('INSERT INTO ledger VALUES (1, ?)').run(JSON.stringify({
          schemaVersion: 1, limits: initialLimits, volumeId: null, vms: [], files: [],
        }));
      }
      this.#read();
      this.#db.exec('COMMIT');
    } catch (error) { this.close(); throw error; }
  }

  // Returns validated durable state inside the caller's transaction.
  #read() {
    const row = this.#db.prepare('SELECT value FROM ledger WHERE id=1').get();
    if (!row) throw new Error('Resource ledger is incomplete');
    const value = JSON.parse(row.value);
    validate(value);
    return value;
  }

  // Holds the database writer lock throughout read, validation and commit.
  #change(operation) {
    this.#db.exec('BEGIN IMMEDIATE');
    try {
      const data = this.#read();
      const result = operation(data);
      validate(data);
      this.#db.prepare('UPDATE ledger SET value=? WHERE id=1').run(JSON.stringify(data));
      this.#db.exec('COMMIT');
      return result;
    } catch (error) {
      try { this.#db.exec('ROLLBACK'); }
      catch { /* A failed disk transaction may already have rolled back. */ }
      throw error;
    }
  }

  // Returns current reservations without presenting them as observed CPU or memory use.
  snapshot(host) { return evaluate(this.#read(), host); }

  // Returns the recorded guests and storage without measuring the host.
  contents() { const { vms, files } = this.#read(); return { vms, files }; }

  // Returns configured global percentages for the host settings interface.
  limits() { return this.#read().limits; }

  // Persists settings even when existing guests exceed the new ceilings.
  configure(value) {
    limits(value);
    this.#change(data => { data.limits = { ...value }; });
  }

  // Commits the entire proposed start before returning its host ownership token.
  reserve(vms, files, host, refresh = file => file) {
    const result = this.#change(data => {
      const lease = randomUUID();
      const previousVMs = structuredClone(data.vms);
      const ids = new Set();
      for (const vm of vms) {
        identity(vm.id);
        if (ids.has(vm.id)) throw new Error('Invalid duplicate VM identity');
        ids.add(vm.id);
        const previous = data.vms.find(row => row.id === vm.id);
        if (previous && previous.state !== 'stopped') throw new Error('VM resource ownership is unresolved');
        const next = { ...vm, lease, state: 'starting' };
        if (previous) Object.assign(previous, next);
        else data.vms.push(next);
      }
      for (const file of files) {
        const previous = data.files.find(row => row.id === file.id);
        if (previous) Object.assign(previous, file, { bytes: Math.max(previous.bytes, file.bytes) });
        else data.files.push({ ...file });
      }
      data.files = data.files.map(refresh);
      const measured = typeof host === 'function' ? host() : host;
      const evaluated = evaluate(data, measured);
      data.volumeId = measured.volumeId;
      if (evaluated.exceeded.length) {
        data.vms = previousVMs;
        return { exceeded: evaluated.exceeded };
      }
      return { lease };
    });
    if (result.exceeded) {
      const error = new Error(`Resource limit exceeded: ${result.exceeded.join(', ')}`);
      error.code = 'resource_limit'; error.resources = result.exceeded;
      throw error;
    }
    return result.lease;
  }

  // Keeps every unconfirmed guest reserved and ignores stale completion tokens.
  settle(lease, confirmedStopped) {
    this.#change(data => {
      for (const vm of data.vms) {
        if (vm.lease === lease) vm.state = confirmedStopped.includes(vm.id) ? 'stopped' : 'unknown';
      }
    });
  }

  // Releases compute for guests whose owner died, after the caller proved they are idle.
  // Only `unknown` rows qualify; disks stay reserved and no row is deleted.
  reconcile(ids) {
    this.#change(data => {
      for (const id of ids) {
        const vm = data.vms.find(row => row.id === id);
        if (vm?.state !== 'unknown') throw new Error('Only a guest in the unknown state can be reconciled');
        vm.state = 'stopped';
      }
    });
  }

  // Drops guests and files that were deleted from disk, which the caller has checked.
  // Only `stopped` guests qualify: any other state may still own compute. A missing file
  // otherwise keeps its full reservation, so deleting a VM would never free its budget.
  forget(ids, paths) {
    this.#change(data => {
      for (const id of ids) {
        if (data.vms.find(row => row.id === id)?.state !== 'stopped') {
          throw new Error('Only a stopped guest can be forgotten');
        }
      }
      data.vms = data.vms.filter(row => !ids.includes(row.id));
      data.files = data.files.filter(file => !paths.includes(file.path));
    });
  }

  // Releases only this database connection; reservations survive process exit.
  close() { this.#db?.close(); this.#db = undefined; }
}
