// Reads and writes the remembered installation.
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
// Finds the per-user settings folder.
import { homedir } from 'node:os';
// Resolves the record and installation paths.
import { isAbsolute, join, win32 } from 'node:path';

// Returns platform-owned installation, cache and selection-record locations.
export function installationPaths(home = homedir(), { platform = process.platform, env = process.env } = {}) {
  if (platform === 'win32') {
    const local = env.LOCALAPPDATA || win32.join(home, 'AppData', 'Local');
    return { installation: win32.join(local, 'MiniClaw Install'),
      record: win32.join(local, 'MiniClaw', 'installation.json'),
      cache: win32.join(local, 'MiniClaw', 'setup-cache') };
  }
  return { installation: join(home, 'Library/Application Support/MiniClaw Install'),
    record: join(home, 'Library/Application Support/MiniClaw/installation.json'),
    cache: join(home, 'Library/Caches/MiniClaw-setup') };
}

// Returns the remembered installation, or the standard location when none is recorded.
// `options` are those of installationPaths (a test passes a private profile's environment).
export async function defaultInstallation(home = homedir(), options = {}) {
  try {
    const saved = JSON.parse(await readFile(installationPaths(home, options).record, 'utf8'));
    if (typeof saved.path === 'string' && isAbsolute(saved.path)) return saved.path;
  } catch { /* No record yet, or an unreadable one: fall back to the standard location. */ }
  return installationPaths(home, options).installation;
}

// Remembers one installation so later commands need no path.
export async function rememberInstallation(path, home = homedir(), options = {}) {
  if (!isAbsolute(path)) throw new Error('An installation path must be absolute');
  const file = installationPaths(home, options).record;
  await mkdir(join(file, '..'), { recursive: true, mode: 0o700 });
  await writeFile(`${file}.tmp`, `${JSON.stringify({ path })}\n`, { mode: 0o600 });
  await rename(`${file}.tmp`, file);
}

// Forgets the remembered installation when it is `path` (uninstall); another remembered one stays.
export async function forgetInstallation(path, home = homedir(), options = {}) {
  const file = installationPaths(home, options).record;
  let saved;
  try { saved = JSON.parse(await readFile(file, 'utf8')); } catch { return; }
  const same = (options.platform ?? process.platform) === 'win32' ? (a, b) => a.toLowerCase() === b.toLowerCase() : (a, b) => a === b;
  if (typeof saved.path === 'string' && same(saved.path, path)) await rm(file, { force: true });
}
