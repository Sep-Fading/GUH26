// `miniclaw uninstall` on Windows. One administrator approval removes the installation's network
// service (which first removes its own Winsock catalog entries), then the program files go. The data
// folder (conversations, projects, settings) stays unless --delete-data was given and confirmed.
// Reads the installation's service identity.
import { lstat, readFile, rm, writeFile } from 'node:fs/promises';
// Resolves installation paths.
import { join } from 'node:path';
// Asks Windows which processes run this installation's backend.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
// Removes the service with one administrator approval.
import { removeWindowsNetwork } from './windows-network.mjs';
// Forgets the remembered installation.
import { forgetInstallation } from './default-installation.mjs';
// Names the marker a later setup recognises the kept data folder by.
import { keptMarker } from './destination.mjs';

const exists = path => lstat(path).then(() => true, error => { if (error.code === 'ENOENT') return false; throw error; });

// Reports whether a process runs this installation's backend executable (the user's own processes only).
export async function backendRunning(executable) {
  const shell = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    "@(Get-CimInstance Win32_Process -Filter \"Name='miniclaw-server.exe'\" | Where-Object { $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $env:MINICLAW_UNINSTALL_EXE, 'OrdinalIgnoreCase') }).Count"],
  { env: { ...process.env, MINICLAW_UNINSTALL_EXE: executable }, windowsHide: true, timeout: 30000 });
  return Number(stdout.trim()) > 0;
}

// The refusal for an installation whose backend runs: nothing changes while MiniClaw has the folder open.
export function runningMessage(destination, command) {
  return `MiniClaw is running from ${destination}. Stop it (Ctrl+C in its window), then run ${command} again. Nothing was changed.`;
}

// The signed-in Windows user's SID, recorded in the kept-data marker.
async function currentUserSid() {
  const shell = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    '[Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], { windowsHide: true, timeout: 15000 });
  return stdout.trim();
}

// Names the service this installation owns: the finished setup's client configuration, or the
// intent an interrupted setup recorded before it asked for approval. Null when it never got that far.
async function installedService(data) {
  const read = async name => JSON.parse(await readFile(join(data, name), 'utf8'));
  if (await exists(join(data, 'windows-network.json'))) {
    const config = await read('windows-network.json');
    const match = /^MiniClawNetwork-([a-f0-9]{32})$/.exec(config.service_name ?? '');
    if (!match || typeof config.owner_sid !== 'string') throw new Error('Invalid installed network service identity; nothing was changed.');
    return { id: match[1], ownerSid: config.owner_sid };
  }
  if (await exists(join(data, 'windows-network-pending.json'))) {
    const intent = await read('windows-network-pending.json');
    if (!/^[a-f0-9]{32}$/.test(intent.id ?? '') || typeof intent.ownerSid !== 'string') throw new Error('Invalid installed network service identity; nothing was changed.');
    return { id: intent.id, ownerSid: intent.ownerSid };
  }
  return null;
}

// Program files that live in the data folder; everything else there is the user's.
export const programData = ['windows-network.json', 'windows-network-pending.json', 'windows-setup-pending.json',
  'windows-git.json', 'windows-git.json.next', 'windows-git.json.previous', 'windows-git-runtime'];

// Removes one Windows installation. `confirm(message)` answers each question; nothing changes
// before the answers, and a refused approval leaves every file in place.
export async function uninstallWindows(destination, { deleteData = false, confirm, platform = process.platform,
  running = backendRunning, removeService = removeWindowsNetwork, forget = forgetInstallation, userSid = currentUserSid,
  onOutput = text => process.stdout.write(text) } = {}) {
  if (platform !== 'win32') throw new Error('miniclaw uninstall removes MiniClaw from Windows. On a Mac, delete the installation folder.');
  const runtime = join(destination, 'runtime'), data = join(destination, 'data');
  const marked = await Promise.all([join(destination, 'settings.json'), join(runtime, 'manifest.json'),
    join(data, 'windows-setup-pending.json')].map(exists));
  if (!marked.some(Boolean)) throw new Error(`No MiniClaw installation at ${destination}. Nothing was changed.`);
  if (await running(join(runtime, 'bin', 'miniclaw-server.exe'))) {
    throw new Error(runningMessage(destination, 'miniclaw uninstall'));
  }
  const service = await installedService(data);
  const name = service && `MiniClawNetwork-${service.id}`;
  const plan = [`Remove MiniClaw from ${destination}?`,
    service ? `The network isolation service ${name} is removed with one administrator approval.` : 'No network service is installed.',
    deleteData ? 'Your data folder is deleted too.' : `Your data (conversations, projects, settings) stays in ${data}.`].join('\n');
  if (!await confirm(plan)) throw new Error('Uninstall cancelled. Nothing was changed.');
  if (deleteData && !await confirm(`Delete ${data} with all conversations, project settings and history? This cannot be undone.`)) {
    throw new Error('Uninstall cancelled. Nothing was changed.');
  }
  if (service) {
    onOutput(`Removing ${name}; Windows asks for administrator approval once.\n`);
    try { await removeService(service); }
    catch (error) { throw new Error(`${error.message} Nothing else was removed.`, { cause: error }); }
    onOutput(`Removed ${name} and its Windows network access entries.\n`);
  }
  if (deleteData) {
    await rm(destination, { recursive: true, force: true, maxRetries: 3 });
  } else {
    await rm(runtime, { recursive: true, force: true, maxRetries: 3 });
    for (const file of ['settings.json', 'settings.ready.json']) await rm(join(destination, file), { force: true });
    for (const name of programData) await rm(join(data, name), { recursive: true, force: true, maxRetries: 3 });
    // Lets a later `miniclaw setup` into this folder recognise the kept data and install beside it.
    if (await exists(data)) {
      const marker = { kind: 'miniclaw-kept-data', version: 1, destination, ownerSid: service?.ownerSid ?? await userSid(), time: new Date().toISOString() };
      await writeFile(join(data, keptMarker), `${JSON.stringify(marker)}\n`);
    }
  }
  await forget(destination);
  onOutput(deleteData ? `MiniClaw and its data were removed from ${destination}.\n`
    : `MiniClaw was removed. Your data is kept in ${data}; delete that folder yourself when you no longer need it, `
      + `or run miniclaw setup "${destination}" to install MiniClaw there again with it.\n`);
  return { service: name, dataKept: deleteData ? null : data };
}
