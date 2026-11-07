// Checks installation approval, isolation and receipts.
// Names the bundled Windows Git archive setup installs from.
import { windowsGitBundle } from '../scripts/setup/windows-git-bundle.mjs';
import assert from 'node:assert/strict';
// Runs installation cases without real downloads.
import { test } from 'node:test';
// Creates disposable installations and existing markers.
import { copyFile, mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
// Locates temporary storage for installation fixtures.
import { tmpdir } from 'node:os';
// Resolves disposable installation and cache paths.
import { join } from 'node:path';
// Imports the copied VM runtime in a clean process.
import { execFileSync } from 'node:child_process';
// Imports by URL, since Windows ESM refuses a bare drive path; locates the built backend.
import { fileURLToPath, pathToFileURL } from 'node:url';
import { nativeArtifacts } from './support/provider-fixture-paths.mjs';
// Uses real planning with measured-size fixtures.
import { planSetup, GiB } from '../scripts/setup/plan.mjs';
// Runs the installation transaction under test.
import { install, copyVmRuntime } from '../scripts/setup/install.mjs';

const host = { platform: 'darwin', arch: 'arm64', cpuCount: 8,
  memoryBytes: 16 * GiB, diskBytes: 512 * GiB, freeBytes: 300 * GiB };
const choice = { port: 8787, providers: { codex: true, claude: false }, vmEnabled: false, master: null };
// A macOS plan builds no network service and makes owner-only files with mode bits. On Windows the
// backend writes private state and the plan always adds Git and the network service, so these
// cases run on macOS only; the Windows install case below covers the same rules there.
const macOnly = { skip: process.platform === 'win32' && 'macOS plan: Windows installs are covered by "a Windows install secures its data ..."' };

// Returns isolated paths and an approved Mac-only plan.
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-setup-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return { destination: join(root, 'installation'), cache: join(root, 'cache'),
    plan: planSetup(choice, host), codexBin: '/test/codex', claudeBin: '/test/claude', approved: true,
    checkPort: async () => {}, freeBytes: async () => host.freeBytes };
}

test('a Mac-only install builds once, downloads nothing and records the provider choice', macOnly, async t => {
  const options = await fixture(t);
  let builds = 0;
  const result = await install({ ...options,
    buildHost: async path => { builds++; await mkdir(path); },
    download: () => assert.fail('A Mac-only install requested a download'),
    prepareVm: () => assert.fail('A Mac-only install prepared a VM'),
  });
  assert.equal(builds, 1);
  assert.equal(result.hostReady, true);
  assert.equal(result.pair, null);
  const saved = JSON.parse(await readFile(join(options.destination, 'settings.json'), 'utf8'));
  assert.equal(saved.backend, 'host');
  assert.equal(saved.codexBin, '/test/codex');
  assert.equal(saved.claudeBin, null, 'a disabled provider is not wired in');
  assert.deepEqual(saved.providers, { codex: true, claude: false });
  const providers = join(options.destination, 'data/providers.json');
  assert.deepEqual(JSON.parse(await readFile(providers, 'utf8')), { codex: true, claude: false });
  assert.equal((await stat(providers)).mode & 0o077, 0);
  await assert.rejects(readdir(options.cache), { code: 'ENOENT' });
});

test('an enabled provider needs its executable', macOnly, async t => {
  const options = await fixture(t);
  await assert.rejects(install({ ...options, codexBin: null, buildHost: () => assert.fail('built') }), /Codex/);
  const claudeOnly = { ...options, plan: planSetup({ ...choice, providers: { codex: false, claude: true } }, host) };
  await assert.rejects(install({ ...claudeOnly, claudeBin: 'claude', buildHost: () => assert.fail('built') }), /Claude/);
  const result = await install({ ...claudeOnly, codexBin: null, buildHost: async path => mkdir(path) });
  assert.equal(result.codexBin, null);
  assert.equal(result.claudeBin, '/test/claude');
});

test('refused approval makes no changes', async t => {
  const options = await fixture(t);
  await assert.rejects(install({ ...options, approved: false,
    buildHost: () => assert.fail('Build before approval') }), /approval/i);
  await assert.rejects(readdir(options.destination), { code: 'ENOENT' });
});

