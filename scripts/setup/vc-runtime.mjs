// Makes sure Microsoft's Visual C++ runtime is installed on Windows before MiniClaw's binaries run.
// The backend and the bundled Git link vcruntime140.dll dynamically; MiniClaw does not ship Microsoft's
// DLLs, so setup installs Microsoft's own redistributable instead. See docs/windows/vc-runtime.md.
// Hashes the download so the elevated step can re-check the same bytes.
import { createHash } from 'node:crypto';
// Stores the download privately and removes it afterwards.
import { mkdtemp, open, readFile, rm } from 'node:fs/promises';
import { mkdtempSync, rmSync } from 'node:fs';
// Finds the user's private temporary folder.
import { tmpdir } from 'node:os';
// Builds download and system paths.
import { join, win32 } from 'node:path';
// Locates the fixed signature and elevated-install scripts.
import { fileURLToPath } from 'node:url';
// Runs reg.exe, PowerShell and the approval request without a shell.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const exec = promisify(execFile);
// The MSVC toolset (14.51.36231) that built MiniClaw's Windows backend and Git. Microsoft: the installed
// redistributable "must be the same or later than the version of the MSVC Build Tools used to create
// your application". Raise this whenever the Windows build machine's toolset is updated.
export const VC_RUNTIME_MINIMUM = '14.51.36231';
export const VC_RUNTIME_PAGE = 'https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist';
// Microsoft's documented permalinks for the latest supported v14 redistributable.
const PERMALINK = { arm64: 'https://aka.ms/vc14/vc_redist.arm64.exe', x64: 'https://aka.ms/vc14/vc_redist.x64.exe' };
// aka.ms redirects to download.visualstudio.microsoft.com (observed 2026-10-04); download.microsoft.com is
// Microsoft's other documented download host. Every hop must stay on these hosts over HTTPS.
const HOSTS = new Set(['aka.ms', 'download.visualstudio.microsoft.com', 'download.microsoft.com']);
// Current installers are 12 MB (ARM64) and 25 MB (x64); anything far larger is refused.
const MAX_BYTES = 64 * 1024 ** 2;
// Exit codes the elevated script uses for its own re-checks (bit 29: application-defined).
const COPY_MISMATCH = 0x20000001, NOT_MICROSOFT = 0x20000002;
const system = (root, ...parts) => win32.join(root, 'System32', ...parts);
const systemRoot = () => process.env.SystemRoot || 'C:\\Windows';

// Names the redistributable for a MiniClaw release architecture, or null where none is needed.
export function vcRuntimeArch(platform, arch) {
  return platform === 'win32' && Object.hasOwn(PERMALINK, arch) ? arch : null;
}

// Returns Microsoft's permalink for one architecture.
export function vcRuntimeUrl(arch) {
  if (!Object.hasOwn(PERMALINK, arch)) throw new Error('No Microsoft Visual C++ runtime for this architecture');
  return PERMALINK[arch];
}

// Compares dotted versions numerically; missing parts count as 0.
export function compareVersions(left, right) {
  const a = String(left).split('.').map(Number), b = String(right).split('.').map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index++) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference) return Math.sign(difference);
  }
  return 0;
}

// Reads the Installed flag and the Major/Minor/Bld/Rbld DWORDs of one Runtimes key from reg.exe output.
export function parseRuntimeKey(text) {
  const value = name => {
    const match = new RegExp(`^\\s+${name}\\s+REG_DWORD\\s+0x([0-9a-f]+)\\s*$`, 'im').exec(String(text));
    return match ? parseInt(match[1], 16) : null;
  };
  if (value('Installed') !== 1) return null;
  const parts = ['Major', 'Minor', 'Bld', 'Rbld'].map(value);
  return parts.some(part => part === null) ? null : parts.join('.');
}

// Reports the installed runtime for an architecture. Microsoft documents the key under the 32-bit view
// (Wow6432Node); both views are read and the newer registration counts.
export async function vcRuntimeStatus({ arch, systemRoot: root = systemRoot(),
  query = async (file, args) => (await exec(file, args, { windowsHide: true, timeout: 15000, maxBuffer: 16384 })).stdout } = {}) {
  vcRuntimeUrl(arch);
  const path = `HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\${arch}`;
  const versions = await Promise.all(['/reg:32', '/reg:64'].map(view =>
    query(system(root, 'reg.exe'), ['query', path, view]).then(parseRuntimeKey, () => null)));
  const version = versions.filter(Boolean).sort(compareVersions).at(-1) ?? null;
  return { arch, version, sufficient: Boolean(version) && compareVersions(version, VC_RUNTIME_MINIMUM) >= 0 };
}

// Reads a file's Authenticode state through the fixed signature script; the path is a literal argument.
async function authenticode(path) {
  const { stdout } = await exec(system(systemRoot(), 'WindowsPowerShell', 'v1.0', 'powershell.exe'), ['-NoLogo', '-NoProfile',
    '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
    fileURLToPath(new URL('./windows-computer-signature.ps1', import.meta.url)), path],
  { windowsHide: true, timeout: 60000, maxBuffer: 8192 });
  return JSON.parse(stdout);
}

