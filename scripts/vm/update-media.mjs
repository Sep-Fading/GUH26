#!/usr/bin/env node
// Rebuilds an installation's VM setup disk with the current worker and capture helper.
// Finds VM processes and builds the worker.
import { execFile } from 'node:child_process';
// Reads settings and swaps the setup disk folders.
import { copyFile, chmod, lstat, readFile, rename, rm, writeFile } from 'node:fs/promises';
// Resolves installation and source paths.
import { dirname, join, resolve } from 'node:path';
// Finds the user's Codex Computer Use helper.
import { homedir } from 'node:os';
// Locates this checkout's sources.
import { fileURLToPath } from 'node:url';
// Awaits child processes without a shell.
import { promisify } from 'node:util';
// Builds the read-only setup disk.
import { prepareMasterMedia } from './master-media.mjs';
// Uses a release's prebuilt files instead of compiling.
import { prebuilt } from '../setup/prebuilt.mjs';
// Remembers where MiniClaw was installed.
import { defaultInstallation } from '../setup/default-installation.mjs';

const exec = promisify(execFile);
const source = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

// Reports whether a VM supervisor of this installation is alive.
async function supervisorRunning(destination) {
  try {
    await exec('/usr/bin/pgrep', ['-f', `${join(destination, 'master')}/`]);
    return true;
  } catch { return false; }
}

// Compiles the release worker from this checkout and returns its path.
async function cargoWorker() {
  // A release brings the worker already built.
  const ready = await prebuilt.file('miniclaw-worker');
  if (ready) return ready;
  await exec('cargo', ['build', '--release', '--locked', '--bin', 'miniclaw-worker',
    '--target-dir', join(source, 'target')], { cwd: join(source, 'server'), maxBuffer: 1 << 26 });
  return join(source, 'target/release/miniclaw-worker');
}

// Replaces the setup disk while keeping the pairing key, so the Mac still trusts the VM.
// `computerApp` adds the Codex Computer Use helper to the disk; the choice is remembered.
export async function updateMasterMedia({ destination, vmRunning = supervisorRunning,
  buildWorker = cargoWorker, buildMedia = prepareMasterMedia, now = Date.now, computerApp }) {
  destination = resolve(destination);
  const settings = JSON.parse(await readFile(join(destination, 'settings.json'), 'utf8'));
  if (!settings.vmReady) throw new Error('This installation has no VM to update.');
  // The running VM has the old disk attached.
  if (await vmRunning(destination)) throw new Error('Shut the VM down first, then run this again.');
  const worker = JSON.parse(await readFile(join(destination, 'worker.json'), 'utf8'));
  const archive = settings.downloads?.map(item => item.path)
    .find(path => /codex-package[^/]*\.tar\.gz$/.test(path));
  if (!archive) throw new Error('The cached Codex package is not recorded in settings.json.');
  if (computerApp) {
    computerApp = resolve(computerApp);
    const info = await lstat(computerApp).catch(() => null);
    if (!info?.isDirectory()) throw new Error(`The Computer Use application was not found at ${computerApp}.`);
  }
  const helper = computerApp ?? settings.computer?.path;
  const binary = await buildWorker();
  const current = join(destination, 'master-setup');
  const staged = join(destination, 'master-setup.new');
  await rm(staged, { recursive: true, force: true });
  try {
    await buildMedia({ output: staged, archive, computerApp: helper,
      worker: { binary, settings: worker } });
  } catch (error) {
    await rm(staged, { recursive: true, force: true });
    throw error;
  }
  // The new disk is complete before anything is touched, so no backup is kept: the old
  // folder steps aside for the swap and is removed once the new one is in place.
  const retired = join(destination, `master-setup.old-${now()}`);
  await rename(current, retired);
  try {
    await rename(staged, current);
  } catch (error) {
    await rename(retired, current);
    throw error;
  }
  await rm(retired, { recursive: true, force: true });
  // Project VMs are provisioned from this copy of the worker.
  const runtimeWorker = join(destination, 'runtime/miniclaw-worker');
  await copyFile(binary, `${runtimeWorker}.new`);
  await chmod(`${runtimeWorker}.new`, 0o700);
  await rename(`${runtimeWorker}.new`, runtimeWorker);
  if (computerApp) {
    const next = { ...settings, computer: { ...settings.computer, path: computerApp },
      masterSetup: { ...settings.masterSetup, computerUse: true } };
    // New project VMs are provisioned from this record.
    if (next.pair?.provisioning) next.pair.provisioning = { ...next.pair.provisioning, computerApp };
    const staging = join(destination, `settings.json.${now()}`);
    await writeFile(staging, `${JSON.stringify(next, null, 2)}\n`, { mode: 0o600 });
    await rename(staging, join(destination, 'settings.json'));
  }
  return { media: join(current, 'MasterSetup.dmg'), computerUse: Boolean(helper) };
}

// Runs the update for the remembered or named installation.
async function main() {
  const args = process.argv.slice(2);
  const flag = args.indexOf('--computer-use');
  let computerApp;
  if (flag >= 0) {
    // An optional path may follow the flag; the default is Codex's own copy.
    const given = args[flag + 1]?.endsWith('.app') ? args[flag + 1] : undefined;
    computerApp = given ?? join(homedir(), '.codex/computer-use/Codex Computer Use.app');
    args.splice(flag, given ? 2 : 1);
  }
  const destination = args[0] ?? await defaultInstallation();
  console.log('Building the worker and the setup disk. This takes a minute or two.');
  const { computerUse } = await updateMasterMedia({ destination, computerApp });
  console.log(`The setup disk is updated in place; this takes no extra disk space and creates no VM.
Next: start MiniClaw and a VM chat, tick "Control this VM" in the VM window, open the
"MiniClawSetup" disk in the VM's Finder and run "Install Codex.command" again. It replaces
the worker and the capture helper and keeps the Codex sign-in.`);
  if (computerUse) {
    console.log(`The disk carries the Computer Use helper. After the installer, allow Accessibility and
Screen Recording for "Codex Computer Use" in the VM's System Settings when macOS asks.`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
