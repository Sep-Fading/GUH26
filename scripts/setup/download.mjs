// Hashes completed images without buffering them.
import { createHash } from 'node:crypto';
// Opens files without following symbolic links.
import { constants, createReadStream } from 'node:fs';
// Publishes verified files without overwriting destinations.
import { link, lstat, open, unlink } from 'node:fs/promises';
// Resolves validated filenames inside private caches.
import { join, win32 } from 'node:path';
// Checks Windows permissions with a fixed script.
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

// Verifies a Windows cache belongs to this user and has no other account access.
async function windowsPrivateDirectory(path) {
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
  await promisify(execFile)(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', fileURLToPath(new URL('./windows-private-cache.ps1', import.meta.url)), path],
  { windowsHide: true, timeout: 15000, maxBuffer: 8192 });
}

// Enforces the platform's real directory protection before creating download files.
export async function validateCacheDirectory(path, { platform = process.platform, verifyWindows = windowsPrivateDirectory } = {}) {
  const dir = await lstat(path);
  if (!dir.isDirectory() || dir.isSymbolicLink()) throw new Error('Download cache must be a private directory');
  if (platform === 'win32') await verifyWindows(path);
  else if ((dir.mode & 0o077) !== 0) throw new Error('Download cache must be a private directory');
}

// Keeps root-to-leaf native directory handles alive until the caller completes its operation.
export async function acquireCacheDirectory(path, { platform = process.platform } = {}) {
  if (platform !== 'win32') {
    await validateCacheDirectory(path, { platform });
    return { close: async () => {} };
  }
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
  const child = spawn(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', fileURLToPath(new URL('./windows-private-cache.ps1', import.meta.url)), path, '-Hold'],
  { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const controller = new AbortController();
  let closing = false, diagnostics = '', output = '';
  child.stdin.on('error', error => { if (!closing) controller.abort(error); });
  child.stderr.on('data', bytes => { diagnostics = (diagnostics + bytes).slice(-8192); });
  const ended = new Promise(resolve => child.once('close', resolve));
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(new Error('Cache protection timed out')); }, 15000);
      const fail = error => { clearTimeout(timer); reject(error); };
      child.once('error', fail);
      child.once('close', () => fail(new Error(diagnostics || 'Cache protection ended before readiness')));
      child.stdout.on('data', bytes => {
        output += bytes;
        if (output === 'ready\r\n' || output === 'ready\n') { clearTimeout(timer); resolve(); }
        else if (output.length > 128) fail(new Error('Unexpected cache protection response'));
      });
    });
  } catch (error) { child.kill(); await ended; throw error; }
  child.on('close', () => { if (!closing) controller.abort(new Error('Cache protection process exited')); });
  return { signal: controller.signal, close: async () => {
    closing = true;
    child.stdin.end();
    const timer = setTimeout(() => child.kill(), 5000);
    try { await ended; } finally { clearTimeout(timer); }
  } };
}

// Restricts original requests and redirects to the approved HTTPS image hosts.
function source(value) {
  const url = new URL(value);
  const allowed = ['cloud.debian.org', 'updates.cdn-apple.com', 'releases.openai.com', 'go.dev'];
  const goArchive = url.hostname === 'dl.google.com' && url.pathname.startsWith('/go/');
  // The official Node build a release carries; only its own download folder is accepted.
  const nodeArchive = url.hostname === 'nodejs.org' && url.pathname.startsWith('/dist/v');
  // Setup never downloads Git: no GitHub host is approved here. Packaging fetches MiniClaw's own
  // release assets only through downloadReleaseAsset and its narrower rules below.
  if (url.protocol !== 'https:' || url.username || url.password || (url.port && url.port !== '443')
    || !(allowed.includes(url.hostname) || goArchive || nodeArchive || url.hostname.endsWith('.ftp.acc.umu.se'))) {
    throw new Error('Unapproved download host');
  }
  return url;
}

// The only GitHub repository whose release assets packaging may fetch, and GitHub's asset storage
// hosts its downloads redirect to (release-assets.* today, objects.* before 2025).
const releaseRepositories = ['Sep-Fading/miniclaw-windows'];
const releaseStorage = ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'];

