// Checks a selected installed native helper without opening provider credentials.
import { realpath, stat } from 'node:fs/promises';
import { basename, isAbsolute, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// Uses a fixed script and literal argument binding; user paths never become PowerShell code.
async function signature(path) {
  if (process.platform !== 'win32') throw new Error('Windows helper verification requires Windows');
  const shell = join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
  const { stdout } = await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', fileURLToPath(new URL('./windows-computer-signature.ps1', import.meta.url)), path],
  { windowsHide: true, timeout: 15000, maxBuffer: 8192 });
  return JSON.parse(stdout);
}

// An error that names the step-by-step guide the wizard prints before offering to change the choice.
const helperError = message => Object.assign(new Error(`${message}\nThe helper is codex-computer-use-arm64.exe or codex-computer-use.exe `
  + 'inside the Codex app package. See: miniclaw guide computer-use'), { guide: 'computer-use' });

// Returns reviewed identity for an existing signed helper; installation verifies it again.
export async function inspectWindowsComputerHelper(path, { verify = signature } = {}) {
  if (typeof path !== 'string' || !isAbsolute(path)
    || !['codex-computer-use.exe', 'codex-computer-use-arm64.exe'].includes(basename(path))) {
    throw helperError('Choose the absolute installed codex-computer-use.exe or codex-computer-use-arm64.exe path.');
  }
  const resolved = await realpath(path).catch(() => { throw helperError(`No file at ${path}. After a Codex app update its folder name changes.`); });
  const info = await stat(resolved);
  if (!info.isFile() || info.size <= 0) throw helperError('Windows Computer Use helper must be a file.');
  const result = await verify(resolved);
  if (result.valid !== true) throw helperError('Windows Computer Use helper has no valid Authenticode signature.');
  if (!/(?:^|,\s*)O=(?:OpenAI|OpenAI LLC|OpenAI OpCo LLC|"OpenAI, (?:Inc\.|LLC|L\.L\.C\.)"|"OpenAI OpCo, LLC")(?=,|$)/i.test(result.subject ?? '')) {
    throw helperError('Windows Computer Use helper must be signed by OpenAI.');
  }
  return { path: resolved, bytes: info.size, signer: result.subject };
}