test('existing installations are left untouched', async t => {
  const options = await fixture(t);
  await mkdir(options.destination);
  await writeFile(join(options.destination, 'keep'), 'existing');
  await assert.rejects(install({ ...options, buildHost: () => assert.fail('Unexpected build') }), /exist/i);
  assert.equal(await readFile(join(options.destination, 'keep'), 'utf8'), 'existing');
});

test('cancelled and failed builds do not create ready installations', async t => {
  const options = await fixture(t);
  await assert.rejects(install({ ...options, buildHost: async path => {
    await mkdir(path);
    throw new Error('Build failed');
  } }), /Build failed/);
  await assert.rejects(readdir(options.destination), { code: 'ENOENT' });
});

test('ports and free space are rechecked before installing', async t => {
  const options = await fixture(t);
  await assert.rejects(install({ ...options, checkPort: async () => { throw new Error('Port occupied'); } }), /occupied/);
  await assert.rejects(install({ ...options, freeBytes: async () => 1 }), /disk/i);
  await assert.rejects(readdir(options.destination), { code: 'ENOENT' });
});

// Windows refuses VM plans (tests/windows-setup.test.mjs).
test('a VM install builds the Mac runtime and prepares a Mac-backend pair', macOnly, async t => {
  const options = await fixture(t);
  const computer = { path: '/test/Codex Computer Use.app', bytes: 100 };
  const plan = planSetup({ ...choice, vmEnabled: true, computer,
    master: { cpuCount: 2, memoryGiB: 4, diskGiB: 64 } }, host,
  [{ id: 'macos', file: 'macos.ipsw', bytes: 10, expandedBytes: 0 }]);
  const order = [];
  const result = await install({ ...options, plan,
    inspectComputer: async () => computer,
    download: async item => { order.push(`download:${item.id}`); return { path: join(options.cache, item.id) }; },
    buildHost: async path => { order.push('build'); await mkdir(path); },
    prepareVm: async ({ destination, plan: selected, downloads, runtime }) => {
      order.push('vm');
      assert.equal(destination, options.destination);
      assert.equal(runtime, join(options.destination, 'runtime'));
      assert.equal(downloads.length, 3);
      assert.deepEqual(selected.computer, computer);
      return { root: destination, port: selected.port, mac: { worker: join(destination, 'worker.json') },
        master: { binary: '/b', guest: '/g', media: '/m' } };
    },
  });
  assert.deepEqual(order, ['download:macos', 'download:codex', 'download:network-go', 'build', 'vm']);
  assert.equal(result.backend, 'host');
  assert.equal(result.hostReady, true);
  assert.equal(result.pair.mac.worker, join(options.destination, 'worker.json'));
  assert.equal(result.pair.controller, undefined, 'no Linux controller exists');
  assert.equal(result.status, 'awaiting-guest-setup');
  assert.deepEqual(result.masterSetup, { image: '/m', status: 'worker-prepared', oauthReady: false, computerUse: true });
  assert.equal(result.vmReady, false);
});

test('changed helper source fails before downloads or directory creation', async t => {
  const options = await fixture(t);
  await assert.rejects(install({ ...options,
    plan: { ...options.plan, computer: { path: '/test/helper.app', bytes: 100 } },
    inspectComputer: async () => ({ path: '/test/helper.app', bytes: 101 }),
    download: () => assert.fail('Download before source verification'),
    buildHost: () => assert.fail('Build before source verification'),
  }), /changed.*review/i);
  await assert.rejects(readdir(options.destination), { code: 'ENOENT' });
});

test('the copied VM runtime resolves every import from the installed layout', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-vm-runtime-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  // The installed runtime sits beside the project's node_modules only in this test.
  const runtime = join(root, 'runtime');
  await mkdir(runtime);
  await copyVmRuntime(runtime);
  const output = execFileSync(process.execPath, ['--input-type=module', '-e',
    `const m = await import(${JSON.stringify(pathToFileURL(join(runtime, 'vm/session.mjs')).href)}); console.log(typeof m.startPair);`],
  { encoding: 'utf8' });
  assert.equal(output.trim(), 'function');
});

