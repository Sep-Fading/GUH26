// Verifies Windows setup contracts without changing installations.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { recommend, planSetup, GiB } from '../scripts/setup/plan.mjs';
import { executable } from '../scripts/setup/system.mjs';
// Supplies a stand-in bundled Git runtime.
import { gitFixture } from './support/windows-git-fixture.mjs';
import { assemble } from '../scripts/backend-package.mjs';

const host = { platform: 'win32', arch: 'x64', cpuCount: 8,
  memoryBytes: 16 * GiB, diskBytes: 512 * GiB, freeBytes: 100 * GiB };

test('Windows installs without a VM and refuses VM requests', () => {
  assert.equal(recommend(host).vmEnabled, false);
  assert.equal(recommend(host).masterFits, false);
  const input = { port: 8787, providers: { codex: true, claude: false }, vmEnabled: false };
  assert.equal(planSetup(input, host).platform, 'win32');
  assert.throws(() => planSetup({ ...input, vmEnabled: true }, host), /Windows.*VM|VM.*Windows/);
});

test('Windows executable lookup uses semicolons and rejects directories and cmd shims', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-windows-path-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'cargo.exe'));
  await writeFile(join(root, 'codex.cmd'), '@echo off');
  await writeFile(join(root, 'codex.exe'), 'fixture');
  const options = { platform: 'win32', env: { PATH: `${root};C:\\missing`, PATHEXT: '.COM;.EXE;.CMD' } };
  assert.equal(await executable('codex', [], options), join(root, 'codex.exe'));
  assert.equal(await executable('cargo', [], options), null);
  assert.equal(await executable('codex.cmd', [], options), null);
});

test('Windows packages use exe and record their platform', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-windows-package-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'assets'));
  await writeFile(join(root, 'assets/index.html'), '<html></html>');
  // The packager reads the architecture from the PE header, so the fixture carries an x64 one.
  const image = Buffer.alloc(128);
  image.write('MZ', 0, 'ascii'); image.writeUInt32LE(64, 60); image.writeUInt32LE(0x4550, 64); image.writeUInt16LE(0x8664, 68);
  await writeFile(join(root, 'server.exe'), image);
  const result = await assemble({ binary: join(root, 'server.exe'), assets: join(root, 'assets'),
    output: join(root, 'package'), version: 'test', platform: 'win32', arch: 'x64', gitFiles: gitFixture,
    shellFiles: async () => new Map([['busybox.exe', Buffer.from('shell')], ['busybox-w32-FRP-6075-g169694ebd.tgz', Buffer.from('source')], ['LICENSE.txt', Buffer.from('license')]]),
    terminalFiles: async () => new Map([['conpty.dll', Buffer.from('dll')], ['OpenConsole.exe', Buffer.from('exe')]]) });
  assert.equal(result.platform, 'win32');
  assert.equal(result.arch, 'x64');
  assert.ok(result.files['bin/miniclaw-server.exe']);
});

test('Windows defaults stay under local app data', async () => {
  const { installationPaths } = await import('../scripts/setup/default-installation.mjs');
  const paths = installationPaths('C:\\Users\\Test User', { platform: 'win32',
    env: { LOCALAPPDATA: 'D:\\Local Data' } });
  assert.equal(paths.installation, 'D:\\Local Data\\MiniClaw Install');
  assert.equal(paths.record, 'D:\\Local Data\\MiniClaw\\installation.json');
  assert.equal(paths.cache, 'D:\\Local Data\\MiniClaw\\setup-cache');
});

test('Windows provider discovery finds native npm payloads and Claude native install', async () => {
  const { detectProviders } = await import('../scripts/setup/providers.mjs');
  const seen = {};
  await detectProviders({ platform: 'win32', arch: 'x64', home: 'C:\\Users\\Test User',
    env: { APPDATA: 'C:\\Users\\Test User\\AppData\\Roaming', PATH: 'C:\\tools;D:\\npm' },
    find: async (name, candidates) => { seen[name] = candidates; return null; } });
  assert.ok(seen.codex.some(path => path.includes('@openai\\codex-win32-x64\\vendor\\x86_64-pc-windows-msvc\\bin\\codex.exe')));
  assert.ok(seen.claude.includes('C:\\Users\\Test User\\.local\\bin\\claude.exe'));
  assert.equal(seen.codex.some(path => path.endsWith('.cmd') || path.startsWith('/Applications')), false);
});

