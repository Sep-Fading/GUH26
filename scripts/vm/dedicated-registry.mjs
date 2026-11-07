// Keeps retained project VM identities in private host storage.
import { randomBytes, randomUUID } from 'node:crypto';
// Reads and atomically replaces the bounded machine registry.
import { closeSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync,
  renameSync, unlinkSync, writeFileSync, fsyncSync, constants } from 'node:fs';
// Deletes one removed VM's folder.
import { rm } from 'node:fs/promises';
// Derives the registry path without accepting guest paths.
import { join } from 'node:path';

const cwd = '/Users/Shared/MiniClaw/Workspace';
const stages = new Set(['preparing', 'installing', 'awaiting_setup', 'failed']);
const vmIdPattern = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;

// Rejects replaced, shared, or nonprivate host metadata.
function owned(path, directory) {
  const info = lstatSync(path);
  if ((directory ? !info.isDirectory() : !info.isFile() || info.nlink !== 1)
    || info.uid !== process.getuid() || info.mode & 0o077) {
    throw new Error('Project VM registry must be private and owned');
  }
  return info;
}

// Checks every retained identity before returning it to the controller.
function validate(rows) {
  if (!Array.isArray(rows) || rows.length > 64) throw new Error('Invalid project VM registry');
  const ids = new Set(), requests = new Set(), slots = new Set();
  for (const row of rows) {
    if (!vmIdPattern.test(row?.vmId) || !vmIdPattern.test(row?.requestId)
      || !Number.isSafeInteger(row?.slot) || row.slot < 0 || row.slot >= 64
      || !/^[a-f0-9]{64}$/.test(row?.key) || row.cwd !== cwd
      || !Number.isSafeInteger(row?.attempt) || row.attempt < 1 || row.attempt > 100
      || !stages.has(row?.stage) || !Number.isSafeInteger(row?.percent)
      || row.percent < 0 || row.percent > 100
      || typeof row.reason !== 'string' || row.reason.length > 160
      || ids.has(row.vmId) || requests.has(row.requestId) || slots.has(row.slot)) {
      throw new Error('Invalid project VM registry');
    }
    ids.add(row.vmId); requests.add(row.requestId); slots.add(row.slot);
  }
}

// Replaces one small private file only after the new contents reach storage.
function persist(directory, rows) {
  validate(rows);
  const target = join(directory, 'registry.json');
  const temporary = join(directory, `registry-${randomUUID()}.tmp`);
  const fd = openSync(temporary, constants.O_WRONLY | constants.O_CREAT
    | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    writeFileSync(fd, `${JSON.stringify({ schemaVersion: 1, machines: rows })}\n`);
    fsyncSync(fd);
  } finally { closeSync(fd); }
  try {
    if (existsSync(target)) owned(target, false);
    renameSync(temporary, target);
    const directoryFd = openSync(directory, constants.O_RDONLY | constants.O_NOFOLLOW);
    try { fsyncSync(directoryFd); } finally { closeSync(directoryFd); }
  } catch (error) {
    try { unlinkSync(temporary); } catch { /* The failed write retains the old registry. */ }
    throw error;
  }
}

