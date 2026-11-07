// Claims one fresh attempt without reading Master guest files.
import { chmod, copyFile, mkdir, lstat, readFile, writeFile } from 'node:fs/promises';
// Records the kept helpers' checksums.
import { createHash } from 'node:crypto';
// Tells a release from a checkout.
import { prebuilt } from '../setup/prebuilt.mjs';
// Refuses to replace an existing installed worker.
import { constants } from 'node:fs';
// Builds guest and media paths only from a minted VM identity.
import { isAbsolute, join } from 'node:path';
// Resolves packaged source modules without shell paths.
import { fileURLToPath } from 'node:url';
// Verifies cached Apple and Codex downloads before use.
import { verifyDownload } from '../setup/download.mjs';
// Runs the existing signed native VM installer without a shell.
import { run } from '../setup/system.mjs';
// Builds fresh read-only software media with this VM's worker key.
import { prepareMasterMedia } from './master-media.mjs';

const uuid = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const cwd = '/Users/Shared/MiniClaw/Workspace';

// Names the files a project VM's setup needs, by folder inside an installation.
export const supportFiles = [
  ['runtime', ['dedicated-setup.mjs', 'dedicated-registry.mjs', 'master-media.mjs',
    'codex-package.mjs', 'computer-source.mjs', 'master-probe.c',
    'desktop-stream.swift',
    'install-master.command']],
  ['setup', ['download.mjs', 'system.mjs', 'prebuilt.mjs']],
];

// Packages fresh-media tooling and the built worker inside one installation.
export async function packageDedicatedSupport(destination, source, workerBinary) {
  const scripts = source instanceof URL ? fileURLToPath(source) : source;
  if (!isAbsolute(destination) || !isAbsolute(scripts) || !isAbsolute(workerBinary)) {
    throw new Error('Invalid project VM package paths');
  }
  const root = await lstat(destination);
  if (!root.isDirectory() || root.uid !== process.getuid() || root.mode & 0o077) {
    throw new Error('Project VM package root is not private');
  }
  for (const [directory, names] of supportFiles) {
    const target = join(destination, directory);
    await mkdir(target, { recursive: true, mode: 0o700 });
    for (const name of names) {
      await copyFile(join(scripts, directory === 'runtime' ? 'vm' : 'setup', name),
        join(target, name), constants.COPYFILE_EXCL);
    }
  }
  // An installation made from a release keeps the two guest helpers prebuilt, so a project VM's
  // setup disk can be made later on a Mac without compilers.
  if (await prebuilt.available()) {
    const kept = join(destination, 'prebuilt');
    await mkdir(kept, { recursive: true, mode: 0o700 });
    const files = {};
    for (const name of ['miniclaw-desktop-stream', 'socket-probe']) {
      const bytes = await readFile(await prebuilt.file(name));
      await writeFile(join(kept, name), bytes, { mode: 0o755 });
      files[name] = createHash('sha256').update(bytes).digest('hex');
    }
    await writeFile(join(kept, 'manifest.json'), `${JSON.stringify({ schemaVersion: 1, arch: process.arch, files }, null, 2)}\n`);
  }
  const installedWorker = join(destination, 'runtime/miniclaw-worker');
  await copyFile(workerBinary, installedWorker, constants.COPYFILE_EXCL);
  await chmod(installedWorker, 0o700);
  return installedWorker;
}

