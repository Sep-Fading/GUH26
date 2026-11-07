// Installs a missing Claude Code in the background with Anthropic's own installer, on Windows and macOS.
// Anthropic's binary is not shipped in the MiniClaw archive; see docs/windows/claude-install.md.
// Hashes the download against Anthropic's manifest.
import { createHash } from 'node:crypto';
// Stores the download and checks the launcher.
import { chmod, mkdir, open, rm, stat } from 'node:fs/promises';
// Finds the user's profile folder.
import { homedir } from 'node:os';
// Builds download and launcher paths.
import { join } from 'node:path';
// Locates the fixed signature-check script.
import { fileURLToPath } from 'node:url';
// Runs the installer and checks without a shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const exec = promisify(execFile);
// The same origin and layout that https://claude.ai/install.ps1 and install.sh use.
const RELEASES = 'https://downloads.claude.ai/claude-code-releases';
const MANUAL = { win32: 'in PowerShell:  irm https://claude.ai/install.ps1 | iex',
  darwin: 'in Terminal:  curl -fsSL https://claude.ai/install.sh | bash' };
// Anthropic PBC's Apple Developer ID team, as shown by codesign on the official build.
const APPLE_TEAM = 'Q6L2SF6YDW';
// A sanity bound; current builds are about 250 MB.
const MAX_BYTES = 1024 ** 3;
const system = name => join(process.env.SystemRoot || 'C:\\Windows', 'System32', name);

// Reports whether setup can install Claude Code by itself on this system.
export function claudeInstallable(platform = process.platform, arch = process.arch, env = {}) {
  try { return Boolean(claudePlatform(platform, arch, env)); } catch { return false; }
}

// Names Anthropic's build for a MiniClaw release target; ARM64 Windows gets the native ARM64 build.
export function claudePlatform(platform, arch, env = {}) {
  if (platform === 'win32' && (arch === 'arm64' || env.PROCESSOR_ARCHITECTURE === 'ARM64')) return 'win32-arm64';
  if (platform === 'win32' && arch === 'x64') return 'win32-x64';
  if (platform === 'darwin' && arch === 'arm64') return 'darwin-arm64';
  throw new Error('MiniClaw cannot install Claude Code for this system and architecture');
}

// Tells a terminal raised by "Run as administrator" (or SYSTEM) from the user's normal token.
// With User Account Control off, high integrity is an administrator's only token, so it is allowed.
export function administratorTerminal(groups, policy) {
  if (/S-1-16-16384\b/.test(groups)) return true;
  return /S-1-16-12288\b/.test(groups) && !/EnableLUA\s+REG_DWORD\s+0x0\b/.test(policy);
}

// Reports whether setup runs above the user's normal rights: raised on Windows, root on macOS.
async function runningElevated() {
  if (process.platform !== 'win32') return process.getuid() === 0;
  const options = { windowsHide: true, timeout: 15000 };
  const { stdout } = await exec(system('whoami.exe'), ['/groups'], options);
  const policy = await exec(system('reg.exe'), ['query',
    'HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System', '/v', 'EnableLUA'], options)
    .then(result => result.stdout, () => '');
  return administratorTerminal(stdout, policy);
}

// Checks a Mac binary against Apple's Developer ID requirement and reads its signing team.
async function developerId(path) {
  const requirement = 'anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists'
    + ` and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "${APPLE_TEAM}"`;
  await exec('/usr/bin/codesign', ['--verify', '--strict', `-R=${requirement}`, path], { timeout: 120000 });
  const { stderr } = await exec('/usr/bin/codesign', ['-dv', '--verbose=4', path], { timeout: 60000 });
  return { valid: true, team: /^TeamIdentifier=(\S+)$/m.exec(stderr)?.[1] };
}

// Reads a file's Authenticode state through a fixed script; the path is a literal argument.
async function authenticode(path) {
  const { stdout } = await exec(system('WindowsPowerShell/v1.0/powershell.exe'), ['-NoLogo', '-NoProfile',
    '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
    fileURLToPath(new URL('./windows-computer-signature.ps1', import.meta.url)), path],
  { windowsHide: true, timeout: 60000, maxBuffer: 8192 });
  return JSON.parse(stdout);
}

