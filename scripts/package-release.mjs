#!/usr/bin/env node
// Builds a self-contained release folder and archive with a native backend and Node runtime.
// Windows x64/ARM64 packages omit VM binaries; Apple silicon packages include the VM helpers.
// Releases are unsigned; the operating system may prompt before the first run.
//
//   miniclaw package [OUTPUT_FOLDER]      default: ./release
// Hashes files for the manifest.
import { createHash } from 'node:crypto';
// Runs the compilers without a shell.
import { execFile } from 'node:child_process';
// Assembles the release folder.
import { chmod, copyFile, cp, lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { gunzipSync } from 'node:zlib';
// Downloads and verifies the pinned Node archive.
import { downloadImage } from './setup/download.mjs';
// Finds the Go toolchain an installation already unpacked.
import { defaultInstallation } from './setup/default-installation.mjs';
// Pins the Git build a Windows bundle carries, whose source is published beside the release.
import { gitCorrespondingSource, windowsGitBundle } from './setup/windows-git-bundle.mjs';
// Pins npm's tree inside the private Node, which the release audit checks instead of scanning.
import { npmTreeDigest, sandboxNpm } from './setup/windows-node-sandbox.mjs';

const exec = promisify(execFile);
const root = fileURLToPath(new URL('../', import.meta.url));

// Identifies Finder metadata that should never enter a release.
function macMetadata(path) {
  const name = path.replaceAll('\\', '/').split('/').at(-1);
  return name === '.DS_Store' || name.startsWith('._');
}

// Identifies VM installers and disk media that belong in local caches.
function vmMedia(name) {
  return /\.(?:ipsw|dmg|img|iso|qcow2|vmdk|vdi|vhdx?|sparseimage|sparsebundle|ova)$/i.test(name);
}

// Flags literal credential assignments while allowing the documented placeholder.
function populatedSecretAssignment(line) {
  const match = /^\s*(?:export\s+)?([A-Z][A-Z0-9_]*)\s*=\s*(.+)$/.exec(line);
  if (!match || !/(?:API_KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PRIVATE_KEY)/.test(match[1])) return false;
  const value = match[2].trim().replace(/^['"]|['"]$/g, '');
  return value !== '' && value !== 'your-key' && !/^<[^>]+>$/.test(value)
    && !/^[a-z_][a-z0-9_.]*\([^)]*\)$/.test(value);
}

// Refuses local credentials and VM media in copied release trees. `pinned` names subtrees the caller has
// already verified byte for byte against a pin (the private Node's npm tree); they are not scanned again.
export async function auditReleaseTree(folder, { ignoredContent = [], ignoreMacMetadata = false, pinned = [] } = {}) {
  const walk = async (directory, prefix = '') => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (ignoreMacMetadata && macMetadata(path)) continue;
      if (pinned.includes(path) && entry.isDirectory()) continue;
      const source = join(directory, entry.name);
      const name = entry.name.toLowerCase();
      const credentialTemplate = /(?:^|\.)env\.example$/.test(name) && !name.startsWith('.');
      const privateName = /^\.env(?:\.|$)|^\.(?:npmrc|netrc|pypirc|aws|ssh|codex|claude)$/.test(name)
        || /^(?:credentials?|secrets?|auth|settings)\.(?:json|ya?ml|toml|env|ini|config)$/.test(name) && !credentialTemplate
        || /^id_(?:rsa|dsa|ecdsa|ed25519)(?:\.|$)/.test(name)
        || /\.(?:env|pem|key|p12|pfx|jks|keystore|sqlite|sqlite3|db)$/.test(name);
      if (privateName || vmMedia(name) || macMetadata(path) || (!entry.isDirectory() && !entry.isFile())) {
        throw new Error(`Release audit refused private or unsupported path: ${source}`);
      }
      if (entry.isDirectory()) { await walk(source, path); continue; }
      if (ignoredContent.includes(path)) continue;
      const bytes = await readFile(source);
      if (bytes.includes(0)) continue;
      const contents = bytes.toString('utf8');
      const secret = /-----BEGIN (?:[A-Z ]*PRIVATE KEY|PGP PRIVATE KEY BLOCK)-----/.test(contents)
        || /\b(?:sk-(?:proj-|live-)?[A-Za-z0-9_-]{24,}|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|xox[baprs]-[A-Za-z0-9-]{20,}|AKIA[0-9A-Z]{16})\b/.test(contents)
        || contents.split(/\r?\n/).some(populatedSecretAssignment)
        || credentialTemplate && contents.split(/\r?\n/).some(line => {
          const value = /^\s*(?:export\s+)?[A-Za-z_][A-Za-z0-9_]*\s*=\s*(.*)$/.exec(line)?.[1];
          return value !== undefined && value.trim() !== '';
        });
      if (secret) throw new Error(`Release audit refused possible secret in: ${source}`);
    }
  };
  await walk(folder);
}