// Opens a retained registry, marking interrupted work as failed for explicit retry.
export async function openDedicatedRegistry(root, { setup, stop = async () => {} } = {}) {
  if (typeof setup !== 'function' || typeof stop !== 'function') {
    throw new Error('Project VM setup callback is required');
  }
  owned(root, true);
  const directory = join(root, 'dedicated-vms');
  try { mkdirSync(directory, { mode: 0o700 }); }
  catch (error) { if (error.code !== 'EEXIST') throw error; }
  owned(directory, true);
  const file = join(directory, 'registry.json');
  let rows = [];
  if (existsSync(file)) {
    const info = owned(file, false);
    if (info.size > 32768) throw new Error('Project VM registry is too large');
    const saved = JSON.parse(readFileSync(file, { flag: constants.O_RDONLY | constants.O_NOFOLLOW }));
    if (saved.schemaVersion !== 1) throw new Error('Unknown project VM registry version');
    rows = saved.machines;
    validate(rows);
    let changed = false;
    for (const row of rows) {
      if (row.stage === 'preparing' || row.stage === 'installing') {
        row.stage = 'failed'; row.reason = 'VM setup was interrupted. Its files were kept for retry.';
        changed = true;
      }
    }
    if (changed) persist(directory, rows);
  }
  const jobs = new Map();
  let closing = false;

  // Returns a bounded public state without worker keys or host file paths.
  function status(vmId) {
    const row = rows.find(value => value.vmId === vmId);
    if (!row) return { state: 'missing', reason: 'Assigned VM does not exist.' };
    return { state: row.stage === 'failed' ? 'failed' : 'starting',
      stage: row.stage, percent: row.percent,
      ...(row.reason ? { reason: row.reason } : {}) };
  }

  // Records stage updates before the browser may show them.
  async function report(row, stage, percent) {
    if (!stages.has(stage) || stage === 'failed' || !Number.isSafeInteger(percent)
      || percent < 0 || percent > 100 || closing) throw new Error('Invalid VM setup progress');
    row.stage = stage; row.percent = percent; row.reason = '';
    persist(directory, rows);
  }

  // Runs one cancellable attempt and keeps its files and error on failure.
  function launch(row) {
    const controller = new AbortController();
    const job = Promise.resolve().then(async () => {
      try {
        controller.signal.throwIfAborted();
        await setup({ vmId: row.vmId, slot: row.slot, key: row.key,
          cwd, attempt: row.attempt }, (stage, percent) => report(row, stage, percent),
        controller.signal);
        if (row.stage !== 'awaiting_setup') {
          throw new Error('VM setup did not reach guest setup');
        }
      } catch (error) {
        row.reason = controller.signal.aborted
          ? 'VM setup was cancelled. Its files were kept for retry.'
          : error?.code === 'ENOENT'
            ? 'The cached macOS restore image or setup package is missing.'
          : error?.code === 'ENOSPC'
              ? 'The host ran out of disk space during VM setup.'
              : error?.code === 'resource_limit'
                ? 'VM resource limit exceeded. Its files were kept for retry.'
              : row.stage === 'installing'
                ? 'macOS installation failed. Its files were kept for retry.'
                : 'VM setup failed. Its files were kept for retry.';
        row.stage = 'failed';
        persist(directory, rows);
      }
    }).finally(() => jobs.delete(row.vmId));
    jobs.set(row.vmId, { controller, job });
  }

  return {
    // Starts one fresh install and reuses the same result for a repeated request ID.
    async provision(requestId) {
      if (closing || !vmIdPattern.test(requestId)) throw new Error('Invalid VM setup request');
      const previous = rows.find(row => row.requestId === requestId);
      if (previous) return { vmId: previous.vmId, ...status(previous.vmId) };
      if (rows.length >= 64) throw new Error('Project VM limit reached');
      const slot = Array.from({ length: 64 }, (_, index) => index)
        .find(index => !rows.some(row => row.slot === index));
      const row = { vmId: randomUUID(), requestId, slot,
        key: randomBytes(32).toString('hex'), cwd, attempt: 1,
        stage: 'preparing', percent: 0, reason: '' };
      rows.push(row);
      try { persist(directory, rows); }
      catch (error) { rows.pop(); throw error; }
      launch(row);
      return { vmId: row.vmId, ...status(row.vmId) };
    },
    // Restarts only a failed setup under the same VM identity and a new attempt number.
    async retry(vmId) {
      const row = rows.find(value => value.vmId === vmId);
      if (closing || !row || row.stage !== 'failed' || jobs.has(vmId)
        || row.attempt >= 100) throw new Error('Project VM setup cannot be retried');
      row.attempt++; row.stage = 'preparing'; row.percent = 0; row.reason = '';
      persist(directory, rows);
      launch(row);
      return status(vmId);
    },
    // Cancels the current attempt without deleting its VM files.
    async cancel(vmId) {
      const row = rows.find(value => value.vmId === vmId);
      if (!row) throw new Error('Assigned VM does not exist.');
      const active = jobs.get(vmId);
      if (active) {
        active.controller.abort();
        await active.job;
      } else if (row.stage === 'awaiting_setup') {
        await stop({ vmId: row.vmId, slot: row.slot });
        row.stage = 'failed';
        row.reason = 'VM setup was cancelled. Its files were kept for retry.';
        persist(directory, rows);
      }
      return status(vmId);
    },
    // Deletes one VM's folder for good, then its record. The folder goes first, so a failed
    // deletion keeps the record and can be tried again. The caller checks the VM is stopped.
    async remove(vmId) {
      const index = rows.findIndex(value => value.vmId === vmId);
      if (closing || index < 0 || !vmIdPattern.test(vmId)) throw new Error('Assigned VM does not exist.');
      if (jobs.has(vmId)) throw new Error('This VM is still being set up. Cancel its setup first.');
      await rm(join(directory, vmId), { recursive: true, force: true });
      const [row] = rows.splice(index, 1);
      try { persist(directory, rows); }
      catch (error) { rows.splice(index, 0, row); throw error; }
    },
    // Gives the controller only bounded, distinct worker routes.
    list() { return rows.map(({ vmId, slot, key }) => ({ vmId, slot, key, cwd })); },
    // Returns one private host record for starting its retained guest.
    details(vmId) {
      const row = rows.find(value => value.vmId === vmId);
      return row ? { vmId: row.vmId, slot: row.slot, key: row.key,
        attempt: row.attempt, stage: row.stage } : null;
    },
    status,
    // Waits for one disposable test or real setup attempt to finish.
    async idle(vmId) { await jobs.get(vmId)?.job; },
    // Aborts unfinished setup while retaining registry and guest files.
    async close() {
      if (closing) return;
      closing = true;
      for (const { controller } of jobs.values()) controller.abort();
      await Promise.all([...jobs.values()].map(({ job }) => job));
    },
  };
}
