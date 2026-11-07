// Verifies package contents and launcher isolation.
import assert from 'node:assert/strict';
// Runs independent package assembly acceptance cases.
import { test } from 'node:test';
// Executes packaged launchers with synthetic environments.
import { execFileSync } from 'node:child_process';
// Computes independent hashes for packaged files.
import { createHash } from 'node:crypto';
// Creates disposable package inputs and destinations.
import {
  access, copyFile, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, symlink, writeFile,
} from 'node:fs/promises';
// Locates temporary storage outside retained guests.
import { tmpdir } from 'node:os';
// Resolves input assets and package paths.
import { join } from 'node:path';
// Runs the builder script by its file path, which on Windows is not the URL's pathname.
import { fileURLToPath } from 'node:url';
// Assembles explicit binaries and frontend assets.
import { assemble } from '../scripts/backend-package.mjs';
// Names the bundled Git archive per architecture.
import { windowsGitBundle } from '../scripts/setup/windows-git-bundle.mjs';
// Names the shell runtime files a Windows bundle carries.
import { shellNames } from '../scripts/setup/windows-shell.mjs';
// Names the private Node files a Windows bundle carries.
import { sandboxNodeNames } from '../scripts/setup/windows-node-sandbox.mjs';

// A Windows package is the real Windows layout: the backend is a PE image of this processor and
// the bundle also carries the terminal, shell and Git runtimes, supplied here without downloads.
const windows = process.platform === 'win32';
const runtimeFiles = async () => new Map(['conpty.dll', 'OpenConsole.exe', ...shellNames].map(name => [name, Buffer.from(name)]));
const nodeFiles = async () => new Map(sandboxNodeNames.map(name => [name, Buffer.from(name)]));
const windowsParts = windows ? { terminalFiles: runtimeFiles, shellFiles: runtimeFiles, nodeFiles,
  gitFiles: async ({ arch }) => new Map([[windowsGitBundle(arch).file, Buffer.from('zip')], ['licenses/NOTICE.txt', Buffer.from('notice')]]) } : {};

// Creates harmless executable and web fixtures without reading local credentials.
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-package-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'server');
  if (windows) await writeFile(binary, peImage({ x64: 0x8664, arm64: 0xaa64 }[process.arch]));
  else await writeFile(binary, `#!${process.execPath}\n` +
    'console.log(JSON.stringify({args:process.argv.slice(2),env:process.env}));\n',
  { mode: 0o755 });
  const assets = join(root, 'assets');
  await mkdir(assets);
  await writeFile(join(assets, 'index.html'), '<h1>Packaged UI</h1>');
  await writeFile(join(assets, 'app.js'), 'console.log("asset");');
  return { root, binary, assets, output: join(root, 'bundle with spaces'), version: '0.1.0' };
}

test('package contains only declared runtime files', async t => {
  const input = await fixture(t);
  const manifest = await assemble({ ...input, ...windowsParts });
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.version, '0.1.0');
  assert.equal(manifest.role, 'host');
  assert.equal(manifest.platform, process.platform);
  assert.equal(manifest.arch, process.arch);
  assert.deepEqual(Object.keys(manifest.files).sort(), windows ? [
    'bin/conpty/LICENSE.txt', 'bin/conpty/OpenConsole.exe', 'bin/conpty/conpty.dll',
    `bin/git/${windowsGitBundle(process.arch).file}`, 'bin/git/licenses/NOTICE.txt',
    'bin/miniclaw-server.exe', ...sandboxNodeNames.map(name => `bin/node/${name}`), 'bin/python/sitecustomize.py', ...shellNames.map(name => `bin/shell/${name}`),
    'start-host.mjs', 'web/app.js', 'web/index.html',
  ].sort() : [
    'bin/miniclaw-server', 'start-host.mjs', 'web/app.js', 'web/index.html',
  ]);
  for (const [path, hash] of Object.entries(manifest.files)) {
    const data = await readFile(join(input.output, path));
    assert.equal(hash, createHash('sha256').update(data).digest('hex'));
  }
  // Execute bits are POSIX only; a Windows bundle's access comes from its installation folder.
  if (!windows) assert.equal((await stat(join(input.output, 'start-host.mjs'))).mode & 0o777, 0o755);
  const text = await readFile(join(input.output, 'manifest.json'), 'utf8');
  assert.ok(!text.includes(input.root));
});