// The Windows counterpart of the macOS cases above: provider executables are required, data is
// secured before the provider choice is written through the backend's private-file primitive,
// Git and the network service are installed in order, and the receipt records all of it.
test('a Windows install secures its data, installs Git and the network service, and records the provider choice', async t => {
  const windows = process.platform === 'win32';
  const winHost = { ...host, platform: 'win32', arch: process.arch === 'arm64' ? 'arm64' : 'x64', freeBytes: 100 * GiB };
  const root = await mkdtemp(join(tmpdir(), 'mc-setup-windows-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'installation');
  const cache = join(root, 'cache');
  // The helper the wizard verified, inside the Codex app's versioned package folder.
  const helper = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0\\app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows\\codex-computer-use-arm64.exe';
  const plan = planSetup({ ...choice, windowsComputerHelper: helper, windowsComputer: { path: helper, bytes: 1406256 } }, winHost);
  const stages = [];
  const calls = [];
  const backend = nativeArtifacts(fileURLToPath(new URL('../', import.meta.url)), process.env.MINICLAW_TEST_TARGET).backend;
  const service = { serviceName: `MiniClawNetwork-${'c'.repeat(32)}`, executable: 'C:\\Program Files\\fixture\\miniclaw-network-service.exe' };
  const options = { plan, destination, cache, codexBin: '/test/codex', claudeBin: '/test/claude', approved: true,
    checkPort: async () => {}, freeBytes: async () => winHost.freeBytes, onStage: stage => stages.push(stage),
    inspectWindowsComputer: async path => ({ path, bytes: 1406256 }),
    download: async item => { calls.push(`download:${item.id}`); return { path: join(cache, `${item.id}.zip`) }; },
    // Windows runs the real `--secure-data-dir` and private-file writes of the built backend.
    buildHost: async path => {
      calls.push('build');
      await mkdir(join(path, 'bin'), { recursive: true });
      if (windows) await copyFile(backend, join(path, 'bin', 'miniclaw-server.exe'));
    },
    ...(windows ? {} : { secureData: async (runtime, data) => { calls.push('secure'); await mkdir(data, { mode: 0o700 }); } }),
    // Git comes from the release's own bundle (runtime/bin/git), pinned, never from the download cache.
    installGit: async ({ runtime, data, arch, archive, pin }) => {
      calls.push('git');
      assert.equal(runtime, join(destination, 'runtime'));
      assert.equal(data, join(destination, 'data'));
      assert.equal(arch, winHost.arch);
      assert.deepEqual(pin, windowsGitBundle(winHost.arch));
      assert.equal(archive, join(destination, 'runtime', 'bin', 'git', windowsGitBundle(winHost.arch).file));
    },
    installNetwork: async ({ runtime, data }) => {
      calls.push('network');
      assert.equal(runtime, join(destination, 'runtime'));
      const pending = JSON.parse(await readFile(join(data, 'windows-setup-pending.json'), 'utf8'));
      assert.equal(pending.status, 'awaiting-network-service');
      assert.equal(pending.hostReady, false);
      return service;
    } };
  await assert.rejects(install({ ...options, codexBin: null, buildHost: () => assert.fail('built') }), /Codex/);
  await assert.rejects(readdir(destination), { code: 'ENOENT' });
  const result = await install(options);
  assert.deepEqual(stages, ['build', 'windows-git', 'windows-network']);
  assert.deepEqual(calls.filter(call => call !== 'secure'), ['build', 'git', 'network']);
  assert.equal(result.hostReady, true);
  assert.equal(result.status, 'installed');
  const saved = JSON.parse(await readFile(join(destination, 'settings.json'), 'utf8'));
  assert.equal(saved.backend, 'host');
  assert.equal(saved.codexBin, '/test/codex');
  assert.equal(saved.claudeBin, null, 'a disabled provider is not wired in');
  assert.deepEqual(saved.providers, { codex: true, claude: false });
  assert.deepEqual(saved.windowsNetwork, service);
  // The consent is saved with the Codex app's package family, not the folder that changes with each Codex update.
  assert.deepEqual(saved.windowsComputerHelper, { enabled: true, packageFamily: 'OpenAI.Codex_2p2nqsd0c76g0',
    file: 'codex-computer-use-arm64.exe' });
  assert.equal(saved.vmEnabled, false);
  assert.deepEqual(JSON.parse(await readFile(join(destination, 'data/providers.json'), 'utf8')), { codex: true, claude: false });
  for (const name of ['windows-setup-pending.json', 'windows-network-pending.json']) {
    await assert.rejects(stat(join(destination, 'data', name)), { code: 'ENOENT' }, name);
  }
});
