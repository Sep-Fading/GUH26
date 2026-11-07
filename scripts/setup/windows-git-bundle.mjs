// Carries MiniClaw's own Git for Windows build inside a Windows release, like BusyBox and ConPTY.
// The build is Git for Windows v2.56.0.windows.1 with three sandbox patches. Setup never downloads Git;
// packaging takes the ZIP from MINICLAW_GIT_ARCHIVE or else from MiniClaw's own v0.1.5 GitHub release.
// Hashes candidate archives before use.
import { createHash } from 'node:crypto';
// Reads the bundled archive and replaces the installed record.
import { mkdir, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
// Unpacks the downloaded corresponding-source archive with the system tar.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
// Resolves runtime and data paths.
import { join, resolve, sep } from 'node:path';
// Loads bounded offline bytes; a pin without a URL is never downloaded.
import { readPinned } from './pinned-bytes.mjs';
// Fetches MiniClaw's own pinned release assets for packaging.
import { downloadReleaseAsset } from './download.mjs';
// Decodes the archive strictly and installs it through the private runtime writer.
import { gitArchiveEntries, installWindowsGit } from './windows-git.mjs';
// Saves the earlier record with the same protected permissions as the live one.
import { writePrivateFile } from './private-file.mjs';

// Pins each architecture's rebuilt runtime ZIP (git-build-provenance.md in the Windows evidence folder).
// Built on 2026-10-04 from source archive 66e379c9... (both patches after the independent review's L1 fix).
// Packaged without Microsoft's Visual C++ runtime DLLs; setup installs Microsoft's redistributable instead
// (vc-runtime.mjs, docs/windows/vc-runtime.md). lpac4 (2026-10-05): the same Git build with OpenSSL rebuilt
// with _WIN32_WINNT=0x0A00, so libcrypto seeds from BCryptGenRandom inside the sandbox (docs/windows/git-runtime.md).
// lpac5 (2026-10-05): Git recompiled from source v4 d84d5146... with the third patch, git-lpac-console-handle.patch
// (Git no longer exits 0xC0000008 when its stdout is the sandboxed terminal).
// The same bytes are assets of MiniClaw's v0.1.5 GitHub release (asset ids below); a clean clone packages from there.
const pins = {
  arm64: { series: 'lpac5', bytes: 29526262, digest: 'ca7c05a029d02682bb90b2b8eeb9adde157bfa3c5e389b53ed51e54201fc19a3', asset: 616891706 },
  x64: { series: 'lpac5', bytes: 29632338, digest: '66f3c549d5a5d995c562efdf4c722a468006e683d0c15e0d3a36f1f744f30d1c', asset: 616891738 },
};
// MiniClaw's GitHub release that publishes the pinned Git ZIPs and their corresponding source.
const gitRelease = { repository: 'Sep-Fading/miniclaw-windows', tag: 'v0.1.5' };

// The complete corresponding source for both pinned ZIPs (its SOURCE-MANIFEST.json describes both), as published
// beside release 0.1.5. Packaging unpacks it when MINICLAW_GIT_SOURCE is unset.
export const gitSourceRelease = Object.freeze({ name: 'Git corresponding source',
  file: 'miniclaw-0.1.5-6457c45-windows-arm64-git-corresponding-source.tar.gz', bytes: 154469901, algorithm: 'sha256',
  digest: 'b3e5fb5bafbcbb459f77d4a487cb0711cd260319bcb2946c294354e04cfdfea0', release: { ...gitRelease, asset: 616891793 } });

// Returns the pinned bundled Git runtime for one Windows architecture.
export function windowsGitBundle(arch) {
  if (!Object.hasOwn(pins, arch)) throw new Error('Unsupported Git architecture');
  const pin = pins[arch];
  return { id: 'windows-git', name: 'Git 2.56.0 (MiniClaw build) for workspace commands',
    file: `miniclaw-git-2.56.0-${pin.series}-${arch}.zip`, runtimePrefix: 'git', algorithm: 'sha256',
    bytes: pin.bytes, digest: pin.digest, expandedBytes: 128 * 1024 ** 2, release: { ...gitRelease, asset: pin.asset } };
}

// Where a bundle (and an installed runtime) keeps the archive and its licence texts.
export const gitBundleFolder = 'bin/git';

// Reads the configured archive (or, without one, fetches the pinned release asset), verifies its pin before
// decoding, and returns the archive plus its licence, patch, notice and written-offer texts.
export async function gitBundleFiles({ arch, archive = process.env.MINICLAW_GIT_ARCHIVE, bytes, pin = windowsGitBundle(arch),
  fetcher, env }) {
  if (!Number.isSafeInteger(pin.bytes) || pin.bytes <= 0 || !/^[a-f0-9]{64}$/.test(pin.digest)) {
    throw new Error(`The ${arch} Git runtime is not pinned yet`);
  }
  if (!bytes && !archive) {
    try { bytes = await downloadReleaseAsset({ ...pin, name: `Git runtime ${pin.file}` }, { fetcher, env }); } catch (error) {
      throw new Error(`${error.message}. Or set MINICLAW_GIT_ARCHIVE to a local ${pin.file}; Windows packages never ship without Git.`);
    }
  }
  // The pin carries no URL, so readPinned can only read the offline file or the verified bytes.
  const content = await readPinned({ name: `Git runtime ${pin.file}`, bytes: pin.bytes, digest: pin.digest },
    { archive, bytes });
  const entries = gitArchiveEntries(content), prefix = `${pin.runtimePrefix}/licenses/`;
  const files = new Map([[pin.file, content]]);
  for (const [name, data] of entries) {
    if (name.startsWith(prefix) && !name.slice(prefix.length).includes('/')) files.set(`licenses/${name.slice(prefix.length)}`, data);
  }
  for (const required of ['git-COPYING.txt', 'NOTICE.txt', 'SOURCE-OFFER.txt', 'git-lpac-path.patch', 'git-lpac-leading-directories.patch',
    'git-lpac-console-handle.patch']) {
    if (!files.get(`licenses/${required}`)?.length) throw new Error(`Git runtime lacks licenses/${required}`);
  }
  for (const required of [`${pin.runtimePrefix}/bin/git.exe`, `${pin.runtimePrefix}/etc/ssl/certs/ca-bundle.crt`]) {
    if (!entries.get(required)?.length) throw new Error(`Git runtime lacks ${required}`);
  }
  return files;
}

// Returns the Git corresponding-source folder for a release build: MINICLAW_GIT_SOURCE when set, else the pinned
// release archive downloaded, verified and unpacked below `directory` (the caller's temporary work folder).
export async function gitCorrespondingSource({ folder = process.env.MINICLAW_GIT_SOURCE, directory, tar, fetcher, env,
  pin = gitSourceRelease }) {
  if (folder) return folder;
  let bytes;
  try { bytes = await downloadReleaseAsset(pin, { fetcher, env }); } catch (error) {
    throw new Error(`${error.message}. Or set MINICLAW_GIT_SOURCE to the unpacked git-corresponding-source folder.`);
  }
  const archive = join(directory, pin.file), target = join(directory, 'git-source');
  await writeFile(archive, bytes, { flag: 'wx' });
  await mkdir(target);
  await promisify(execFile)(tar, ['-xzf', archive, '-C', target], { windowsHide: true, maxBuffer: 1024 ** 2 });
  await rm(archive, { force: true });
  return join(target, 'git-corresponding-source');
}

// Names the bundled archive inside an installed or freshly built runtime folder.
export function bundledGitArchive(runtime, arch) {
  return join(runtime, gitBundleFolder, windowsGitBundle(arch).file);
}

// Accepts only a previous runtime that setup generated: data/windows-git-runtime/<UUID>.
function generatedRoot(data, root) {
  const parent = resolve(data, 'windows-git-runtime');
  const path = typeof root === 'string' ? resolve(root) : '';
  const name = path.slice(parent.length + 1);
  return path.startsWith(parent + sep) && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(name) ? path : null;
}

// Reads a record's generated runtime root: undefined when the file is absent, null when it names none.
async function recordedRoot(data, path) {
  let text;
  try { text = await readFile(path, 'utf8'); } catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
  try { return generatedRoot(data, JSON.parse(text).root); } catch { return null; }
}

// Brings an interrupted update to a consistent state before anything else changes Git.
// The live record always wins: a leftover `.next` (never published) and `.previous` (a switch that
// finished) are dropped. The earlier protocol could crash between renaming the live record away and
// renaming the new one in; that state has no live record and is rolled back to `.previous`. Finally
// every generated runtime folder the live record does not name is removed (best effort, retried next time).
export async function recoverWindowsGit({ data, rename: move = rename }) {
  const record = join(data, 'windows-git.json'), next = `${record}.next`, previous = `${record}.previous`;
  let live = await recordedRoot(data, record);
  if (live === undefined && await recordedRoot(data, previous) !== undefined) {
    await move(previous, record);
    live = await recordedRoot(data, record);
  }
  await rm(next, { force: true });
  await rm(previous, { force: true });
  const parent = join(data, 'windows-git-runtime');
  const names = await readdir(parent).catch(error => { if (error.code === 'ENOENT') return []; throw error; });
  for (const name of names) {
    const root = generatedRoot(data, join(parent, name));
    if (root && root !== live) await rm(root, { recursive: true, force: true }).catch(() => {});
  }
  return { root: live ?? null };
}

// Installs the bundled runtime beside the current one and switches the private record to it.
// The live record exists at every moment: the earlier record is first saved as `.previous`, then
// one rename replaces the live record with the new one. A crash at any point is repaired by
// recoverWindowsGit, which runs first. The caller commits after its whole update group succeeded,
// or rolls back to the earlier record.
export async function replaceWindowsGit({ runtime, data, arch, pin = windowsGitBundle(arch),
  archive = bundledGitArchive(runtime, arch), install = installWindowsGit, backend, rename: move = rename }) {
  const record = join(data, 'windows-git.json'), next = `${record}.next`, previous = `${record}.previous`;
  const { root: previousRoot } = await recoverWindowsGit({ data, rename: move });
  const options = backend ? { backend } : {};
  const installed = await install({ runtime, data, archive, arch, pin, record: next, ...options });
  const earlier = await readFile(record, 'utf8').catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  try {
    if (earlier !== null) await writePrivateFile(runtime, previous, earlier, options);
    await move(next, record);
  } catch (error) {
    await rm(next, { force: true });
    await rm(previous, { force: true });
    await rm(installed.root, { recursive: true, force: true });
    throw error;
  }
  return {
    installed,
    // Restores the earlier record (or none) and removes the runtime this update added.
    async rollback() {
      if (earlier !== null) await move(previous, record);
      else await rm(record, { force: true });
      await rm(installed.root, { recursive: true, force: true });
    },
    // Drops the earlier record and runtime; a leftover folder is removed by the next recovery.
    async commit() {
      await rm(previous, { force: true });
      if (previousRoot && previousRoot !== resolve(installed.root)) await rm(previousRoot, { recursive: true, force: true }).catch(() => {});
    },
  };
}

// Hashes bytes for diagnostics and the release source manifest check.
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