// Accepts this asset's own release download or API asset URL, or a storage redirect; nothing else.
function releaseSource(value, { repository, tag, asset }, file) {
  const url = new URL(value);
  const own = url.hostname === 'github.com' && url.pathname === `/${repository}/releases/download/${tag}/${file}` && !url.search;
  const api = url.hostname === 'api.github.com' && url.pathname === `/repos/${repository}/releases/assets/${asset}` && !url.search;
  const storage = releaseStorage.includes(url.hostname) && url.pathname.startsWith('/github-production-release-asset');
  if (url.protocol !== 'https:' || url.username || url.password || (url.port && url.port !== '443')
    || !releaseRepositories.includes(repository) || !(own || api || storage)) throw new Error('Unapproved download host');
  return url;
}

// Downloads one pinned asset of MiniClaw's own GitHub release into memory and returns the verified bytes.
// With GH_TOKEN or GITHUB_TOKEN it uses the API asset endpoint (a private repository's only route) and sends
// the token there alone; without one it tries the public download URL. Size and SHA-256 decide.
export async function downloadReleaseAsset(pin, { fetcher = globalThis.fetch, env = process.env,
  signal = AbortSignal.timeout(600000) } = {}) {
  const { release, file } = pin;
  if (!release || !/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(file ?? '') || !/^v\d+\.\d+\.\d+$/.test(release.tag ?? '')
    || !Number.isSafeInteger(release.asset) || release.asset <= 0 || (pin.algorithm ?? 'sha256') !== 'sha256'
    || !/^[a-f0-9]{64}$/.test(pin.digest ?? '') || !Number.isSafeInteger(pin.bytes) || pin.bytes <= 0) {
    throw new Error('Invalid release asset descriptor');
  }
  const token = env.GH_TOKEN || env.GITHUB_TOKEN;
  let url = releaseSource(token ? `https://api.github.com/repos/${release.repository}/releases/assets/${release.asset}`
    : `https://github.com/${release.repository}/releases/download/${release.tag}/${file}`, release, file);
  let response;
  for (let count = 0; ; count++) {
    if (count === 6) throw new Error('Too many download redirects');
    const headers = url.hostname === 'api.github.com' && token ? { Authorization: `Bearer ${token}`,
      Accept: 'application/octet-stream', 'X-GitHub-Api-Version': '2022-11-28' } : {};
    response = await fetcher(url, { headers, redirect: 'manual', signal });
    if (![301, 302, 303, 307, 308].includes(response.status)) break;
    const location = response.headers.get('location');
    await response.body?.cancel();
    if (!location) throw new Error('Download redirect has no destination');
    url = releaseSource(new URL(location, url), release, file);
  }
  if (response.status !== 200 || !response.body) {
    await response.body?.cancel().catch(() => {});
    throw new Error(`${pin.name} download from ${release.repository} ${release.tag} failed: HTTP ${response.status}`
      + (response.status === 404 && !token ? ' (the repository is private: set GH_TOKEN or GITHUB_TOKEN)' : ''));
  }
  const chunks = [], hash = createHash('sha256');
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > pin.bytes) throw new Error(`${pin.name} download exceeds its pinned size`);
    hash.update(chunk);
    chunks.push(chunk);
  }
  if (size !== pin.bytes || hash.digest('hex') !== pin.digest) throw new Error(`${pin.name} checksum mismatch`);
  return Buffer.concat(chunks);
}