// Runs Anthropic's installer as the current user with no console input; cancellation ends it.
async function runOfficialInstaller(path, args, { signal } = {}) {
  const running = exec(path, args, { windowsHide: true, timeout: 15 * 60_000, maxBuffer: 1024 * 1024, signal });
  running.child.stdin?.end();
  await running;
}

// Asks the installed launcher for its version line.
async function versionLine(path) {
  return (await exec(path, ['--version'], { windowsHide: true, timeout: 60000, maxBuffer: 8192 })).stdout;
}

// Refuses a file the operating system does not accept as validly signed by Anthropic.
async function requireAnthropic(path, verify) {
  const result = await verify(path).catch(() => ({ valid: false }));
  if (result?.valid !== true) throw new Error('Claude Code has no valid code signature');
  if (result.team !== APPLE_TEAM && !/(?:^|,\s*)O="Anthropic, PBC"(?=,|$)/.test(result.subject ?? '')) {
    throw new Error('Claude Code is not signed by Anthropic');
  }
}

// Fetches one HTTPS resource from Anthropic's download host; redirects elsewhere are errors.
async function get(fetcher, url, signal, deadline) {
  let response;
  try {
    response = await fetcher(url, { redirect: 'error',
      signal: AbortSignal.any([AbortSignal.timeout(deadline), ...(signal ? [signal] : [])]) });
  } catch (error) {
    if (signal?.aborted) throw new Error('Setup cancelled');
    throw new Error(`Could not reach downloads.claude.ai (${error.message}). Check the internet connection.`);
  }
  if (!response.ok || !response.body) throw new Error(`Claude Code download failed: HTTP ${response.status}`);
  return response;
}

// Streams the build to disk, bounded by the manifest size, and returns its SHA-256.
async function save(response, path, size, onProgress) {
  const file = await open(path, 'w', 0o600);
  const hash = createHash('sha256');
  let received = 0;
  try {
    for await (const chunk of response.body) {
      received += chunk.length;
      if (received > size) throw new Error('Claude Code download exceeds the manifest size');
      hash.update(chunk);
      await file.write(chunk);
      onProgress(received, size);
    }
  } finally { await file.close(); }
  if (received !== size) throw new Error('Claude Code download does not match the manifest size');
  return hash.digest('hex');
}

// Downloads Anthropic's build for this architecture, verifies it, and runs its own installer
// for the current user. Returns the launcher path and version. Never signs in.
export async function installClaude(options = {}) {
  try { return await installVerified(options); }
  catch (error) { throw options.signal?.aborted ? new Error('Setup cancelled') : error; }
}