test('the package launches the configured backend and rejects other installations', async t => {
  const input = await fixture(t);
  const probe = join(input.root, 'probe.cjs');
  const launch = { ...process.env, CODEX_HOME: '/test/existing-auth' };
  if (windows) {
    // Windows starts only a real .exe: the backend is a copy of this Node, which a preloaded probe
    // turns into the same reporter as the Mac script. Only the copy reports; the launcher runs on.
    await copyFile(process.execPath, input.binary);
    await writeFile(probe, "if (/miniclaw-server\\.exe$/i.test(process.execPath)) {"
      + ' process.stdout.write(JSON.stringify({args:process.argv.slice(1),env:process.env}) + "\\n", () => process.exit(0)); }\n');
    launch.NODE_OPTIONS = `--require ${JSON.stringify(probe)}`;
    // The installer creates the protected data folder; the Windows launcher never creates it.
    await mkdir(join(input.root, 'data'));
  }
  const manifest = await assemble({ ...input, ...windowsParts });
  assert.equal(manifest.role, 'host');
  const settings = { schemaVersion: 1, backend: 'host', hostReady: true,
    port: 9876, codexBin: '/test/codex' };
  await writeFile(join(input.root, 'settings.json'), JSON.stringify(settings));
  const result = JSON.parse(execFileSync(process.execPath, [join(input.output, 'start-host.mjs')], {
    encoding: 'utf8', env: launch,
  }));
  assert.deepEqual(result.args, []);
  assert.equal(result.env.MINICLAW_PORT, '9876');
  assert.equal(result.env.CODEX_HOME, '/test/existing-auth');
  assert.equal(result.env.CODEX_BIN, '/test/codex');
  // A provider the wizard did not wire in is switched off, not rediscovered.
  assert.equal(result.env.CLAUDE_BIN, '');
  assert.equal(result.env.MINICLAW_DATA_DIR, join(await realpath(input.root), 'data'));
  assert.equal(result.env.MINICLAW_CHAT_DIR, join(await realpath(input.root), 'data', 'chat-workspace'));
  // The Mac launcher's page folder keeps its baseline form, trailing separator included.
  if (process.platform !== 'win32') assert.equal(result.env.MINICLAW_WEB_DIR, `${join(await realpath(input.output), 'web')}/`);
  else {
    assert.equal(result.env.MINICLAW_WEB_DIR, `${join(await realpath(input.output), 'web')}\\`);
    assert.equal(result.env.MINICLAW_NETWORK_CONFIG, join(await realpath(input.root), 'data', 'windows-network.json'));
    // The backend stops when the launcher closes its stdin, since Windows has no SIGTERM.
    assert.equal(result.env.MINICLAW_LAUNCHER_STDIN, '1');
  }
  await writeFile(join(input.root, 'settings.json'), JSON.stringify({ ...settings, backend: 'linux' }));
  assert.throws(() => execFileSync(process.execPath, [join(input.output, 'start-host.mjs')],
    { stdio: 'pipe' }), error => (process.platform === 'win32' ? /completed installation/ : /completed Mac installation/)
    .test(error.stderr.toString()));
});

// Windows paths are joined natively: no stray slash after the data folder's separator.
test('the launcher builds native Windows state paths', async () => {
  const { win32, posix } = await import('node:path');
  const { launchPaths } = await import('../scripts/start-host.mjs');
  assert.deepEqual(launchPaths('C:\\Users\\Owner Ω\\MiniClaw Install\\runtime\\', win32), {
    data: 'C:\\Users\\Owner Ω\\MiniClaw Install\\data',
    chat: 'C:\\Users\\Owner Ω\\MiniClaw Install\\data\\chat-workspace',
    network: 'C:\\Users\\Owner Ω\\MiniClaw Install\\data\\windows-network.json',
  });
  // The same inputs on macOS give exactly the strings the launcher always produced.
  const mac = launchPaths('/Users/me/Library/Application Support/MiniClaw Install/runtime/', posix);
  assert.equal(mac.data, '/Users/me/Library/Application Support/MiniClaw Install/data');
  assert.equal(mac.chat, '/Users/me/Library/Application Support/MiniClaw Install/data/chat-workspace');
});

