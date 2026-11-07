// Recovers only an installation's private pending Windows setup receipt.
import { readFile, writeFile, rm, lstat } from 'node:fs/promises';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { installWindowsNetwork } from './windows-network.mjs';
// Explains a missing or finished installation in the same words as a Mac resume.
import { readReceipt } from './receipt.mjs';

// Returns when the installation holds a pending Windows setup; otherwise says, before any
// terminal or elevation is needed, why there is nothing to resume and what to run instead.
export async function pendingWindowsSetup(destination) {
  const pending = await lstat(join(destination, 'data', 'windows-setup-pending.json')).then(() => true, error => {
    if (error.code !== 'ENOENT') throw error;
    return false;
  });
  if (pending) return;
  // A Windows receipt never holds a VM, so this explains a missing, foreign or finished folder.
  let finished;
  try { await readReceipt(destination); }
  catch (error) {
    if (!/has no VM/.test(error.message)) throw error;
    finished = error;
  }
  // A finished installation is a no-op for resume, not a failure: say so and how to start it.
  if (finished) {
    throw Object.assign(new Error(`MiniClaw is already installed at ${destination}. Nothing to resume.\n`
      + 'Start it with .\\miniclaw.cmd in the release folder; .\\miniclaw.cmd repair sets up its network access again.'), { nothingToResume: true });
  }
  throw new Error(`No pending Windows setup at ${destination}. Use miniclaw start if installation finished.`);
}

// Validates data permissions using the native backend before trusting a recovery receipt.
export async function resumeWindows(destination, { secure = async (runtime, data) => {
  await promisify(execFile)(join(runtime, 'bin/miniclaw-server.exe'), ['--secure-data-dir', data]);
}, recoverNetwork = installWindowsNetwork, onOutput = () => {} } = {}) {
  const runtime = join(destination, 'runtime');
  const data = join(destination, 'data');
  await secure(runtime, data);
  const pending = join(data, 'windows-setup-pending.json');
  const info = await lstat(pending).catch(error => {
    if (error.code !== 'ENOENT') throw error;
    throw new Error(`No pending Windows setup at ${destination}. Use miniclaw start if installation finished.`);
  });
  if (!info.isFile() || info.isSymbolicLink() || info.size > 65536) throw new Error('Invalid pending setup receipt');
  const settings = JSON.parse(await readFile(pending, 'utf8'));
  if (settings.schemaVersion !== 1 || settings.backend !== 'host' || settings.hostReady !== false
    || settings.status !== 'awaiting-network-service' || settings.pair || settings.vmEnabled
    || !Number.isInteger(settings.port) || settings.port < 1 || settings.port > 65535) throw new Error('Invalid pending Windows setup');
  settings.windowsNetwork = await recoverNetwork({ runtime, data, onOutput, resume: true });
  settings.hostReady = true;
  settings.status = 'installed';
  const target = join(destination, 'settings.json');
  const contents = `${JSON.stringify(settings, null, 2)}\n`;
  try { await writeFile(target, contents, { flag: 'wx', mode: 0o600 }); }
  catch (error) {
    if (error.code !== 'EEXIST' || await readFile(target, 'utf8') !== contents) throw error;
  }
  await rm(pending);
  await rm(join(data, 'windows-network-pending.json'), { force: true });
  return settings;
}