// Accepts only a valid signature by Microsoft Corporation issued by a Microsoft code-signing CA.
function requireMicrosoft(result) {
  const organization = text => /(?:^|,\s*)O=Microsoft Corporation(?=,|$)/.test(text ?? '');
  if (result?.valid !== true) throw new Error('The downloaded installer has no valid code signature');
  if (!organization(result.subject) || !/(?:^|,\s*)CN=Microsoft Corporation(?=,|$)/.test(result.subject)
    || !organization(result.issuer)) throw new Error('The downloaded installer does not carry Microsoft\'s signature');
}

// Opens one hop of Microsoft's permalink chain; any host outside Microsoft's download hosts is refused.
async function hop(fetcher, url, signal) {
  const target = new URL(url);
  if (target.protocol !== 'https:' || target.username || target.password || (target.port && target.port !== '443')
    || !HOSTS.has(target.hostname)) throw new Error(`${target.origin} is not a Microsoft download host`);
  try {
    return await fetcher(target.href, { redirect: 'manual',
      signal: AbortSignal.any([AbortSignal.timeout(10 * 60_000), ...(signal ? [signal] : [])]) });
  } catch (error) {
    if (signal?.aborted) throw new Error('Setup cancelled');
    throw new Error(`Could not reach Microsoft's download server (${error.message}). Check the internet connection.`);
  }
}

// Downloads Microsoft's installer for one architecture into a private folder, bounded in size, and
// checks Microsoft's signature before returning. Nothing is executed here.
export async function downloadVcRuntime({ arch, folder, fetcher = fetch, verifySignature = authenticode,
  maxBytes = MAX_BYTES, onProgress = () => {}, signal } = {}) {
  let url = vcRuntimeUrl(arch), response;
  for (let count = 0; ; count++) {
    if (count > 5) throw new Error('Too many redirects from Microsoft\'s download link');
    response = await hop(fetcher, url, signal);
    if (![301, 302, 303, 307, 308].includes(response.status)) break;
    const location = response.headers.get('location');
    await response.body?.cancel();
    if (!location) throw new Error('Microsoft\'s download link redirected nowhere');
    url = new URL(location, url).href;
  }
  if (!response.ok || !response.body) throw new Error(`Microsoft Visual C++ runtime download failed: HTTP ${response.status}`);
  const declared = Number(response.headers.get('content-length'));
  if (declared > maxBytes) { await response.body.cancel(); throw new Error('The Microsoft Visual C++ runtime installer is larger than expected'); }
  const installer = join(folder, `VC_redist.${arch}.exe`);
  const hash = createHash('sha256');
  let bytes = 0;
  try {
    const file = await open(installer, 'wx', 0o600);
    try {
      for await (const chunk of response.body) {
        bytes += chunk.length;
        if (bytes > maxBytes) throw new Error('The Microsoft Visual C++ runtime installer is larger than expected');
        hash.update(chunk);
        await file.write(chunk);
        onProgress(bytes, declared || null);
      }
    } finally { await file.close(); }
    const verdict = await verifySignature(installer).catch(() => ({ valid: false }));
    requireMicrosoft(verdict);
  } catch (error) {
    await rm(installer, { force: true });
    if (signal?.aborted) throw new Error('Setup cancelled');
    throw /signature|signed/.test(error.message) || /larger than expected/.test(error.message) ? error
      : new Error(`Microsoft Visual C++ runtime download failed: ${error.message}`);
  }
  return { arch, installer, sha256: hash.digest('hex'), bytes };
}

// Quotes one PowerShell literal without expanding shell syntax.
function literal(value) { return `'${value.replaceAll("'", "''")}'`; }

