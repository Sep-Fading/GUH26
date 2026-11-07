// Checks Windows private state the way the backend creates it: a protected access list whose
// owner and every rule belong to the current user. POSIX mode bits mean nothing on Windows, so
// tests that check `mode & 0o777` on the Mac check this on Windows instead.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { win32 } from 'node:path';

// The same checks as the fixture test in tests/harness.test.mjs. The path travels in an
// environment variable, never inside the script text.
const SCRIPT = '$acl=Get-Acl -LiteralPath $env:MC_PRIVATE_PATH; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; '
  + 'if (!$acl.AreAccessRulesProtected) { throw "inherits its parent access list" }; '
  + 'if (!$acl.Access.Count) { throw "has no explicit access rule" }; '
  + 'if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw "has a different owner" }; '
  + 'foreach ($entry in $acl.Access) { if ($entry.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw "grants another identity" } }';

/** Resolves when `path` is private to the current Windows user; rejects with the reason otherwise. */
export async function assertWindowsPrivate(path) {
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  try {
    await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', SCRIPT],
      { env: { ...process.env, MC_PRIVATE_PATH: path }, timeout: 30000, windowsHide: true });
  } catch (error) {
    const reason = String(error.stderr ?? '').split(/\r?\n/).find(line => line.trim()) ?? error.message;
    throw new Error(`${path} is not private to the current user: ${reason}`, { cause: error });
  }
}
