// Reads the installed settings and state; saves a migrated helper choice.
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
// Starts the backend and forwards signals.
import { spawn } from 'node:child_process';
// Resolves installed files independently of cwd.
import { fileURLToPath } from 'node:url';
// Validates configured executables and joins native state paths.
import path, { isAbsolute } from 'node:path';
// Tells a direct launch from an import by a test.
import { realpathSync } from 'node:fs';

// Returns the installation's state paths beside the runtime folder, in the platform's own form.
export function launchPaths(runtime, api = path) {
  const data = api.join(runtime, '..', 'data');
  return { data, chat: api.join(data, 'chat-workspace'), network: api.join(data, 'windows-network.json') };
}

// The Codex app's package folder, Name_Version_Architecture_ResourceId_PublisherId; the version changes with each update.
const codexPackage = /^OpenAI\.Codex_\d+(?:\.\d+){3}_[a-z0-9]+_[^_]*_([a-z0-9]{13})$/i;
const helperNames = ['codex-computer-use.exe', 'codex-computer-use-arm64.exe'];
const codexFamily = /^OpenAI\.Codex_[a-z0-9]{13}$/;

// Returns the saved Windows Computer Use choice in its current form. A helper path inside the Codex app's
// package (what setup saved up to 0.1.4) becomes { enabled, packageFamily, file }, so the backend finds the
// helper again after a Codex update. Any other value is returned as saved; a missing one is null.
export function windowsHelperSetting(value) {
  if (typeof value !== 'string') return value ?? null;
  const parts = value.split('\\');
  const at = parts.findIndex(part => part.toLowerCase() === 'windowsapps');
  const publisher = at >= 0 ? codexPackage.exec(parts[at + 1] ?? '')?.[1] : undefined;
  const file = parts.at(-1).toLowerCase();
  if (!publisher || parts.length < at + 3 || !helperNames.includes(file)) return value;
  return { enabled: true, packageFamily: `OpenAI.Codex_${publisher.toLowerCase()}`, file };
}

// Returns the backend environment for the helper choice: the package family and file name, an explicit
// path, or nothing. Refuses a family or file name that is not the Codex app's helper.
export function helperEnvironment(setting) {
  if (typeof setting === 'string') return { MINICLAW_HELPER_BIN: setting };
  if (setting?.enabled !== true) return {};
  if (!codexFamily.test(setting.packageFamily ?? '') || !helperNames.includes(setting.file)) {
    throw new Error('The saved Windows Computer Use helper is not the Codex app\'s helper. Run miniclaw setup again.');
  }
  return { MINICLAW_HELPER_PACKAGE: setting.packageFamily, MINICLAW_HELPER_FILE: setting.file };
}

// Reads settings.json and returns it with the helper choice in its current form. A migrated choice is
// saved back through a temporary file and a rename, so an interrupted write leaves the old file whole;
// if saving fails, this start still uses the migrated value.
export async function readSettings(file) {
  const config = JSON.parse(await readFile(file, 'utf8'));
  const helper = windowsHelperSetting(config.windowsComputerHelper);
  if (helper === config.windowsComputerHelper || helper === null) return config;
  const migrated = { ...config, windowsComputerHelper: helper };
  const staging = `${file}.tmp`;
  try {
    await writeFile(staging, `${JSON.stringify(migrated, null, 2)}\n`, { mode: 0o600 });
    await rename(staging, file);
  } catch (error) {
    console.error(`Could not save the updated Computer Use helper setting: ${error.message}`);
  }
  return migrated;
}

// Launches only an explicitly completed Host installation; never substitutes for a VM.
async function main() {
  if (process.argv.length !== 2) throw new Error('This launcher takes no arguments');
  const config = await readSettings(fileURLToPath(new URL('../settings.json', import.meta.url)));
  if (config.schemaVersion !== 1 || config.backend !== 'host' || config.hostReady !== true
    || !Number.isInteger(config.port) || config.port < 1024 || config.port > 65535
    || [config.codexBin, config.claudeBin, ...(typeof config.windowsComputerHelper === 'object' ? [] : [config.windowsComputerHelper])]
      .some(path => path != null && (typeof path !== 'string' || !isAbsolute(path)))
    || !(config.codexBin || config.claudeBin)) {
    throw new Error(`A completed ${process.platform === 'win32' ? '' : 'Mac '}installation with at least one provider is required`);
  }
  const { data, chat, network } = launchPaths(fileURLToPath(new URL('./', import.meta.url)));
  if (process.platform !== 'win32') await mkdir(data, { recursive: true, mode: 0o700 });
  // Opt-in VMs: the session starts no VM here; it only opens the private links a VM
  // task later uses. Without `pair.mac` the backend runs alone.
  let session;
  if (config.pair?.mac && process.platform === 'win32') throw new Error('VMs are unavailable on Windows');
  if (config.pair?.mac) {
    const { startPair, reportShutdown } = await import('./vm/session.mjs');
    session = await startPair(config.pair, { limits: config.limits, onStatus: reportShutdown });
  }
  const child = spawn(fileURLToPath(new URL(`./bin/miniclaw-server${process.platform === 'win32' ? '.exe' : ''}`, import.meta.url)), [], {
    cwd: data, stdio: process.platform === 'win32' ? ['pipe', 'inherit', 'inherit'] : 'inherit', env: { ...process.env,
      MINICLAW_PORT: String(config.port), MINICLAW_DATA_DIR: data,
      MINICLAW_CHAT_DIR: chat,
      ...(process.platform === 'win32' ? { MINICLAW_NETWORK_CONFIG: network, MINICLAW_LAUNCHER_STDIN: '1' } : {}),
      // An empty value disables a provider the wizard did not wire in.
      CODEX_BIN: config.codexBin ?? '', CLAUDE_BIN: config.claudeBin ?? '',
      ...helperEnvironment(config.windowsComputerHelper),
      ...(session ? { MINICLAW_VM_LINKS: session.linksPath } : {}),
      MINICLAW_WEB_DIR: fileURLToPath(new URL('./web/', import.meta.url)) },
  });
  // Host process shutdown is forwarded; VM lifecycle belongs to its own supervisor.
  const stop = () => process.platform === 'win32' ? child.stdin?.end() : child.kill('SIGTERM');
  child.stdin?.on('error', () => {});
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, stop);
  try {
    await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', code => { process.exitCode = code ?? 1; resolve(); });
    });
  } finally {
    // The backend stops first; then running VMs get their bounded shutdown.
    const code = await session?.close();
    if (code) process.exitCode = code;
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.off(signal, stop);
  }
}

// Node resolves the entry script's links, so the real path identifies a direct launch.
function launchedDirectly() {
  try { return realpathSync(process.argv[1]) === fileURLToPath(import.meta.url); }
  catch { return false; }
}

if (launchedDirectly()) main().catch(error => { console.error(error.message); process.exitCode = 1; });
