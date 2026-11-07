// Windows-only facts about a test's own token: whether it may plant file links, and who owns what it creates.
import { execFileSync } from 'node:child_process';
import { symlink } from 'node:fs/promises';

/** Creates a link, or answers false where Windows refuses this token: an ordinary token may
 *  create file links only in Developer Mode, an elevated one always may. Other errors are thrown. */
export async function linkOrRefused(target, path, type) {
  try { await symlink(target, path, type); return true; }
  catch (error) { if (process.platform === 'win32' && error.code === 'EPERM') return false; throw error; }
}
export const NO_LINKS = 'this Windows token may not create file links (no Developer Mode, not elevated)';

/** Makes the signed-in user the owner of a fixture tree. An elevated token creates folders owned by
 *  Administrators, and the bundled Git refuses a repository whose owner is not the user; a
 *  folder the user made in the ordinary way is the user's. Nothing to do elsewhere. */
export function ownAsUser(path) {
  if (process.platform !== 'win32') return;
  execFileSync('icacls.exe', [path, '/setowner', `${process.env.USERDOMAIN}\\${process.env.USERNAME}`, '/T', '/C', '/Q'], { stdio: 'pipe' });
}