test('a package for another OS is refused even on the same CPU', async t => {
  const { prebuiltIn } = await import('../scripts/setup/prebuilt.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-windows-prebuilt-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'prebuilt'));
  await writeFile(join(root, 'prebuilt/manifest.json'), JSON.stringify({ arch: process.arch,
    platform: process.platform === 'win32' ? 'darwin' : 'win32', files: {} }));
  const release = prebuiltIn(root);
  await assert.rejects(release.available(), /platform|operating system/);
  await assert.rejects(release.available(), /platform|operating system/);
});

test('Windows secures data before provider files and aborts on ACL failure', async t => {
  const { install } = await import('../scripts/setup/install.mjs');
  const { access } = await import('node:fs/promises');
  const root = await mkdtemp(join(tmpdir(), 'mc-windows-install-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  let secured = false;
  const options = { plan: planSetup({ port: 8787, providers: { codex: true, claude: false }, vmEnabled: false }, host),
    destination: join(root, 'install'), cache: join(root, 'cache'), codexBin: process.execPath,
    approved: true, checkPort: async () => {}, freeBytes: async () => host.freeBytes,
    download: async () => ({ path: join(root, 'unused-before-ACL-check.zip') }),
    buildHost: async path => mkdir(path) };
  await assert.rejects(install({ ...options, secureData: async (runtime, data) => {
    secured = true;
    await assert.rejects(access(join(data, 'providers.json')), { code: 'ENOENT' });
    throw new Error('ACL refused');
  } }), /ACL refused/);
  assert.equal(secured, true);
  await assert.rejects(access(options.destination), { code: 'ENOENT' });
});

test('Windows update leaves the page intact while the backend is running',
  { skip: process.platform !== 'win32' }, async t => {
    const { copyFile, readFile } = await import('node:fs/promises');
    const { spawn } = await import('node:child_process');
    const { once } = await import('node:events');
    const { refreshRuntime } = await import('../scripts/setup/update-runtime.mjs');
    const root = await mkdtemp(join(tmpdir(), 'mc-windows-locked-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const runtime = join(root, 'install/runtime');
    const bundle = join(root, 'bundle');
    for (const directory of [runtime, bundle]) {
      await mkdir(join(directory, 'bin'), { recursive: true });
      await mkdir(join(directory, 'web'));
      await writeFile(join(directory, 'web/index.html'), directory);
      await writeFile(join(directory, 'manifest.json'), '{}');
      await writeFile(join(directory, 'start-host.mjs'), '');
      await copyFile(process.execPath, join(directory, 'bin/miniclaw-server.exe'));
      await mkdir(join(directory, 'bin/shell'));
      for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(directory, 'bin/shell', name), directory === runtime ? 'fixture' : 'replacement');
      await mkdir(join(directory, 'bin/conpty'));
      for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(directory, 'bin/conpty', name), directory === runtime ? 'fixture' : 'replacement');
    }
    // A complete Windows bundle names its architecture and carries the bundled Git archive.
    const { windowsGitBundle } = await import('../scripts/setup/windows-git-bundle.mjs');
    await writeFile(join(bundle, 'manifest.json'), JSON.stringify({ arch: process.arch }));
    await mkdir(join(bundle, 'bin/git'));
    await writeFile(join(bundle, 'bin/git', windowsGitBundle(process.arch).file), 'replacement');
    const child = spawn(join(runtime, 'bin/miniclaw-server.exe'), ['-e', 'setInterval(() => {}, 1000)']);
    await once(child, 'spawn');
    try {
      await assert.rejects(refreshRuntime({ destination: join(root, 'install'), bundle }), /Stop MiniClaw/);
      assert.equal(await readFile(join(runtime, 'web/index.html'), 'utf8'), runtime);
      for (const name of ['conpty.dll', 'OpenConsole.exe']) assert.equal(await readFile(join(runtime, 'bin/conpty', name), 'utf8'), 'fixture');
      await assert.rejects(readFile(join(runtime, 'bin/git', windowsGitBundle(process.arch).file)), { code: 'ENOENT' });
    } finally {
      child.kill();
      await once(child, 'close');
    }
  });

test('Windows ARM64 plans retain native architecture and reject mismatched helper verification', () => {
  const arm = { ...host, arch: 'arm64' };
  const input = { port: 8787, providers: { codex: true, claude: false }, vmEnabled: false };
  assert.equal(planSetup(input, arm).arch, 'arm64');
  assert.throws(() => planSetup({ ...input, windowsComputerHelper: 'C:\\helper.exe' }, arm), /verified/);
});

test('Windows helper inspection requires its native name and valid OpenAI signature', async t => {
  const { inspectWindowsComputerHelper } = await import('../scripts/setup/windows-computer.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-computer-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'codex-computer-use-arm64.exe');
  await writeFile(binary, 'fixture');
  const inspect = result => inspectWindowsComputerHelper(binary, { verify: async () => result });
  await assert.rejects(inspect({ valid: false, subject: 'O=OpenAI, Inc.' }), /signature/);
  await assert.rejects(inspect({ valid: true, subject: 'O=Untrusted Corp' }), /OpenAI/);
  await assert.rejects(inspect({ valid: true, subject: 'O=OpenAI Unrelated Ltd' }), /OpenAI/);
  const verified = await inspect({ valid: true, subject: 'CN=OpenAI, O=OpenAI, L=San Francisco' });
  assert.equal(verified.path, await realpath(binary));
  assert.equal(verified.bytes, 7);
});


test('helper signature check limits execution policy change to its verification process', async () => {
  const verifier = await readFile(new URL('../scripts/setup/windows-computer.mjs', import.meta.url), 'utf8');
  const script = await readFile(new URL('../scripts/setup/windows-computer-signature.ps1', import.meta.url), 'utf8');
  assert.match(verifier, /'-ExecutionPolicy', 'Bypass'/);
  assert.match(script, /Get-AuthenticodeSignature -LiteralPath \$HelperPath/);
  assert.doesNotMatch(script, /Set-ExecutionPolicy|Invoke-Expression|iex\b/);
});

test('native Windows verifier runs under default script policy and rejects unsigned helpers', { skip: process.platform !== 'win32' }, async t => {
  const { inspectWindowsComputerHelper } = await import('../scripts/setup/windows-computer.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-unsigned-helper-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'codex-computer-use.exe');
  await writeFile(path, 'unsigned fixture');
  await assert.rejects(inspectWindowsComputerHelper(path), /no valid Authenticode signature/);
});

test('a rejected helper says what the right file looks like and names its guide', async () => {
  const { inspectWindowsComputerHelper } = await import('../scripts/setup/windows-computer.mjs');
  const error = await inspectWindowsComputerHelper('C:\\Apps\\Codex.exe').catch(error => error);
  assert.equal(error.guide, 'computer-use');
  assert.match(error.message, /codex-computer-use-arm64\.exe or codex-computer-use\.exe/);
  assert.match(error.message, /miniclaw guide computer-use/);
});
