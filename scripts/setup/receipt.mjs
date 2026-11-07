// Reads the private installation receipt.
import { lstat, readFile } from 'node:fs/promises';
// Locates the receipt inside an installation.
import { join, win32 } from 'node:path';
// Reads a Windows receipt's owner and access list through a fixed script, without a shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// Windows has no uid or mode bits: Node reports uid 0 and a mode made from the read-only flag.
// Its equivalent of "private and owned by you" is the owner and the access list. Like the
// installation folder check (windows-install-directory.ps1), the owner must be you, SYSTEM or
// Administrators, and no other account may have any access. The path travels in the environment.
const WINDOWS_PRIVATE = '$ErrorActionPreference="Stop"; $a=Get-Acl -LiteralPath $env:MINICLAW_RECEIPT; '
  + '$t=@([Security.Principal.WindowsIdentity]::GetCurrent().User.Value,"S-1-5-18","S-1-5-32-544"); '
  + 'if ($a.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $t) { exit 3 }; '
  + 'foreach ($r in $a.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) { '
  + 'if ($r.AccessControlType -eq "Allow" -and $r.IdentityReference.Value -notin $t) { exit 4 } }';

// Reports whether a receipt is private to the current user on this platform.
async function privateReceipt(file, info) {
  if (process.platform !== 'win32') return info.uid === process.getuid() && !(info.mode & 0o077);
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  return promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', WINDOWS_PRIVATE],
    { env: { ...process.env, MINICLAW_RECEIPT: file }, windowsHide: true, timeout: 30000, maxBuffer: 16384 })
    .then(() => true, () => false);
}

// Explains an installation folder that does not exist and what to run instead.
function noInstallation(destination) {
  return new Error(`No installation at ${destination}.\n`
    + 'An install that is interrupted before it finished is removed, so there is nothing to resume.\n'
    + `Run it again with: miniclaw setup "${destination}"\n`
    + 'Downloads are kept in the cache, so nothing is fetched twice.');
}

// Returns the receipt of an installation whose VM setup is still waiting, or explains in
// plain words why there is nothing to resume and what to run instead.
export async function readReceipt(destination) {
  const file = join(destination, 'settings.json');
  const info = await lstat(file).catch(error => {
    if (error.code !== 'ENOENT') throw error;
    throw noInstallation(destination);
  });
  if (!info.isFile() || info.size > 65536 || !await privateReceipt(file, info)) {
    throw new Error('The installation receipt must be private and owned by you');
  }
  const saved = JSON.parse(await readFile(file, 'utf8'));
  const start = `Start it with: miniclaw start "${destination}"`;
  if (saved.schemaVersion !== 1 || saved.backend !== 'host') {
    throw new Error('This folder does not hold a MiniClaw installation that setup can resume');
  }
  if (!saved.pair?.mac) throw new Error(`This installation has no VM. Nothing to resume.\n${start}`);
  if (saved.pair.root !== destination || saved.pair.port !== saved.port) {
    throw new Error('This receipt does not belong to this folder');
  }
  if (saved.vmReady === true) throw new Error(`VM setup already finished here.\n${start}`);
  return saved;
}
