import { writePrivateFile } from './private-file.mjs';
// Re-checks folder permissions before a backend is offered for elevation.
import { verifyInstallation } from './destination.mjs';
// Builds literal, pinned Windows service installation requests.
import { dirname, isAbsolute, join, relative, win32 } from 'node:path';
// Pins executable content and creates service identities.
import { createHash, randomBytes } from 'node:crypto';
// Copies fresh service credentials into private state.
import { readFile } from 'node:fs/promises';
// Locates the fixed, packaged installation script.
import { fileURLToPath } from 'node:url';
// Runs Windows commands without a command shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// Quotes one PowerShell literal without expanding shell syntax.
function literal(value) { return `'${value.replaceAll("'", "''")}'`; }

// Rejects device, share, relative and alternate-stream paths before elevation.
function localPath(value) {
  if (typeof value !== 'string' || !/^[A-Za-z]:\\/.test(value)
    || /[\0\r\n"]/.test(value) || value.slice(2).includes(':')
    || value.split('\\').some(part => part === '..')) throw new Error('Expected a local absolute Windows path');
  return value;
}

// Accepts canonical interactive-user SID families, never service or built-in principals.
function userSid(value) {
  if (typeof value !== 'string' || !/^S-1-(?:5-21|12-1)(?:-(?:0|[1-9]\d*)){4}$/.test(value)) return false;
  return value.split('-').slice(4).every(part => Number(part) <= 0xffffffff);
}

// Creates a UAC request for the fixed installer, preserving every argument as data.
export function networkServiceCommand({ source, sha256, ownerSid, id, scriptBody,
  systemRoot = 'C:\\Windows' }) {
  localPath(source); localPath(systemRoot + '\\System32');
  if (!/^[a-f0-9]{64}$/.test(sha256) || !/^[a-f0-9]{32}$/.test(id)
    || !userSid(ownerSid)) throw new Error('Invalid service installation identity');
  if (typeof scriptBody !== 'string' || !scriptBody.trim()) throw new Error('Missing installation script');
  const inner = `$ErrorActionPreference = 'Stop'; & {\n${scriptBody}\n} -Source ${literal(source)} -ExpectedSha256 ${literal(sha256)} -OwnerSid ${literal(ownerSid)} -Id ${literal(id)}`;
  const encoded = Buffer.from(inner, 'utf16le').toString('base64');
  const shell = win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const outer = `$p = Start-Process -FilePath ${literal(shell)} -Verb RunAs -Wait -PassThru -ArgumentList '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encoded}'; exit $p.ExitCode`;
  if (outer.length > 30000) throw new Error('Network installer exceeds Windows command-line limit');
  return { file: shell, args: ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', outer] };
}

// Creates the UAC request that removes one installation's service (uninstall): the fixed removal
// script with the service id and the owner's SID as its only, literal arguments.
export function networkRemovalCommand({ id, ownerSid, scriptBody, systemRoot = 'C:\\Windows' }) {
  localPath(systemRoot + '\\System32');
  if (!/^[a-f0-9]{32}$/.test(id) || !userSid(ownerSid)) throw new Error('Invalid service removal identity');
  if (typeof scriptBody !== 'string' || !scriptBody.trim()) throw new Error('Missing removal script');
  const inner = `$ErrorActionPreference = 'Stop'; & {\n${scriptBody}\n} -Id ${literal(id)} -OwnerSid ${literal(ownerSid)}`;
  const encoded = Buffer.from(inner, 'utf16le').toString('base64');
  const shell = win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const outer = `$p = Start-Process -FilePath ${literal(shell)} -Verb RunAs -Wait -PassThru -ArgumentList '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encoded}'; exit $p.ExitCode`;
  if (outer.length > 30000) throw new Error('Network removal exceeds Windows command-line limit');
  return { file: shell, args: ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', outer] };
}

// Marks the error of a backend that does not match its manifest, so an update can word it for a new release.
export const backendMismatch = 'MINICLAW_BACKEND_MISMATCH';

// Returns the backend an elevated step may copy, pinned to its release manifest. The folder's
// permissions are verified first: a hash computed from a folder another account can write
// would only pin whatever that account planted there. An update swaps the backend in before it
// publishes the new manifest, so it names the manifest of its bundle, which `miniclaw update`
// builds inside the same verified installation folder; a manifest anywhere else is refused.
export async function pinnedBackend(runtime, { verify = verifyInstallation, manifest: manifestPath = join(runtime, 'manifest.json') } = {}) {
  const installation = dirname(runtime);
  const inside = relative(installation, manifestPath);
  if (!inside || isAbsolute(inside) || inside.split(/[\\/]/)[0] === '..') {
    throw new Error('The backend manifest must be inside the installation folder');
  }
  await verify(installation);
  const source = join(runtime, 'bin', 'miniclaw-server.exe');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  const sha256 = manifest?.files?.['bin/miniclaw-server.exe'];
  if (typeof sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(sha256)) throw new Error('The runtime manifest does not pin the backend');
  if (createHash('sha256').update(await readFile(source)).digest('hex') !== sha256) {
    throw Object.assign(new Error('The installed backend does not match its release manifest; reinstall MiniClaw'), { code: backendMismatch });
  }
  return { source, sha256 };
}

// How long setup and update wait for the administrator approval Windows asks for: long enough for a user
// who looked away from the consent prompt. The removal step has always waited as long.
export const approvalWaitMs = 5 * 60_000;

// Runs one elevated step and words its failure for a person: a timeout says what to run next, a declined
// prompt says so, and nothing echoes the long encoded command. `action` is 'install' (setup, resumable)
// or 'update' (undone on failure, so run again).
export async function runElevated(command, { action, run = promisify(execFile) } = {}) {
  try { return await run(command.file, command.args, { windowsHide: true, timeout: approvalWaitMs, maxBuffer: 16384 }); }
  catch (error) {
    const minutes = approvalWaitMs / 60_000;
    const next = action === 'update' ? 'Run miniclaw update again and approve the prompt.'
      : 'If the approval window is still open, approve it first. Then finish setup with: miniclaw resume';
    if (error.killed || error.signal) {
      throw Object.assign(new Error(`Windows did not get administrator approval within ${minutes} minutes. ${next}`, { cause: error }), { approvalTimedOut: true });
    }
    const lines = String(error.stderr ?? '').trim().split(/\r?\n/).filter(Boolean);
    if (lines.some(line => /cancel/i.test(line))) {
      throw new Error(`Administrator approval was declined. ${action === 'update' ? 'Run miniclaw update again and approve the prompt.' : 'Finish setup with: miniclaw resume'}`, { cause: error });
    }
    throw new Error(`The elevated step failed (exit ${error.code ?? 'unknown'})${lines.length ? `: ${lines.at(-1)}` : ''}`, { cause: error });
  }
}

// Installs the protected service through UAC and saves its private client configuration.
export async function installWindowsNetwork({ runtime, data, onOutput = () => {}, resume = false, pin = pinnedBackend }) {
  if (process.platform !== 'win32') throw new Error('Network service installation requires Windows');
  const run = promisify(execFile);
  const systemRoot = process.env.SystemRoot || 'C:\\Windows';
  const shell = win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await run(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    '[Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], { windowsHide: true, timeout: 15000 });
  const { source, sha256 } = await pin(runtime);
  const ownerSid = stdout.trim();
  const intentPath = win32.join(data, 'windows-network-pending.json');
  const intent = resume ? JSON.parse(await readFile(intentPath, 'utf8'))
    : { id: randomBytes(16).toString('hex'), ownerSid, sha256 };
  if (intent.ownerSid !== ownerSid || intent.sha256 !== sha256 || !/^[a-f0-9]{32}$/.test(intent.id)) {
    throw new Error('Network installation identity or backend changed; administrator recovery is required');
  }
  const id = intent.id;
  if (!resume) await writePrivateFile(runtime, intentPath, JSON.stringify(intent));
  const scriptBody = await readFile(fileURLToPath(new URL('./windows-network-install.ps1', import.meta.url)), 'utf8');
  const command = networkServiceCommand({ source, sha256, ownerSid, id, systemRoot, scriptBody });
  onOutput('Windows will request administrator approval to install MiniClaw network isolation.\n');
  const configPath = win32.join(process.env.ProgramData || 'C:\\ProgramData', `MiniClawNetwork-${id}`, 'client.json');
  try {
    let contents = resume ? await readFile(configPath, 'utf8').catch(error => {
      if (error.code !== 'ENOENT') throw error; return null;
    }) : null;
    if (contents === null) {
      await runElevated(command, { action: 'install' });
      contents = await readFile(configPath, 'utf8');
    }
    const config = JSON.parse(contents);
    if (config.owner_sid !== ownerSid || config.service_name !== `MiniClawNetwork-${id}`
      || config.pipe_name !== config.service_name || !/^[a-f0-9]{64}$/.test(config.secret_hex)) {
      throw new Error('Installed network service returned a mismatched identity');
    }
    if (resume) await updateWindowsNetwork({ runtime, data, onOutput, config, pin, action: 'install' });
    const privateConfig = win32.join(data, 'windows-network.json');
    await writePrivateFile(runtime, privateConfig, contents);
    return { serviceName: config.service_name, executable: config.service_exe };
  } catch (error) {
    error.windowsNetwork = { serviceName: `MiniClawNetwork-${id}` };
    throw error;
  }
}

// Refreshes the existing protected service without deleting its recovery journal or filters.
export async function updateWindowsNetwork({ runtime, data, onOutput = () => {}, config: suppliedConfig, manifest, pin = pinnedBackend, action = 'update' }) {
  if (process.platform !== 'win32') throw new Error('Network service update requires Windows');
  const config = suppliedConfig ?? JSON.parse(await readFile(win32.join(data, 'windows-network.json'), 'utf8'));
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoProfile', '-NonInteractive', '-Command',
    '[Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], { windowsHide: true, timeout: 15000 });
  if (!userSid(config.owner_sid) || config.owner_sid !== stdout.trim()) throw new Error('Network service belongs to a different interactive user');
  const match = /^MiniClawNetwork-([a-f0-9]{32})$/.exec(config.service_name);
  if (!match || config.pipe_name !== config.service_name) throw new Error('Invalid installed network service identity');
  const { source, sha256 } = await pin(runtime, manifest ? { manifest } : {});
  const scriptBody = await readFile(fileURLToPath(new URL('./windows-network-update.ps1', import.meta.url)), 'utf8');
  const command = networkServiceCommand({ source, sha256, ownerSid: config.owner_sid,
    id: match[1], scriptBody, systemRoot: process.env.SystemRoot || 'C:\\Windows' });
  onOutput('Windows will request administrator approval to update MiniClaw network isolation.\n');
  await runElevated(command, { action });
}

// Removes the installation's service with one administrator approval: the service first removes its
// Winsock entries, then the service, its program folder and its data folder go (windows-network-remove.ps1).
export async function removeWindowsNetwork({ id, ownerSid, onOutput = () => {} }) {
  if (process.platform !== 'win32') throw new Error('Network service removal requires Windows');
  const systemRoot = process.env.SystemRoot || 'C:\\Windows';
  const shell = win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoProfile', '-NonInteractive', '-Command',
    '[Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], { windowsHide: true, timeout: 15000 });
  if (!userSid(ownerSid) || ownerSid !== stdout.trim()) throw new Error('The network service belongs to a different Windows user; sign in as that user to remove it');
  const scriptBody = await readFile(fileURLToPath(new URL('./windows-network-remove.ps1', import.meta.url)), 'utf8');
  const command = networkRemovalCommand({ id, ownerSid, scriptBody, systemRoot });
  onOutput('Windows will request administrator approval to remove MiniClaw network isolation.\n');
  try { await promisify(execFile)(command.file, command.args, { windowsHide: true, timeout: 300000, maxBuffer: 65536 }); }
  catch (error) {
    const reason = String(error.stderr ?? '').trim().split(/\r?\n/).find(line => /cancel/i.test(line))
      ? 'Administrator approval was declined' : `The removal step failed (exit ${error.code ?? 'unknown'})`;
    throw new Error(`${reason}; the service MiniClawNetwork-${id} is kept. Check it in Windows Services, then run miniclaw uninstall again.`, { cause: error });
  }
}