// Builds the approval request: an elevated PowerShell runs the fixed script with the verified path and digest.
export function vcRuntimeCommand({ installer, sha256, scriptBody, systemRoot: root = 'C:\\Windows' }) {
  for (const path of [installer, system(root)]) {
    if (typeof path !== 'string' || !/^[A-Za-z]:\\/.test(path) || /[\0\r\n"']/.test(path) || path.slice(2).includes(':')
      || path.split('\\').some(part => part === '..')) throw new Error('Expected a local absolute Windows path');
  }
  if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error('Invalid installer digest');
  if (typeof scriptBody !== 'string' || !scriptBody.trim()) throw new Error('Missing installation script');
  const inner = `$ErrorActionPreference = 'Stop'; & {\n${scriptBody}\n} -Source ${literal(installer)} -ExpectedSha256 ${literal(sha256)}`;
  const encoded = Buffer.from(inner, 'utf16le').toString('base64');
  const shell = system(root, 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const outer = `try { $p = Start-Process -FilePath ${literal(shell)} -Verb RunAs -Wait -PassThru -ArgumentList '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encoded}' } catch { exit 1223 }; exit $p.ExitCode`;
  if (outer.length > 30000) throw new Error('Runtime installer request exceeds Windows command-line limit');
  return { file: shell, args: ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', outer] };
}

// Explains a failed installation in one sentence that ends with where to get the runtime by hand.
function failure(arch, reason) {
  return new Error(`Setup could not install the Microsoft Visual C++ runtime: ${reason}. MiniClaw needs it. `
    + `Install the ${arch} "Latest supported" Visual C++ Redistributable from ${VC_RUNTIME_PAGE}, then rerun.`);
}

// Runs Microsoft's verified installer with administrator approval and checks the registration afterwards.
// `arch` is the release target (the backend's architecture); it is required, never assumed.
export async function installVcRuntime({ arch, installer, sha256, scriptBody, run, status = vcRuntimeStatus,
  systemRoot: root = systemRoot() }) {
  vcRuntimeUrl(arch);
  scriptBody ??= await readFile(fileURLToPath(new URL('./windows-vc-runtime-install.ps1', import.meta.url)), 'utf8');
  run ??= (file, args) => exec(file, args, { windowsHide: true, timeout: 15 * 60_000, maxBuffer: 16384 });
  const command = vcRuntimeCommand({ installer, sha256, scriptBody, systemRoot: root });
  let code = 0;
  try { await run(command.file, command.args); }
  catch (error) { code = Number.isInteger(error.code) ? error.code : -1; }
  const reasons = { 1223: 'administrator approval was declined', 1602: 'administrator approval was declined',
    1618: 'Another installation is in progress; wait for it to finish',
    [COPY_MISMATCH]: 'the file the administrator step read did not match the verified download',
    [NOT_MICROSOFT]: 'the file the administrator step read is not signed by Microsoft' };
  // 1638: a newer version is already installed. 3010 and 1641: installed, a restart completes it.
  if (![0, 1638, 3010, 1641].includes(code)) throw failure(arch, reasons[code] ?? `Microsoft's installer ended with exit code ${code}`);
  const after = await status({ arch });
  if (!after.sufficient) {
    throw failure(arch, `the runtime is still not installed in version ${VC_RUNTIME_MINIMUM} or later (found ${after.version ?? 'none'})`);
  }
  return { version: after.version, restart: code === 3010 || code === 1641 };
}

// Starts downloading a missing or outdated runtime without a question and returns a handle, or null when
// the runtime is sufficient or this is not Windows. `finish` returns the verified download or throws a
// message naming the official page; `cancel` stops it. The download is removed when setup exits.
export function startVcRuntime(ui, status, { platform = process.platform, download = downloadVcRuntime, temp = tmpdir() } = {}) {
  if (!vcRuntimeArch(platform, status?.arch) || status.sufficient) return null;
  const { arch } = status;
  ui.log.info(`The Microsoft Visual C++ runtime is ${status.version ? `older (${status.version}) than MiniClaw needs` : 'not installed'}. `
    + 'Setup is downloading Microsoft\'s installer (aka.ms/vc14) in the background; it is installed with administrator approval.');
  const controller = new AbortController();
  // The folder exists before the download starts, so the request is sent at once.
  const folder = mkdtempSync(join(temp, 'miniclaw-vc-runtime-'));
  const cleanup = () => rmSync(folder, { recursive: true, force: true });
  process.once('exit', cleanup);
  const outcome = (async () => download({ arch, folder, signal: controller.signal }))()
    .then(result => ({ result }), error => ({ error }));
  return {
    async cancel() { controller.abort(); await outcome; cleanup(); },
    async finish() {
      const ended = await outcome;
      if (ended.result) return ended.result;
      if (controller.signal.aborted || /cancel/i.test(ended.error.message)) throw new Error('Setup cancelled');
      throw new Error(`${failure(arch, ended.error.message).message} Nothing was changed.`);
    },
  };
}

// Ensures the runtime in the foreground (used by `miniclaw update`): nothing happens when it is present.
export async function ensureVcRuntime({ arch, temp = tmpdir(), onOutput = () => {}, status = vcRuntimeStatus,
  download = downloadVcRuntime, install = installVcRuntime } = {}) {
  const found = await status({ arch });
  if (found.sufficient) return { installed: false, version: found.version, restart: false };
  onOutput(`Installing the Microsoft Visual C++ runtime (${arch}) from Microsoft; Windows will request administrator approval.\n`);
  const folder = await mkdtemp(join(temp, 'miniclaw-vc-runtime-'));
  try {
    let fetched;
    try { fetched = await download({ arch, folder }); }
    catch (error) { throw failure(arch, error.message); }
    const result = await install({ arch, installer: fetched.installer, sha256: fetched.sha256 });
    if (result.restart) onOutput('The Microsoft Visual C++ runtime is installed; Windows recommends a restart to complete it.\n');
    return { installed: true, version: result.version, restart: result.restart };
  } finally { await rm(folder, { recursive: true, force: true }); }
}