// Returns file metadata or null; refuses links, special files and hard links.
async function regular(path) {
  const info = await lstat(path).catch(error => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (info && (!info.isFile() || info.nlink !== 1)) throw new Error('Expected a regular private download file');
  return info;
}

// Streams a local file through the expected checksum algorithm.
export async function verifyDownload(path, image, signal) {
  const info = await regular(path);
  if (!info || info.size !== image.bytes) throw new Error('Downloaded image size mismatch');
  const hash = createHash(image.algorithm);
  const input = createReadStream(path, { flags: constants.O_RDONLY | constants.O_NOFOLLOW });
  for await (const chunk of input) {
    signal?.throwIfAborted();
    hash.update(chunk);
  }
  if (hash.digest('hex') !== image.digest) throw new Error('Downloaded image checksum mismatch');
}

// Follows bounded vendor redirects without accepting insecure or local destinations.
async function request(url, headers, fetcher, signal) {
  for (let count = 0; count < 6; count++) {
    const response = await fetcher(source(url), { headers, redirect: 'manual', signal });
    if (![301, 302, 303, 307, 308].includes(response.status)) return response;
    const location = response.headers.get('location');
    await response.body?.cancel();
    if (!location) throw new Error('Download redirect has no destination');
    url = new URL(location, url);
  }
  throw new Error('Too many download redirects');
}

// Resumes into a private partial file, then publishes it only after size/hash checks.
async function downloadPinned(image, directory, {
  fetcher = fetch, signal, onProgress = () => {},
} = {}) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(image.file)
    || !['sha256', 'sha512'].includes(image.algorithm)
    || !new RegExp(`^[a-f0-9]{${image.algorithm === 'sha256' ? 64 : 128}}$`).test(image.digest)
    || !Number.isSafeInteger(image.bytes) || image.bytes <= 0) throw new Error('Invalid image descriptor');
  source(image.url);
  signal?.throwIfAborted();
  const final = join(directory, image.file);
  const partial = `${final}.part`;
  const lock = await open(`${final}.lock`, 'wx', 0o600);
  let handle;
  try {
    if (await regular(final)) {
      onProgress({ received: image.bytes, total: image.bytes, verified: false, phase: 'verify' });
      await verifyDownload(final, image, signal);
      onProgress({ received: image.bytes, total: image.bytes, verified: true });
      return { path: final, cached: true };
    }
    const previous = await regular(partial);
    let received = previous?.size ?? 0;
    if (received > image.bytes) throw new Error('Partial image size exceeds expected size');
    handle = await open(partial, constants.O_WRONLY | constants.O_NOFOLLOW
      | (previous ? 0 : constants.O_CREAT | constants.O_EXCL), 0o600);
    const opened = await handle.stat();
    if (!opened.isFile() || opened.nlink !== 1 || previous && (opened.dev !== previous.dev || opened.ino !== previous.ino)) {
      throw new Error('Download partial file changed while opening');
    }
    if (received < image.bytes) {
      const headers = { 'Accept-Encoding': 'identity', ...(received ? { Range: `bytes=${received}-` } : {}) };
      const response = await request(image.url, headers, fetcher, signal);
      try {
        if (response.status === 206) {
          if (response.headers.get('content-range') !== `bytes ${received}-${image.bytes - 1}/${image.bytes}`) {
            throw new Error('Download range does not match partial file');
          }
        } else if (response.status === 200) {
          await handle.truncate(0);
          received = 0;
        } else throw new Error(`Download failed: HTTP ${response.status}`);
        if (!response.body) throw new Error('Download has no body');
        onProgress({ received, total: image.bytes, verified: false });
        for await (const chunk of response.body) {
          signal?.throwIfAborted();
          if (received + chunk.length > image.bytes) throw new Error('Download exceeds expected size');
          // Append-only Windows handles cannot truncate when a server ignores Range.
          // Explicit offsets keep both resumed and restarted writes on this checked handle.
          let offset = 0;
          while (offset < chunk.length) {
            const { bytesWritten } = await handle.write(chunk, offset, chunk.length - offset, received);
            if (!bytesWritten) throw new Error('Download write made no progress');
            offset += bytesWritten;
            received += bytesWritten;
          }
          onProgress({ received, total: image.bytes, verified: false });
        }
      } finally {
        if (!response.body?.locked) await response.body?.cancel().catch(() => {});
      }
    }
    signal?.throwIfAborted();
    await handle.sync();
    await handle.close();
    handle = undefined;
    onProgress({ received: image.bytes, total: image.bytes, verified: false, phase: 'verify' });
    try { await verifyDownload(partial, image, signal); }
    catch (error) {
      if (/checksum/.test(error.message)) await unlink(partial);
      throw error;
    }
    // A competing destination is never overwritten, even after verification.
    await link(partial, final);
    await unlink(partial);
    onProgress({ received: image.bytes, total: image.bytes, verified: true });
    return { path: final, cached: false };
  } finally {
    await handle?.close();
    await lock.close();
    await unlink(`${final}.lock`);
  }
}

// Owns native cache pins throughout verification and publication, including error cleanup.
export async function downloadImage(image, directory, { acquireDirectory = acquireCacheDirectory, ...options } = {}) {
  const guard = await acquireDirectory(directory);
  try {
    const signals = [options.signal, guard.signal].filter(Boolean);
    return await downloadPinned(image, directory, { ...options,
      signal: signals.length ? AbortSignal.any(signals) : undefined });
  } finally { await guard.close(); }
}

// Reports which items already sit in the cache with their exact pinned size, so the
// wizard can say what is left to fetch. The checksum is still verified at install time.
export async function cachedItems(items, directory) {
  const found = new Set();
  for (const item of items) {
    const info = await lstat(join(directory, item.file)).catch(() => null);
    if (info?.isFile() && info.size === item.bytes) found.add(item.id);
  }
  return found;
}