test('existing output is never overwritten', async t => {
  const input = await fixture(t);
  await mkdir(input.output);
  await writeFile(join(input.output, 'keep.txt'), 'existing data');
  await assert.rejects(assemble({ ...input, ...windowsParts }));
  assert.equal(await readFile(join(input.output, 'keep.txt'), 'utf8'), 'existing data');
  assert.deepEqual(await readdir(input.output), ['keep.txt']);
});

test('symlinked assets cannot copy host data', async t => {
  const input = await fixture(t);
  const secret = join(input.root, 'secret');
  await writeFile(secret, 'do not package');
  try { await symlink(secret, join(input.assets, 'linked')); }
  catch (error) {
    if (windows && error.code === 'EPERM') return t.skip('creating a symbolic link needs Developer Mode or an elevated terminal');
    throw error;
  }
  await assert.rejects(assemble({ ...input, ...windowsParts }), /symbolic|regular/i);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
});

test('invalid inputs leave no package', async t => {
  const input = await fixture(t);
  await rm(join(input.assets, 'index.html'));
  await assert.rejects(assemble({ ...input, ...windowsParts }), /index\.html/);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
});

test('builder requires a new output directory', async t => {
  const input = await fixture(t);
  const script = fileURLToPath(new URL('../scripts/package-backend.mjs', import.meta.url));
  for (const args of [[], ['--unknown', input.output]]) {
    assert.throws(() => execFileSync(process.execPath, [script, ...args],
      { stdio: 'pipe' }), error => {
      assert.match(error.stderr.toString(), /Usage: npm run package:backend/);
      return true;
    });
  }
  await mkdir(input.output);
  assert.throws(() => execFileSync(process.execPath, [script, input.output],
    { stdio: 'pipe' }), error => {
    assert.match(error.stderr.toString(), /Output already exists/);
    return true;
  });
  assert.deepEqual(await readdir(input.output), []);
});

// Builds a minimal PE image whose header names one machine type.
function peImage(machine) {
  const bytes = Buffer.alloc(128);
  bytes.write('MZ', 0, 'ascii');
  bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE(machine, 68);
  return bytes;
}

// The Rust target, not the Node process, decides a Windows bundle's architecture.
test('a Windows bundle takes its architecture from the built executable', async t => {
  for (const [machine, arch, other] of [[0xaa64, 'arm64', 'x64'], [0x8664, 'x64', 'arm64']]) {
    const input = await fixture(t);
    await writeFile(input.binary, peImage(machine));
    const asked = [];
    const runtime = async ({ arch }) => { asked.push(arch);
      return new Map(['conpty.dll', 'OpenConsole.exe', 'busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt'].map(name => [name, Buffer.from(name)])); };
    const git = async ({ arch }) => { asked.push(arch);
      return new Map([[windowsGitBundle(arch).file, Buffer.from('zip')], ['licenses/NOTICE.txt', Buffer.from('notice')]]); };
    const node = async ({ arch }) => { asked.push(arch); return nodeFiles(); };
    const manifest = await assemble({ ...input, platform: 'win32', terminalFiles: runtime, shellFiles: runtime, gitFiles: git, nodeFiles: node });
    assert.equal(manifest.arch, arch);
    assert.deepEqual(asked, [arch, arch, arch, arch], 'the terminal, shell, Git and private Node match the backend');
    for (const name of sandboxNodeNames) assert.match(manifest.files[`bin/node/${name}`], /^[a-f0-9]{64}$/);
    // MiniClaw's sitecustomize for sandboxed Python, byte for byte the reviewed file the backend checks.
    assert.equal(manifest.files['bin/python/sitecustomize.py'],
      createHash('sha256').update(await readFile(new URL('../scripts/setup/python-sandbox/sitecustomize.py', import.meta.url))).digest('hex'));
    assert.match(manifest.files[`bin/git/${windowsGitBundle(arch).file}`], /^[a-f0-9]{64}$/);
    assert.match(manifest.files['bin/git/licenses/NOTICE.txt'], /^[a-f0-9]{64}$/);
    // A caller naming another architecture is refused before anything is written.
    await assert.rejects(assemble({ ...input, output: `${input.output}-mismatch`, platform: 'win32', arch: other,
      terminalFiles: runtime, shellFiles: runtime, gitFiles: git, nodeFiles: node }), /architecture/i);
    await assert.rejects(access(`${input.output}-mismatch`), { code: 'ENOENT' });
  }
  const input = await fixture(t);
  await writeFile(input.binary, peImage(0x014c));
  await assert.rejects(assemble({ ...input, platform: 'win32', terminalFiles: async () => new Map(), shellFiles: async () => new Map() }), /architecture|unsupported/i);
});