// The private Node's npm tree inside a Windows release, as the bundle lays it out.
const releaseNpm = 'prebuilt/bundle/bin/node/node_modules/npm';

// Audits a Windows release folder. npm's tree from the official Node archive holds an empty `.npmrc` and
// documentation with example tokens; it is exempt only when it is exactly the pinned tree (file count and
// digest, as the backend checks it), everything else is audited as usual.
export async function auditWindowsRelease(folder, { npm = sandboxNpm } = {}) {
  const tree = new Map();
  const collect = async (directory, prefix = '') => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await collect(join(directory, entry.name), path);
      else if (entry.isFile()) tree.set(path, await readFile(join(directory, entry.name)));
      else throw new Error(`Release audit refused private or unsupported path: ${join(directory, entry.name)}`);
    }
  };
  const present = await lstat(join(folder, releaseNpm)).then(info => info.isDirectory(), () => false);
  if (present) {
    await collect(join(folder, releaseNpm));
    if (tree.size !== npm.files || npmTreeDigest(tree) !== npm.digest) throw new Error('Release audit refused the private npm tree: it differs from its pin');
  }
  await auditReleaseTree(folder, { pinned: present ? [releaseNpm] : [] });
}

// Checks raw tar headers because macOS tar hides AppleDouble entries in its listing.
export async function auditReleaseArchive(archive) {
  const data = gunzipSync(await readFile(archive));
  for (let offset = 0; offset + 512 <= data.length;) {
    const header = data.subarray(offset, offset + 512);
    if (header.every(byte => byte === 0)) return;
    const field = (start, end) => header.subarray(start, end).toString('utf8').split('\0')[0];
    const name = field(0, 100);
    const prefix = field(345, 500);
    const path = prefix ? `${prefix}/${name}` : name;
    const size = Number.parseInt(field(124, 136).trim(), 8);
    const type = field(156, 157);
    if (!Number.isSafeInteger(size) || size < 0 || !path || !['', '0', '5'].includes(type)
      || path.split('/').some(part => macMetadata(part)
        || /^\.env(?:\.|$)/.test(part) || vmMedia(part))) {
      throw new Error(`Release audit refused archive entry: ${path || '<invalid>'}`);
    }
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  throw new Error('Release audit refused truncated archive.');
}

// Lists every regular file below a folder by its slash-separated relative path.
async function listFiles(folder, prefix = '') {
  const names = [];
  for (const entry of await readdir(join(folder, prefix), { withFileTypes: true })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) names.push(...await listFiles(folder, path));
    else if (entry.isFile()) names.push(path);
    else throw new Error(`Git corresponding source has an unsupported entry: ${path}`);
  }
  return names;
}