// Performs the installation; the caller turns any failure after an abort into a cancellation.
async function installVerified({ platform = process.platform, arch = process.arch, env = process.env,
  home = homedir(), target = 'latest', fetcher = fetch,
  verifySignature = platform === 'win32' ? authenticode : developerId,
  runInstaller = runOfficialInstaller, version = versionLine, elevated = runningElevated,
  onStatus = () => {}, onProgress = () => {}, signal } = {}) {
  const windows = platform === 'win32';
  const build = claudePlatform(platform, arch, env);
  if (!/^(stable|latest)$/.test(target)) throw new Error('Unknown Claude Code release channel');
  if (await elevated()) {
    throw new Error(`Claude Code installs for one user. Run setup ${windows
      ? 'from a normal terminal, not as administrator' : 'as yourself, without sudo'}.`);
  }
  const pointer = (await (await get(fetcher, `${RELEASES}/latest`, signal, 30000)).text()).trim();
  if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(pointer)) {
    throw new Error('downloads.claude.ai did not return a Claude Code version');
  }
  const manifest = await (await get(fetcher, `${RELEASES}/${pointer}/manifest.json`, signal, 30000)).json()
    .catch(() => null);
  const entry = manifest?.platforms?.[build];
  if (!/^[a-f0-9]{64}$/.test(entry?.checksum ?? '') || !Number.isSafeInteger(entry.size)
    || entry.size <= 0 || entry.size > MAX_BYTES) {
    throw new Error(`Claude Code manifest has no usable ${build} entry`);
  }
  const folder = join(home, '.claude', 'downloads');
  const download = join(folder, `claude-${pointer}-${build}${windows ? '.exe' : ''}`);
  await mkdir(folder, { recursive: true });
  try {
    onStatus(`Downloading Claude Code ${pointer} (${Math.round(entry.size / 1e6)} MB) from downloads.claude.ai`);
    const response = await get(fetcher, `${RELEASES}/${pointer}/${build}/${windows ? 'claude.exe' : 'claude'}`, signal, 30 * 60_000);
    if (await save(response, download, entry.size, onProgress) !== entry.checksum) {
      throw new Error('Claude Code download checksum mismatch');
    }
    if (!windows) await chmod(download, 0o700);
    onStatus('Checking Anthropic\'s signature');
    await requireAnthropic(download, verifySignature);
    onStatus(`Installing Claude Code ${pointer} for this user`);
    signal?.throwIfAborted();
    try { await runInstaller(download, ['install', target], { signal }); }
    catch (error) {
      throw new Error(`Claude Code installer failed: ${String(error.stderr || error.message).trim().slice(-400)}`);
    }
  } finally { await rm(download, { force: true }).catch(() => {}); }
  const launcher = join(home, '.local', 'bin', windows ? 'claude.exe' : 'claude');
  if (!await stat(launcher).then(info => info.isFile(), () => false)) {
    throw new Error(`The installer finished but ${windows ? 'claude.exe' : 'claude'} was not found at ${launcher}`);
  }
  await requireAnthropic(launcher, verifySignature);
  const installed = /^(\d+\.\d+\.\d+\S*) \(Claude Code\)/.exec((await version(launcher).catch(() => '')).trim())?.[1];
  if (!installed) throw new Error('The installed Claude Code did not report its version');
  return { path: launcher, version: installed };
}

// Starts the installation of a missing Claude Code without a question and returns a handle, or
// null when Claude Code exists or this system is unsupported. `finish` waits, shows progress and
// returns the providers found afterwards; a failure returns the given providers. `cancel` stops it.
export function startClaude(ui, providers, { platform = process.platform, arch = process.arch,
  install = installClaude, detect } = {}) {
  if (!claudeInstallable(platform, arch) || providers.some(provider => provider.id === 'claude')) return null;
  ui.log.info('Claude Code is not installed. Setup is downloading it from Anthropic (downloads.claude.ai, '
    + 'about 250 MB) and installing it for this user in the background.');
  const controller = new AbortController();
  let status = 'Asking downloads.claude.ai for the current Claude Code', stage = status;
  let show = () => {};
  const update = text => { if (text !== status) { status = text; show(text); } };
  const outcome = (async () => install({ platform, arch, signal: controller.signal,
    onStatus: text => { stage = text; update(text); },
    onProgress: (received, total) => update(`${stage}: ${Math.floor(received / total * 100)}%`) }))()
    .then(result => ({ result }), error => ({ error }));
  let settled = false;
  outcome.then(() => { settled = true; });
  const abort = () => controller.abort();
  return {
    async cancel() { abort(); await outcome; },
    async finish() {
      const progress = settled ? null : ui.spinner();
      progress?.start(status);
      show = text => progress?.message(text);
      process.on('SIGINT', abort); process.on('SIGTERM', abort);
      let ended;
      try { ended = await outcome; }
      finally { show = () => {}; process.off('SIGINT', abort); process.off('SIGTERM', abort); }
      const report = text => progress ? progress.stop(text) : ui.log.success(text);
      if (ended.result) {
        report(`Claude Code ${ended.result.version} installed at ${ended.result.path}. Sign in later from MiniClaw.`);
        return detect();
      }
      if (controller.signal.aborted || /cancel/i.test(ended.error.message)) {
        progress?.stop('Claude Code installation stopped');
        throw new Error('Setup cancelled');
      }
      progress?.stop('Claude Code was not installed');
      ui.log.warn(`${ended.error.message}\nTo install it yourself, run ${MANUAL[platform]}\n`
        + (providers.length ? 'Setup continues without Claude Code.' : 'Then rerun setup.'));
      return providers;
    },
  };
}