// Windows packages carry MiniClaw's Git build; a missing or wrong-architecture archive stops packaging.
test('a Windows bundle is never assembled without its pinned Git runtime', async t => {
  const input = await fixture(t);
  await writeFile(input.binary, peImage(0xaa64));
  const runtime = async () => new Map(['conpty.dll', 'OpenConsole.exe', 'busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt'].map(name => [name, Buffer.from(name)]));
  const saved = process.env.MINICLAW_GIT_ARCHIVE;
  delete process.env.MINICLAW_GIT_ARCHIVE;
  t.after(() => { if (saved === undefined) delete process.env.MINICLAW_GIT_ARCHIVE; else process.env.MINICLAW_GIT_ARCHIVE = saved; });
  // Without the local archive the packager asks MiniClaw's release; here that release is unreachable.
  const original = globalThis.fetch, requests = [];
  globalThis.fetch = async url => { requests.push(String(url)); return new Response('', { status: 404 }); };
  t.after(() => { globalThis.fetch = original; });
  await assert.rejects(assemble({ ...input, platform: 'win32', terminalFiles: runtime, shellFiles: runtime }), /MINICLAW_GIT_ARCHIVE/);
  assert.equal(requests.length, 1);
  assert.match(requests[0], /^https:\/\/(?:github\.com\/Sep-Fading\/miniclaw-windows\/releases\/download\/v0\.1\.5\/|api\.github\.com\/repos\/Sep-Fading\/miniclaw-windows\/releases\/assets\/)/);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
  const wrong = async () => new Map([[windowsGitBundle('x64').file, Buffer.from('zip')]]);
  await assert.rejects(assemble({ ...input, platform: 'win32', terminalFiles: runtime, shellFiles: runtime, gitFiles: wrong }), /Git runtime/);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
});

// Windows packages carry MiniClaw's private Node; without the built seed library nothing is written.
test('a Windows bundle is never assembled without its private Node', async t => {
  const input = await fixture(t);
  await writeFile(input.binary, peImage(0xaa64));
  const runtime = async () => new Map(['conpty.dll', 'OpenConsole.exe', 'busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt'].map(name => [name, Buffer.from(name)]));
  const git = async ({ arch }) => new Map([[windowsGitBundle(arch).file, Buffer.from('zip')]]);
  await assert.rejects(assemble({ ...input, platform: 'win32', terminalFiles: runtime, shellFiles: runtime, gitFiles: git }), /seed library was not built/);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
  const partial = async () => new Map([['node.exe', Buffer.from('exe')]]);
  await assert.rejects(assemble({ ...input, platform: 'win32', terminalFiles: runtime, shellFiles: runtime, gitFiles: git, nodeFiles: partial }), /private Node file: LICENSE/);
  await assert.rejects(access(input.output), { code: 'ENOENT' });
});