// Verifies the Git corresponding-source folder (MINICLAW_GIT_SOURCE) against its SOURCE-MANIFEST.json
// and the bundled runtime pin, then archives it as <release>-git-corresponding-source.tar.gz beside
// <release>.tar.gz. Source for another build, or any changed or unlisted file, stops the release.
export async function gitSourceArtifact({ arch, output, tar, folder = process.env.MINICLAW_GIT_SOURCE,
  pin = windowsGitBundle(arch), platform = process.platform }) {
  if (!folder) throw new Error('No Git corresponding-source folder: set MINICLAW_GIT_SOURCE, or let the packager fetch it from the release (GH_TOKEN). Windows releases publish it beside the archive.');
  const archive = `${output}-git-corresponding-source.tar.gz`;
  if (await lstat(archive).then(() => true, () => false)) throw new Error(`${archive} already exists; choose a new folder.`);
  const manifest = JSON.parse(await readFile(join(folder, 'SOURCE-MANIFEST.json'), 'utf8'));
  const runtime = manifest.runtime?.[arch];
  if (runtime?.file !== pin.file || runtime.bytes !== pin.bytes || runtime.sha256 !== pin.digest) {
    throw new Error(`Git corresponding source does not describe the bundled ${arch} runtime ${pin.file}`);
  }
  const listed = new Map((manifest.files ?? []).map(item => [item.file, item]));
  for (const name of await listFiles(folder)) {
    if (name === 'SOURCE-MANIFEST.json') continue;
    const item = listed.get(name);
    if (!item) throw new Error(`Git corresponding source has an unlisted file: ${name}`);
    const bytes = await readFile(join(folder, name));
    if (bytes.length !== item.bytes || createHash('sha256').update(bytes).digest('hex') !== item.sha256) {
      throw new Error(`Git corresponding source file changed: ${name}`);
    }
    listed.delete(name);
  }
  if (listed.size) throw new Error(`Git corresponding source is missing: ${[...listed.keys()].join(', ')}`);
  try {
    await step('archive the Git corresponding source', tar, ['--format', 'ustar', ...(platform === 'darwin' ? ['--no-mac-metadata'] : []),
      '--no-xattrs', '--no-acls', '-czf', archive, '-C', dirname(resolve(folder)), relative(dirname(resolve(folder)), resolve(folder))],
    { env: { ...process.env, COPYFILE_DISABLE: '1' } });
  } catch (error) { await rm(archive, { force: true }); throw error; }
  const bytes = await readFile(archive);
  return { archive, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}

// The distributor the user named for the written source offer (2026-10-04). Only a name was supplied;
// MINICLAW_SOURCE_OFFER_CONTACT overrides it, for example with a postal or e-mail address.
export const defaultSourceOfferContact = 'Sep';

// Writes the release's written source offer naming the distributor contact.
export function gitSourceOffer({ contact = process.env.MINICLAW_SOURCE_OFFER_CONTACT, artifact, pin, date = new Date() }) {
  if (!contact?.trim()) contact = defaultSourceOfferContact;
  const name = artifact.archive.replaceAll('\\', '/').split('/').at(-1);
  return [
    'Git source code for this MiniClaw release', '',
    `This release contains ${pin.file} (SHA256 ${pin.digest}), Git for Windows v2.56.0.windows.1 modified by`,
    'MiniClaw and licensed under the GNU General Public License version 2, with libraries under the GNU LGPL 2.1.',
    `Its complete corresponding source is published beside this release as ${name}`,
    `(${artifact.bytes} bytes, SHA256 ${artifact.sha256}).`, '',
    'Written offer: for at least three years from the date below, the distributor named here will give any third',
    'party, for a charge no more than the cost of physically performing source distribution, a complete',
    'machine-readable copy of that corresponding source on a medium customarily used for software interchange.', '',
    `Distributor contact: ${contact.trim()}`,
    `Release date: ${date.toISOString().slice(0, 10)}`, ''].join('\n');
}

// The official Node 24 LTS build for Apple silicon. Checksum from
// https://nodejs.org/dist/v24.21.0/SHASUMS256.txt, read on 2026-09-21.
export const node = {
  id: 'node', name: 'Node.js 24.21.0 (official build)', file: 'node-v24.21.0-darwin-arm64.tar.gz',
  url: 'https://nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-arm64.tar.gz', bytes: 52909993,
  algorithm: 'sha256', digest: 'bed7eea5325e1108f32ce5228ddd6a5f0f08a499ee42aa7442aea583702f6057',
};

// Official Windows checksums and Content-Length verified from nodejs.org on2026-09-30.
const windowsNode = {
  x64: { bytes: 37618919, digest: '158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541' },
  arm64: { bytes: 33679608, digest: '8779b1bde1d39f8d420e3b57aa657b39891af434d3de44a919044cec06785921' },
};

// Selects native runtime files and launch syntax; Windows releases contain no VM binaries.
export function releasePlatform(platform = process.platform, arch = process.arch) {
  if (platform === 'darwin' && arch === 'arm64') return { vm: true, node, tar: '/usr/bin/tar',
    launcher: 'miniclaw', launcherText: '#!/bin/sh\nhere="$(cd "$(dirname "$0")" && pwd)"\nexec "$here/node/bin/node" "$here/scripts/cli.mjs" "$@"\n' };
  if (platform !== 'win32' || !windowsNode[arch]) throw new Error('Unsupported release platform or architecture');
  const file = `node-v24.21.0-win-${arch}.zip`;
  return { vm: false, tar: 'tar.exe', node: { id: 'node', name: 'Node.js24.21.0 (official build)',
    file, url: `https://nodejs.org/dist/v24.21.0/${file}`, algorithm: 'sha256', ...windowsNode[arch] },
    launcher: 'miniclaw.cmd', launcherText: '@echo off\r\nsetlocal DisableDelayedExpansion\r\n"%~dp0node\\bin\\node.exe" "%~dp0scripts\\cli.mjs" %*\r\nexit /b %errorlevel%\r\n' };
}

// Returns the architecture the release ships: the backend bundle's, which follows the Rust
// target. The bundled Node must match it, whatever Node runs this packager.
export function releaseArch(bundle, platform = process.platform, hostArch = process.arch) {
  if ((bundle.platform ?? 'darwin') !== platform) throw new Error(`The backend bundle platform is ${bundle.platform}, not ${platform}`);
  if (platform === 'win32' ? !windowsNode[bundle.arch] : bundle.arch !== hostArch) {
    throw new Error(`The backend bundle architecture ${bundle.arch} cannot be released from this ${platform} ${hostArch} host`);
  }
  return bundle.arch;
}


// The release's READ ME FIRST.txt. Windows x64 is built and host-checked but its sandbox has only
// been verified natively on Windows 11 ARM64 (docs/windows/x64-readiness.md), and says so.
export function releaseReadme({ vm, version, arch }) {
  return (vm ? [
    `MiniClaw ${version} for Apple silicon Macs`, '',
    'Install:   open Terminal, drag the file "miniclaw" into it, type  setup  and press Return.',
    'Start:     ./miniclaw',
    'Update:    unpack a newer release, then run its  ./miniclaw update  and, for a VM,  ./miniclaw update-vm',
    '', 'This release is not signed with an Apple Developer ID. The first time, macOS may say it cannot',
    'check the app. Open System Settings, Privacy & Security, and choose "Open Anyway", or run:',
    '  xattr -dr com.apple.quarantine <this folder>',
    '', 'Nothing else needs to be installed. MiniClaw uses your existing Codex and Claude Code sign-ins.',
    'If Claude Code is missing, setup downloads and installs it from Anthropic (downloads.claude.ai) by itself.', ''] : [
    `MiniClaw ${version} for Windows ${arch}`, '',
    ...(arch === 'x64' ? ['Windows x64: builds and passes host checks; not verified natively in the sandbox.',
      'All native sandbox verification so far ran on Windows 11 ARM64.', ''] : []),
    'Extract the whole folder before running it.',
    'Install: .\\miniclaw.cmd setup', 'Start: .\\miniclaw.cmd',
    'Update: stop MiniClaw, extract the newer release, then run .\\miniclaw.cmd update',
    'Repair: .\\miniclaw.cmd repair (sets up network access for sandboxed tools again)',
    'Uninstall: .\\miniclaw.cmd uninstall (keeps your data; add --delete-data to remove it)',
    'Keep this folder: its miniclaw.cmd starts, updates and uninstalls MiniClaw.',
    '', 'Setup requests administrator approval for the network isolation service.',
    'MiniClaw needs the Microsoft Visual C++ runtime, which is not bundled. If it is missing or too old, setup downloads it',
    'from Microsoft (aka.ms/vc14), checks Microsoft\'s signature and installs it, with one more administrator approval.',
    'MiniClaw needs Codex or Claude Code. Claude Code is not bundled. If it is missing, setup downloads and installs it from Anthropic',
    '(downloads.claude.ai, about 250 MB, for your user only) by itself while it asks its questions. Sign in afterwards from MiniClaw.',
    'Node is bundled. Rust, Swift and Go are not required to install this release.',
    'Git is bundled privately for workspace commands (prebuilt\\bundle\\bin\\git, licences in its licenses folder);',
    'its source code is published beside this release, see GIT-SOURCE-OFFER.txt.',
    'Windows VM features are excluded.', '']).join('\n');
}

// package.json. Only these are copied; build tools stay behind.
export async function closure(modules, names) {
  const seen = new Set();
  const pending = [...names];
  while (pending.length) {
    const name = pending.pop();
    if (seen.has(name)) continue;
    const manifest = JSON.parse(await readFile(join(modules, name, 'package.json'), 'utf8'));
    seen.add(name);
    pending.push(...Object.keys(manifest.dependencies ?? {}));
  }
  return [...seen].sort();
}

// Hashes every regular file below a folder, by its path relative to that folder.
export async function checksums(folder, prefix = '') {
  const files = {};
  for (const entry of (await readdir(join(folder, prefix), { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) Object.assign(files, await checksums(folder, path));
    else if (entry.isFile() && path !== 'manifest.json') {
      files[path] = createHash('sha256').update(await readFile(join(folder, path))).digest('hex');
    }
  }
  return files;
}

// Extracts the verified Node archive into scratch space and copies only runtime files.
export async function extractNodeRuntime(archive, destination, work, { platform = process.platform, tar = platform === 'win32' ? 'tar.exe' : '/usr/bin/tar' } = {}) {
  const unpacked = join(work, 'node-unpacked');
  await mkdir(unpacked, { recursive: true });
  await step('unpack Node', tar, ['-xf', archive, '-C', unpacked, '--strip-components', '1'],
    { env: { ...process.env, COPYFILE_DISABLE: '1' } });
  const executable = platform === 'win32' ? ['node.exe', 'bin/node.exe'] : ['bin/node', 'bin/node'];
  for (const [source, target] of [executable, ['LICENSE', 'LICENSE']]) {
    const from = join(unpacked, source);
    if (!(await lstat(from)).isFile()) throw new Error(`Node archive has no regular ${source}`);
    const to = join(destination, target);
    await mkdir(dirname(to), { recursive: true });
    await copyFile(from, to);
  }
  if (platform !== 'win32') await chmod(join(destination, 'bin/node'), 0o755);
}

// Runs one build step and shows what it is doing.
async function step(title, command, args, options = {}) {
  console.log(`  ${title}`);
  try { await exec(command, args, { maxBuffer: 1 << 27, ...options }); }
  catch (error) { throw new Error(`${title} failed:\n${error.stderr || error.message}`); }
}

// Compiles with the selected Swift toolchain and its matching active SDK.
async function swift(sources, output, work) {
  const args = ['swiftc', '-swift-version', '6', '-warnings-as-errors', '-parse-as-library', '-O'];
  await step(`compile ${relative(root, output)}`, '/usr/bin/xcrun', [...args, ...sources, '-o', output], {
    env: { ...process.env, CLANG_MODULE_CACHE_PATH: join(work, 'clang-modules'),
      SWIFT_MODULE_CACHE_PATH: join(work, 'swift-modules') },
  });
}

async function main() {
  const args = process.argv.slice(2);
  if (args.some(arg => arg.startsWith('-'))) throw new Error('Usage: miniclaw package [OUTPUT_FOLDER]');
  // Only the platform is known before the backend is built; Windows picks its Node afterwards.
  let selected = releasePlatform(process.platform, process.platform === 'win32' && !windowsNode[process.arch] ? 'x64' : process.arch);
  const output = resolve(args[0] ?? 'release');
  if (await lstat(output).then(() => true, () => false)) throw new Error(`${output} already exists; choose a new folder.`);
  for (const sibling of [`${output}.tar.gz`, `${output}-git-corresponding-source.tar.gz`]) {
    if (await lstat(sibling).then(() => true, () => false)) throw new Error(`${sibling} already exists; choose a new folder.`);
  }
  const go = selected.vm ? join(await defaultInstallation(), 'network-tools/go/bin/go') : null;
  if (selected.vm && !await lstat(go).then(() => true, () => false)) {
    throw new Error(`The network helper needs the pinned Go toolchain, which a VM installation unpacks at ${go}. Install MiniClaw with a VM on this Mac first.`);
  }
  // The backend's version is the release's version.
  const version = /^version = "([^"]+)"/m.exec(await readFile(join(root, 'server/Cargo.toml'), 'utf8'))?.[1];
  if (!version) throw new Error('The backend has no version in server/Cargo.toml.');
  const work = await mkdtemp(join(tmpdir(), 'mc-release-'));
  const prebuilt = join(output, 'prebuilt');
  try {
    for (const folder of ['scripts', 'web']) {
      await auditReleaseTree(join(root, folder), { ignoreMacMetadata: true });
    }
    // This source file contains the scanner's own sample key patterns.
    await auditReleaseTree(join(root, 'server/src'), { ignoredContent: ['project_memory.rs'] });
    console.log(`Building MiniClaw ${version} into ${output}`);
    await mkdir(prebuilt, { recursive: true });

    // The backend and the page, assembled exactly as an installation expects them.
    await step('build the backend and the page', process.execPath, [join(root, 'scripts/package-backend.mjs'), join(prebuilt, 'bundle')], { cwd: root });
    const arch = releaseArch(JSON.parse(await readFile(join(prebuilt, 'bundle/manifest.json'), 'utf8')));
    selected = releasePlatform(process.platform, arch);
    if (selected.vm) {
    await step('build the VM worker', 'cargo', ['build', '--release', '--locked', '--bin', 'miniclaw-worker'], { cwd: join(root, 'server') });
    await copyFile(join(root, 'target/release/miniclaw-worker'), join(prebuilt, 'miniclaw-worker'));

    const modules = join(dirname(dirname(go)), 'modules');
    await step('build the network helper', '/usr/bin/env', [`GOPATH=${modules}`, `GOCACHE=${join(work, 'gocache')}`, 'GOTOOLCHAIN=local',
      go, 'build', '-mod=readonly', '-trimpath', '-o', join(prebuilt, 'miniclaw-network'), '.'], { cwd: join(root, 'scripts/vm/network-helper') });

    await swift(['experiments/vm-lab.swift', 'experiments/vm-window.swift', 'experiments/vm-headless.swift',
      'vm/host-relay.swift', 'vm/guest-bridge.swift', 'vm/guest-network.swift'].map(path => join(root, 'scripts', path)), join(prebuilt, 'master-vm'), work);
    await swift([join(root, 'scripts/vm/desktop-stream.swift')], join(prebuilt, 'miniclaw-desktop-stream'), work);
    await step('compile the socket probe', '/usr/bin/xcrun', ['clang', '-Wall', '-Wextra', '-Werror', '-O2',
      join(root, 'scripts/vm/master-probe.c'), '-o', join(prebuilt, 'socket-probe')]);
    // Setup signs these again on the target Mac; an ad-hoc signature here keeps them runnable.
    for (const name of ['miniclaw-desktop-stream', 'socket-probe']) await step(`sign ${name}`, '/usr/bin/codesign', ['--sign', '-', join(prebuilt, name)]);
    for (const name of ['miniclaw-worker', 'miniclaw-network', 'master-vm', 'miniclaw-desktop-stream', 'socket-probe']) await chmod(join(prebuilt, name), 0o755);
    }

    console.log('  copy the scripts and their run-time packages');
    // Tests and probes of the development checkout stay behind; the entitlements file is needed.
    await cp(join(root, 'scripts'), join(output, 'scripts'), { recursive: true,
      filter: source => !macMetadata(source) && !source.replaceAll('\\', '/').includes('/scripts/vm/network-helper')
        && !/\/scripts\/experiments\/.*\.(mjs|md)$/.test(source.replaceAll('\\', '/'))
        && (selected.vm || !/\.(swift|c|h|go)$/.test(source)) });
    for (const name of await closure(join(root, 'node_modules'), ['@clack/prompts'])) {
      await cp(join(root, 'node_modules', name), join(output, 'node_modules', name), {
        recursive: true, filter: source => !macMetadata(source),
      });
    }
    await writeFile(join(output, 'package.json'), `${JSON.stringify({ name: 'miniclaw', version, private: true, type: 'module',
      bin: { miniclaw: 'scripts/cli.mjs' }, engines: { node: '>=24' } }, null, 2)}\n`);

    const runtimeNode = selected.node;
    console.log(`  download ${runtimeNode.file} (${Math.round(runtimeNode.bytes / 1e6)} MB) from nodejs.org and check it`);
    const cache = selected.vm ? join(homedir(), 'Library/Caches/MiniClaw-setup') : join(work, 'node-cache');
    await mkdir(cache, { recursive: true, mode: 0o700 });
    if (selected.vm) await chmod(cache, 0o700);
    const { path: archive } = await downloadImage(runtimeNode, cache);
    await extractNodeRuntime(archive, join(output, 'node'), work);

    await writeFile(join(output, selected.launcher), selected.launcherText, { mode: 0o755 });
    await writeFile(join(output, 'READ ME FIRST.txt'), releaseReadme({ vm: selected.vm, version, arch }));

    if (!selected.vm) {
      // The bundle carries the pinned Git ZIP; its source goes beside the release archive.
      const pin = windowsGitBundle(arch);
      // MINICLAW_GIT_SOURCE, or the pinned source archive from MiniClaw's release unpacked into the work folder.
      const folder = await gitCorrespondingSource({ directory: work, tar: selected.tar });
      const artifact = await gitSourceArtifact({ arch, output, tar: selected.tar, pin, folder });
      await writeFile(join(output, 'GIT-SOURCE-OFFER.txt'), gitSourceOffer({ artifact, pin }));
      console.log(`  Git corresponding source: ${artifact.archive} (${(artifact.bytes / 1e6).toFixed(0)} MB)`);
    }
    const files = await checksums(prebuilt);
    await writeFile(join(prebuilt, 'manifest.json'), `${JSON.stringify({ schemaVersion: 1, version, platform: process.platform, arch,
      builtAt: new Date().toISOString(), node: runtimeNode.file, files }, null, 2)}\n`);
    await (selected.vm ? auditReleaseTree(output) : auditWindowsRelease(output));
    await step('make the archive', selected.tar, ['--format', 'ustar', ...(selected.vm ? ['--no-mac-metadata'] : []), '--no-xattrs', '--no-acls',
      '-czf', `${output}.tar.gz`, '-C', dirname(output), relative(dirname(output), output)],
      { env: { ...process.env, COPYFILE_DISABLE: '1' } });
    await auditReleaseArchive(`${output}.tar.gz`);
    const size = (await lstat(`${output}.tar.gz`)).size;
    console.log(`Done: ${output}\n      ${output}.tar.gz (${(size / 1e6).toFixed(0)} MB), ${Object.keys(files).length} prebuilt files recorded.`);
  } catch (error) {
    await rm(output, { recursive: true, force: true });
    await rm(`${output}.tar.gz`, { force: true });
    await rm(`${output}-git-corresponding-source.tar.gz`, { force: true });
    throw error;
  } finally { await rm(work, { recursive: true, force: true }); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