// Installs macOS on a new disk and returns its retained paths and capacity lease.
export async function prepareDedicatedGuest(pair, machine, {
  signal, report = async () => {}, verify = verifyDownload,
  media = prepareMasterMedia, runNative = run, reserve = async () => null,
} = {}) {
  const source = pair?.provisioning;
  const resources = source?.resources;
  if (!isAbsolute(pair?.root ?? '') || !isAbsolute(pair?.master?.binary ?? '')
    || !isAbsolute(source?.restoreImage ?? '') || !isAbsolute(source?.codexArchive ?? '')
    || !isAbsolute(source?.workerBinary ?? '') || !uuid.test(machine?.vmId)
    || !Number.isSafeInteger(machine?.slot) || machine.slot < 0 || machine.slot >= 64
    || !/^[a-f0-9]{64}$/.test(machine?.key)
    || !Number.isSafeInteger(machine?.attempt) || machine.attempt < 1
    || machine.attempt > 100
    || !Number.isSafeInteger(resources?.cpuCount) || resources.cpuCount < 2
    || !Number.isSafeInteger(resources?.memoryGiB) || resources.memoryGiB < 4
    || !Number.isSafeInteger(resources?.diskGiB) || resources.diskGiB < 64) {
    throw new Error('Invalid project VM setup configuration');
  }
  const rootInfo = await lstat(pair.root);
  if (!rootInfo.isDirectory() || rootInfo.uid !== process.getuid()
    || rootInfo.mode & 0o077) throw new Error('Project VM root is not private');
  signal?.throwIfAborted();
  await report('preparing', 0);
  await verify(source.restoreImage, source.restoreDescriptor, signal);
  await verify(source.codexArchive, source.codexDescriptor, signal);
  const registryRoot = join(pair.root, 'dedicated-vms');
  try { await mkdir(registryRoot, { mode: 0o700 }); }
  catch (error) { if (error.code !== 'EEXIST') throw error; }
  const registryInfo = await lstat(registryRoot);
  if (!registryInfo.isDirectory() || registryInfo.uid !== process.getuid()
    || registryInfo.mode & 0o077) throw new Error('Project VM registry root is not private');
  const vmRoot = join(pair.root, 'dedicated-vms', machine.vmId);
  try { await mkdir(vmRoot, { mode: 0o700 }); }
  catch (error) { if (error.code !== 'EEXIST') throw error; }
  const vmInfo = await lstat(vmRoot);
  if (!vmInfo.isDirectory() || vmInfo.uid !== process.getuid()
    || vmInfo.mode & 0o077) throw new Error('Project VM directory is not private');
  const attempt = join(vmRoot, `Attempt-${machine.attempt}`);
  await mkdir(attempt, { mode: 0o700 });
  await report('preparing', 3);
  const output = join(attempt, 'setup');
  const prepared = await media({ output, archive: source.codexArchive,
    // The helper is optional; without it this VM simply reports Computer Use as unavailable.
    computerApp: isAbsolute(source.computerApp ?? '') ? source.computerApp : undefined,
    worker: { binary: source.workerBinary,
      settings: { key: machine.key, cwd } }, signal });
  const guest = join(attempt, 'Guest');
  await runNative(pair.master.binary, ['init', guest, source.restoreImage,
    String(resources.cpuCount), String(resources.memoryGiB), String(resources.diskGiB)],
  { signal });
  await report('installing', 0);
  const lease = await reserve({ guest, media: prepared.image });
  let lines = '', updates = Promise.resolve();
  try {
    await runNative(pair.master.binary, ['install', guest, source.restoreImage], {
      signal,
      onOutput(chunk) {
        lines += chunk;
        if (lines.length > 4096) lines = lines.slice(-1024);
        while (lines.includes('\n')) {
          const end = lines.indexOf('\n');
          const line = lines.slice(0, end);
          lines = lines.slice(end + 1);
          if (line.length > 1024) continue;
          try {
            const value = JSON.parse(line);
            if (value.status === 'installing' && Number.isSafeInteger(value.percent)
              && value.percent >= 0 && value.percent <= 100) {
              updates = updates.then(() => report('installing', value.percent));
            }
          } catch { /* Native diagnostic text is never sent to the browser. */ }
        }
      },
    });
    await updates;
    await report('installing', 100);
    return { guest, media: prepared.image, lease };
  } catch (error) {
    lease?.settle(['master']);
    throw error;
  }
}
