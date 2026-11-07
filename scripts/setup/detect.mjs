// Finds values setup would otherwise ask for: the Computer Use helper.
// Read-only: nothing is started except one fixed PowerShell query on Windows, and no secret is read.
import { access } from 'node:fs/promises';
// Builds each platform's own paths, whichever system runs the code (tests run both on both).
import { posix, win32 } from 'node:path';
// Finds the per-user folders when no home is given.
import { homedir } from 'node:os';
// Runs the fixed query script without a shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
// Names the helper file for this processor.
import { windowsHelperName } from './guides.mjs';

const present = path => access(path).then(() => true, () => false);
const helperFolders = ['app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows',
  'app\\resources\\cua_node\\bin\\node_modules\\@oai\\sky\\bin\\windows'];

// Runs windows-detect.ps1 under Windows PowerShell and returns its JSON: installed Codex packages.
export async function queryWindows({ env = process.env } = {}) {
  const shell = win32.join(env.SystemRoot || 'C:\\Windows', 'System32\\WindowsPowerShell\\v1.0\\powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', fileURLToPath(new URL('./windows-detect.ps1', import.meta.url))], { windowsHide: true, timeout: 20000, maxBuffer: 1 << 20 });
  return JSON.parse(stdout.trim() || '{}');
}

// Compares dotted version numbers, newest first.
const newest = (a, b) => {
  const left = String(a ?? '').split('.').map(Number), right = String(b ?? '').split('.').map(Number);
  for (let index = 0; index < Math.max(left.length, right.length); index++) {
    const difference = (right[index] || 0) - (left[index] || 0);
    if (difference) return difference;
  }
  return 0;
};
const list = value => Array.isArray(value) ? value : value ? [value] : [];

// Returns the helper inside the newest installed OpenAI.Codex package for this processor, or null.
export async function findWindowsComputerHelper({ arch = process.arch, query = queryWindows, exists = present, result } = {}) {
  let packages;
  try { packages = list((result ?? await query()).codex); } catch { return null; }
  const name = windowsHelperName(arch);
  for (const item of packages.filter(item => win32.isAbsolute(item?.location ?? '') && /^[A-Za-z]:/.test(item.location))
    .sort((a, b) => newest(a.version, b.version))) {
    for (const folder of helperFolders) {
      const path = win32.join(item.location, folder, name);
      if (await exists(path)) return path;
    }
  }
  return null;
}

// Returns ~/.codex/computer-use/Codex Computer Use.app when the Codex app installed it, or null.
export async function findMacComputerApp({ home = homedir(), exists = present } = {}) {
  const path = posix.join(home, '.codex/computer-use/Codex Computer Use.app');
  return await exists(path) ? path : null;
}

// Detects everything at once; a failure of one detector leaves only its value null.
export async function detectSetupValues({ platform = process.platform, arch = process.arch, env = process.env, home = homedir(),
  exists = present, query = queryWindows } = {}) {
  const result = platform === 'win32' ? await query({ env }).catch(() => ({})) : {};
  const safe = promise => promise.catch(() => null);
  const [windowsComputerHelper, computerApp] = await Promise.all([
    platform === 'win32' ? safe(findWindowsComputerHelper({ arch, exists, result })) : null,
    platform === 'darwin' ? safe(findMacComputerApp({ home, exists })) : null,
  ]);
  return { windowsComputerHelper, computerApp };
}
