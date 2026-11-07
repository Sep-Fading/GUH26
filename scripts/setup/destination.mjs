// Validates, claims and re-verifies the installation folder. On Windows the backend inside
// it is later copied into a LocalSystem service, so the folder must stay private to the user.
// Checks destinations and creates the Mac folder.
import { lstat, mkdir, readFile, readdir } from 'node:fs/promises';
// Interprets destinations in the target platform's form.
import path from 'node:path';
// Runs the fixed Windows permission script without a shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

// Names why a Windows path cannot hold an installation: shares, devices, streams and
// relative forms have no local ACL chain the installer can verify.
function windowsPathError(value) {
  const text = value.replaceAll('/', '\\');
  if (/^\\\\[^?.\\]/.test(text)) return 'Network shares cannot hold an installation; choose a local drive path such as C:\\Users\\you\\MiniClaw';
  if (!/^[A-Za-z]:\\[^\\]/.test(text) || text.slice(2).includes(':') || /[\0\r\n"<>|?*]/.test(text.slice(2))
    || text.split('\\').some(part => part === '..' || part === '.')) {
    return 'Enter an absolute local drive path such as C:\\Users\\you\\MiniClaw';
  }
}

// The settings of a finished installation at `destination`, or null when it holds none (nothing there, an
// unfinished setup, or a folder uninstall left). Setup run again on such a folder says it is installed and changes nothing.
export async function readInstallation(destination) {
  try {
    const settings = JSON.parse(await readFile(path.join(destination, 'settings.json'), 'utf8'));
    if (settings?.schemaVersion !== 1 || settings.backend !== 'host' || settings.hostReady !== true) return null;
    if (!(await lstat(path.join(destination, 'runtime', 'manifest.json'))).isFile()) return null;
    return settings;
  } catch { return null; }
}

// Rejects existing, relative or non-local installation directories before approval. The existing folders
// accepted are a Windows folder that `miniclaw uninstall` left with the user's data (`keptData`), and a finished
// installation, which setup then names as installed instead of installing (`readInstallation`).
export async function destinationError(value, { platform = process.platform, kept = keptData, stat = lstat, installed = readInstallation } = {}) {
  const api = platform === 'win32' ? path.win32 : path;
  if (!value || typeof value !== 'string' || !(platform === 'win32' ? /^([A-Za-z]:)?[\\/]/.test(value) : api.isAbsolute(value))) {
    return 'Enter an absolute installation directory';
  }
  if (platform === 'win32') { const problem = windowsPathError(value); if (problem) return problem; }
  try { await stat(value); }
  catch (error) { return error.code === 'ENOENT' ? undefined : error.message; }
  if (platform === 'win32' && await kept(value, { platform })) return;
  if (await installed(value)) return;
  return 'That path already exists; choose a new directory';
}

// The file `miniclaw uninstall` leaves in the data folder it keeps, naming the folder and its owner.
export const keptMarker = 'kept-after-uninstall.json';

// Recognises a folder that `miniclaw uninstall` left on Windows without --delete-data: it holds only
// `data`, which holds uninstall's marker for this very folder and this user, and the permission script
// confirms the folder is still protected, owned by this user and private, as setup made it. Returns
// `{ destination, data }`, or null for anything else; no other existing folder is ever adopted.
export async function keptData(destination, { platform = process.platform,
  verify = (folder, options) => windowsDirectory(folder, ['-Kept'], options), ...options } = {}) {
  // Files are read with this machine's own path rules; a Windows path must also be a plain local one.
  const api = process.platform === 'win32' ? path.win32 : path;
  if (platform !== 'win32' || typeof destination !== 'string' || (process.platform === 'win32' && windowsPathError(destination))) return null;
  try {
    const data = api.join(destination, 'data');
    const [folder, inner] = await Promise.all([lstat(destination), lstat(data)]);
    if (!folder.isDirectory() || folder.isSymbolicLink() || !inner.isDirectory() || inner.isSymbolicLink()) return null;
    const names = await readdir(destination);
    if (names.length !== 1 || names[0].toLowerCase() !== 'data') return null;
    const file = await lstat(api.join(data, keptMarker));
    if (!file.isFile() || file.size > 4096) return null;
    const marker = JSON.parse(await readFile(api.join(data, keptMarker), 'utf8'));
    const same = (a, b) => typeof a === 'string' && api.resolve(a).toLowerCase() === api.resolve(b).toLowerCase();
    if (marker?.kind !== 'miniclaw-kept-data' || marker.version !== 1 || !same(marker.destination, destination)) return null;
    const user = await verify(destination, options);
    if (!marker.ownerSid || String(user?.stdout ?? '').trim() !== marker.ownerSid) return null;
    return { destination, data };
  } catch { return null; }
}

// Checks a kept folder again right before setup installs into it; throws the script's own reason.
export async function adoptKept(destination, { platform = process.platform, ...options } = {}) {
  if (platform === 'win32') await windowsDirectory(destination, ['-Kept'], options);
}

// Runs the fixed permission script and reports its own one-line diagnosis.
async function windowsDirectory(destination, mode, { run = promisify(execFile), systemRoot = process.env.SystemRoot || 'C:\\Windows' }) {
  const problem = windowsPathError(destination);
  if (problem) throw new Error(problem);
  const shell = path.win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  try {
    return await run(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
      '-File', fileURLToPath(new URL('./windows-install-directory.ps1', import.meta.url)), '-Path', destination, ...mode],
    { windowsHide: true, timeout: 30000, maxBuffer: 16384 });
  } catch (error) {
    const reason = String(error.stderr ?? '').trim().split(/\r?\n/).at(-1);
    throw new Error(reason || `Could not verify installation folder permissions: ${error.message}`, { cause: error });
  }
}

// Creates the new installation folder. Windows gets a protected ACL for the installing user,
// SYSTEM and Administrators, and refuses ancestors another account could replace; POSIX
// permission bits mean nothing there. macOS keeps its owner-only folder.
export async function claimDestination(destination, { platform = process.platform, ...options } = {}) {
  if (platform === 'win32') return windowsDirectory(destination, ['-Create'], options);
  await mkdir(destination, { mode: 0o700 });
}

// Re-checks an installation before its backend is hashed for elevation. Nothing to do on macOS.
export async function verifyInstallation(destination, { platform = process.platform, ...options } = {}) {
  if (platform === 'win32') await windowsDirectory(destination, [], options);
}
