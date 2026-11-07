import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { refreshRuntime } from '../scripts/setup/update-runtime.mjs';
// Names the bundled Git archive an update installs.
import { windowsGitBundle } from '../scripts/setup/windows-git-bundle.mjs';

const gitArch = process.arch === 'x64' ? 'x64' : 'arm64';
const otherArch = gitArch === 'x64' ? 'arm64' : 'x64';
// An installed Windows runtime's manifest names its architecture, as backend-package writes it.
const installedWindows = JSON.stringify({ version: '0.0.1', platform: 'win32', arch: gitArch });
// A minimal PE header naming one machine, for installations whose manifest predates the arch field.
function peHeader(arch) {
  const bytes = Buffer.alloc(128); bytes.write('MZ', 0, 'ascii'); bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64); bytes.writeUInt16LE({ x64: 0x8664, arm64: 0xaa64 }[arch], 68);
  return bytes;
}
// Gives runtime and bundle the complete Windows native file set.
async function windowsFiles(runtime, bundle) {
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    await writeFile(join(base, 'bin/miniclaw-server.exe'), label);
    await mkdir(join(base, 'bin/shell'));
    for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(base, 'bin/shell', name), label);
    await mkdir(join(base, 'bin/conpty'));
    for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(base, 'bin/conpty', name), label);
  }
  await withGit(bundle);
}
// Adds the bundled Git archive and licence texts to a Windows bundle and names its architecture.
async function withGit(bundle, label = 'new') {
  await mkdir(join(bundle, 'bin/git/licenses'), { recursive: true });
  await writeFile(join(bundle, 'bin/git', windowsGitBundle(gitArch).file), `${label} git`);
  await writeFile(join(bundle, 'bin/git/licenses/NOTICE.txt'), `${label} notice`);
  const manifest = JSON.parse(await readFile(join(bundle, 'manifest.json'), 'utf8'));
  await writeFile(join(bundle, 'manifest.json'), JSON.stringify({ ...manifest, platform: 'win32', arch: gitArch }));
}
// Records the Git switch an update performs instead of installing a real runtime.
function gitRecorder(events) {
  return async ({ runtime, data, arch }) => {
    events.push(`git:${arch}`);
    assert.equal(await readFile(join(runtime, 'bin/git', windowsGitBundle(arch).file), 'utf8'), 'new git', 'Git installs from the new bundle copy');
    assert.equal(data.endsWith('data'), true);
    return { rollback: async () => events.push('git-rollback'), commit: async () => events.push('git-commit') };
  };
}

const scripts = fileURLToPath(new URL('../scripts/', import.meta.url));
// Microsoft's Visual C++ runtime is already installed; the update installs nothing extra.
const runtimePresent = async () => ({ installed: false, version: '14.51.36247.0', restart: false });

// Builds an installation with an old bundle and a fresh bundle beside it.
async function fixture(t, { vm = true } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'mc-update-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'install');
  const runtime = join(destination, 'runtime');
  await mkdir(join(runtime, 'bin'), { recursive: true, mode: 0o700 });
  await mkdir(join(runtime, 'web/assets'), { recursive: true });
  await writeFile(join(runtime, 'bin/miniclaw-server'), 'old server');
  await writeFile(join(runtime, 'web/index.html'), 'old page');
  await writeFile(join(runtime, 'web/assets/old.js'), 'old asset');
  await writeFile(join(runtime, 'start-host.mjs'), 'old launcher');
  await writeFile(join(runtime, 'manifest.json'), '{"version":"0.0.1"}');
  await writeFile(join(runtime, 'miniclaw-worker'), 'worker stays');
  if (vm) {
    await mkdir(join(runtime, 'vm'), { mode: 0o700 });
    await writeFile(join(runtime, 'vm/session.mjs'), 'old session');
    await writeFile(join(runtime, 'vm/control.mjs'), 'old control');
    await writeFile(join(runtime, 'install-master.command'), 'old installer');
    await writeFile(join(runtime, 'dedicated-setup.mjs'), 'old project setup');
  }
  const bundle = join(root, 'bundle');
  await mkdir(join(bundle, 'bin'), { recursive: true });
  await mkdir(join(bundle, 'web/assets'), { recursive: true });
  await writeFile(join(bundle, 'bin/miniclaw-server'), 'new server');
  await writeFile(join(bundle, 'web/index.html'), 'new page');
  await writeFile(join(bundle, 'web/assets/new.js'), 'new asset');
  await writeFile(join(bundle, 'start-host.mjs'), 'new launcher');
  await writeFile(join(bundle, 'manifest.json'), '{"version":"0.0.2"}');
  return { destination, runtime, bundle };
}
const text = path => readFile(path, 'utf8');

test('update replaces the backend, the page and the VM scripts in place', async t => {
  const { destination, runtime, bundle } = await fixture(t);
  const result = await refreshRuntime({ destination, bundle, scripts, platform: 'darwin' });
  assert.equal(await text(join(runtime, 'bin/miniclaw-server')), 'new server');
  assert.equal(await text(join(runtime, 'web/index.html')), 'new page');
  assert.deepEqual(await readdir(join(runtime, 'web/assets')), ['new.js'], 'old assets are gone');
  assert.equal(await text(join(runtime, 'start-host.mjs')), 'new launcher');
  assert.equal(await text(join(runtime, 'manifest.json')), '{"version":"0.0.2"}');
  for (const name of ['vm/session.mjs', 'vm/control.mjs', 'vm/ledger.mjs']) {
    assert.equal(await text(join(runtime, name)), await text(join(scripts, name)), name);
  }
  for (const name of ['install-master.command', 'dedicated-setup.mjs']) {
    assert.equal(await text(join(runtime, name)), await text(join(scripts, 'vm', name)), name);
  }
  assert.equal(await text(join(runtime, 'miniclaw-worker')), 'worker stays');
  assert.equal(result.vm, true);
});

test('update keeps no backup and leaves no temporary file', async t => {
  const { destination, runtime, bundle } = await fixture(t);
  await refreshRuntime({ destination, bundle, scripts, platform: 'darwin' });
  const names = [...await readdir(runtime), ...await readdir(join(runtime, 'bin'))];
  assert.deepEqual(names.filter(name => /\.(new|old|before)|backup/.test(name)), []);
});

test('an installation without a VM gets no VM scripts', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  const result = await refreshRuntime({ destination, bundle, scripts, platform: 'darwin' });
  assert.equal(result.vm, false);
  assert.equal((await readdir(runtime)).includes('vm'), false);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server')), 'new server');
});

test('update refuses a folder that is not an installation and an incomplete bundle', async t => {
  const { destination, runtime, bundle } = await fixture(t);
  await assert.rejects(refreshRuntime({ destination: join(destination, 'nowhere'), bundle, scripts, platform: 'darwin' }),
    /not a MiniClaw installation/);
  await rm(join(bundle, 'web/index.html'));
  await assert.rejects(refreshRuntime({ destination, bundle, scripts, platform: 'darwin' }), /incomplete/);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server')), 'old server', 'nothing changed');
});

// Updating the host must also update its protected Windows isolation service.
test('Windows refresh updates service before publishing assets and version', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  for (const base of [runtime, bundle]) await writeFile(join(base, 'bin/miniclaw-server.exe'), base === runtime ? 'old server' : 'new server');
  await mkdir(join(bundle, 'bin/shell'));
  for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(bundle, 'bin/shell', name), `new ${name}`);
  await mkdir(join(bundle, 'bin/conpty'));
  for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(bundle, 'bin/conpty', name), `new ${name}`);
  await withGit(bundle);
  const events = [];
  let called = false;
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder(events), updateNetwork: async options => {
    called = true;
    assert.equal(options.runtime, runtime);
    assert.equal(options.data, join(destination, 'data'));
    assert.equal(await text(join(runtime, 'web/index.html')), 'old page');
    assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'new server', 'the service is pinned to the new backend');
    throw new Error('Service drain failed');
  } }), /Service drain failed/);
  assert.ok(called);
  assert.equal(await text(join(runtime, 'manifest.json')), installedWindows);
  // A failed or declined service update must not leave a new backend beside the old service.
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'old server');
  assert.deepEqual((await readdir(join(runtime, 'bin'))).filter(name => /conpty|shell|git/.test(name)), []);
  assert.deepEqual((await readdir(runtime)).filter(name => name.startsWith('.native-update-')), []);
  // Git was switched inside the same group and switched back when the service update failed.
  assert.deepEqual(events, [`git:${gitArch}`, 'git-rollback']);
  events.length = 0;
  await refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder(events), updateNetwork: async () => { events.push('network'); } });
  assert.deepEqual(events, [`git:${gitArch}`, 'network', 'git-commit']);
  assert.equal(await text(join(runtime, 'bin/git', windowsGitBundle(gitArch).file)), 'new git');
  assert.equal(await text(join(runtime, 'bin/git/licenses/NOTICE.txt')), 'new notice');
  assert.equal(JSON.parse(await text(join(runtime, 'manifest.json'))).version, '0.0.2');
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'new server');
  for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) assert.equal(await text(join(runtime, 'bin/conpty', name)), `new ${name}`);
  assert.equal(await text(join(runtime, 'web/index.html')), 'new page');
  assert.deepEqual(await readdir(join(runtime, 'web/assets')), ['new.js']);
  assert.equal(await text(join(runtime, 'start-host.mjs')), 'new launcher');
  assert.equal((await readdir(runtime)).includes('vm'), false);
});

// Missing terminal files must stop an update before replacing the server.
test('Windows refresh rejects missing ConPTY before mutation', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  for (const base of [runtime, bundle]) await writeFile(join(base, 'bin/miniclaw-server.exe'), base === runtime ? 'old' : 'new');
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateNetwork: async () => {} }), /incomplete.*conpty/i);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'old');
});

// The bundled Git runtime is part of a Windows bundle; an update without it changes nothing.
test('Windows refresh rejects a bundle without its Git runtime before mutation', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  for (const base of [runtime, bundle]) await writeFile(join(base, 'bin/miniclaw-server.exe'), base === runtime ? 'old' : 'new');
  await mkdir(join(bundle, 'bin/shell'));
  for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(bundle, 'bin/shell', name), 'new');
  await mkdir(join(bundle, 'bin/conpty'));
  for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(bundle, 'bin/conpty', name), 'new');
  await writeFile(join(bundle, 'manifest.json'), JSON.stringify({ version: '0.0.2', platform: 'win32', arch: gitArch }));
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: async () => assert.fail('no Git'),
    updateNetwork: async () => assert.fail('no service') }), /incomplete.*git/i);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'old');
});

// A failed Git switch restores the backend, terminal and shell, and leaves the service alone.
test('Windows refresh rolls back native files when the Git switch fails', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    await writeFile(join(base, 'bin/miniclaw-server.exe'), label);
    await mkdir(join(base, 'bin/shell'));
    for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(base, 'bin/shell', name), label);
    await mkdir(join(base, 'bin/conpty'));
    for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(base, 'bin/conpty', name), label);
  }
  await withGit(bundle);
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: async () => { throw new Error('Git archive checksum mismatch'); },
    updateNetwork: async () => assert.fail('the service is not touched') }), /checksum/);
  for (const name of ['miniclaw-server.exe', 'conpty/conpty.dll', 'shell/busybox.exe']) assert.equal(await text(join(runtime, 'bin', name)), 'old');
  assert.deepEqual((await readdir(join(runtime, 'bin'))).filter(name => name === 'git'), []);
  assert.equal(await text(join(runtime, 'manifest.json')), installedWindows);
});

// A failed terminal replacement restores the backend and every earlier runtime file.
test('Windows runtime replacement rolls back on terminal failure', async t => {
  const { copyFile } = await import('node:fs/promises');
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    await writeFile(join(base, 'bin/miniclaw-server.exe'), label);
    await mkdir(join(base, 'bin/shell'));
    for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(base, 'bin/shell', name), label);
    await mkdir(join(base, 'bin/conpty'));
    for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(base, 'bin/conpty', name), label);
  }
  await withGit(bundle);
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateNetwork: async () => {},
    updateGit: async () => assert.fail('Git is switched only after every file was replaced'),
    replace: async (source, target) => {
      if (target.endsWith('OpenConsole.exe')) throw new Error('terminal locked');
      await copyFile(source, target);
    } }), /terminal locked/);
  for (const name of ['miniclaw-server.exe', 'conpty/conpty.dll', 'conpty/OpenConsole.exe']) {
    assert.equal(await text(join(runtime, 'bin', name)), 'old');
  }
  assert.equal(await text(join(runtime, 'manifest.json')), installedWindows);
});

// The service update runs before the new manifest is published, so the backend it copies must be
// pinned to the new bundle's manifest: pinning against the installed (old) manifest refused every
// Windows update whose backend changed, and an unpinned copy would trust any backend.
test('Windows refresh pins the service update to the new bundle manifest', async t => {
  const { createHash } = await import('node:crypto');
  const { pinnedBackend } = await import('../scripts/setup/windows-network.mjs');
  const { rename } = await import('node:fs/promises');
  const { destination, runtime, bundle: outside } = await fixture(t, { vm: false });
  // `miniclaw update` builds its bundle inside the installation, as here.
  const bundle = join(destination, '.update-fixture');
  await rename(outside, bundle);
  const sha = text => createHash('sha256').update(text).digest('hex');
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    await writeFile(join(base, 'bin/miniclaw-server.exe'), `${label} server`);
    await writeFile(join(base, 'manifest.json'), JSON.stringify({ version: label, platform: 'win32', arch: gitArch, files: { 'bin/miniclaw-server.exe': sha(`${label} server`) } }));
    await mkdir(join(base, 'bin/shell'));
    for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(base, 'bin/shell', name), label);
    await mkdir(join(base, 'bin/conpty'));
    for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(base, 'bin/conpty', name), label);
  }
  await withGit(bundle);
  let pinned;
  // Mirrors updateWindowsNetwork: it pins with the options refreshRuntime hands it.
  await refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder([]), updateNetwork: async ({ runtime: installed, manifest }) => {
    pinned = await pinnedBackend(installed, { manifest, verify: async () => {} });
  } });
  assert.deepEqual(pinned, { source: join(runtime, 'bin', 'miniclaw-server.exe'), sha256: sha('new server') });
  assert.equal(JSON.parse(await text(join(runtime, 'manifest.json'))).version, 'new');
  assert.equal(JSON.parse(await text(join(runtime, 'manifest.json'))).arch, gitArch);
  // A manifest outside the verified installation folder is never trusted.
  await mkdir(outside);
  await writeFile(join(outside, 'manifest.json'), JSON.stringify({ files: { 'bin/miniclaw-server.exe': sha('new server') } }));
  await assert.rejects(pinnedBackend(runtime, { manifest: join(outside, 'manifest.json'), verify: async () => {} }), /installation/);
});

// v3 re-review R2: the bundle's manifest selects the Git archive, so a build for another architecture
// (a release or a checkout build) must never replace this installation's files.
test('Windows refresh refuses a build for another architecture before any change', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), JSON.stringify({ version: '0.0.1', platform: 'win32', arch: otherArch }));
  await windowsFiles(runtime, bundle);
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: async () => assert.fail('no Git'),
    updateNetwork: async () => assert.fail('no service') }), new RegExp(`architecture.*${otherArch}.*${gitArch}`));
  for (const name of ['miniclaw-server.exe', 'conpty/conpty.dll', 'shell/busybox.exe']) assert.equal(await text(join(runtime, 'bin', name)), 'old');
  assert.deepEqual((await readdir(join(runtime, 'bin'))).filter(name => name === 'git'), []);
  assert.equal(JSON.parse(await text(join(runtime, 'manifest.json'))).arch, otherArch);
});

// An installed manifest without the arch field falls back to the installed backend's PE header.
test('Windows refresh reads the installed architecture from the backend when the manifest lacks it', async t => {
  for (const [installed, accepted] of [[peHeader(gitArch), true], [peHeader(otherArch), false], [Buffer.from('not a PE file'), false]]) {
    const { destination, runtime, bundle } = await fixture(t, { vm: false });
    await windowsFiles(runtime, bundle);
    await writeFile(join(runtime, 'bin/miniclaw-server.exe'), installed);
    const events = [];
    const update = refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder(events), updateNetwork: async () => { events.push('network'); } });
    if (accepted) {
      await update;
      assert.deepEqual(events, [`git:${gitArch}`, 'network', 'git-commit']);
      assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'new');
    } else {
      await assert.rejects(update, /architecture/);
      assert.deepEqual(events, []);
      assert.ok((await readFile(join(runtime, 'bin/miniclaw-server.exe'))).equals(installed));
    }
  }
});

// The new backend and Git link vcruntime140.dll, so Microsoft's runtime must be in place before they replace the old ones.
test('Windows refresh ensures the Microsoft runtime for the bundle architecture before replacing any file', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  await windowsFiles(runtime, bundle);
  const events = [];
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32',
    ensureRuntime: async ({ arch }) => { events.push(`runtime:${arch}`); throw new Error('Setup could not install the Microsoft Visual C++ runtime: administrator approval was declined.'); },
    updateGit: async () => assert.fail('no Git switch'), updateNetwork: async () => assert.fail('no service update') }), /administrator approval was declined/);
  assert.deepEqual(events, [`runtime:${gitArch}`]);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'old');
  assert.equal(await text(join(runtime, 'manifest.json')), installedWindows);
  assert.deepEqual((await readdir(runtime)).filter(name => name.startsWith('.native-update-')), []);
  await refreshRuntime({ destination, bundle, platform: 'win32',
    ensureRuntime: async ({ arch }) => { events.push(`runtime:${arch}`); return { installed: true, version: '14.51.36247.0', restart: false }; },
    updateGit: gitRecorder(events), updateNetwork: async () => { events.push('network'); } });
  assert.deepEqual(events, [`runtime:${gitArch}`, `runtime:${gitArch}`, `git:${gitArch}`, 'network', 'git-commit']);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'new');
});

// Every native file of the new bundle belongs to the recovery group, not only the backend, ConPTY, BusyBox and
// Git: the private Node (its seed library is pinned by hash inside the new backend), npm's tree and Python's hook.
// Before this an update left the old seed library beside a backend that expects the new one, and every
// confined MCP server on the private Node stopped at once after `miniclaw update` (VM2, 2026-10-05).
test('Windows refresh replaces the private Node and Python hook in the same recovery group', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  await windowsFiles(runtime, bundle);
  const extra = ['bin/node/node.exe', 'bin/node/miniclaw_openssl_seed.dll', 'bin/node/node_modules/npm/package.json', 'bin/python/sitecustomize.py'];
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    for (const name of extra) { await mkdir(join(base, name, '..'), { recursive: true }); await writeFile(join(base, name), `${label} ${name}`); }
  }
  const manifest = JSON.parse(await text(join(bundle, 'manifest.json')));
  await writeFile(join(bundle, 'manifest.json'), JSON.stringify({ ...manifest, files: Object.fromEntries(['bin/miniclaw-server.exe', ...extra].map(name => [name, 'x'])) }));
  const events = [];
  await assert.rejects(refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder(events),
    updateNetwork: async () => {
      for (const name of extra) assert.equal(await text(join(runtime, name)), `new ${name}`, `${name} is new beside the new backend`);
      throw new Error('Service drain failed');
    } }), /Service drain failed/);
  for (const name of extra) assert.equal(await text(join(runtime, name)), `old ${name}`, `${name} is restored with the backend`);
  await refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder([]), updateNetwork: async () => {} });
  for (const name of extra) assert.equal(await text(join(runtime, name)), `new ${name}`);
});

// A failed update that was undone completely says so: nothing changed, why it failed and what to try. The
// release agent's broken release (its manifest pins another backend) failed this way and was told to reinstall.
test('a fully rolled-back update says nothing changed and what to try, not to reinstall', async t => {
  const { createHash } = await import('node:crypto');
  const { pinnedBackend } = await import('../scripts/setup/windows-network.mjs');
  const { rename } = await import('node:fs/promises');
  const { destination, runtime, bundle: outside } = await fixture(t, { vm: false });
  const bundle = join(destination, '.update-fixture');
  await rename(outside, bundle);
  const sha = text => createHash('sha256').update(text).digest('hex');
  for (const [base, label] of [[runtime, 'old'], [bundle, 'new']]) {
    await writeFile(join(base, 'bin/miniclaw-server.exe'), `${label} server`);
    // The new bundle's manifest pins a backend other than the one it carries.
    await writeFile(join(base, 'manifest.json'), JSON.stringify({ version: label === 'old' ? '0.1.0' : '0.1.1', platform: 'win32', arch: gitArch,
      files: { 'bin/miniclaw-server.exe': label === 'old' ? sha('old server') : '0'.repeat(64) } }));
    await mkdir(join(base, 'bin/shell'));
    for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) await writeFile(join(base, 'bin/shell', name), label);
    await mkdir(join(base, 'bin/conpty'));
    for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) await writeFile(join(base, 'bin/conpty', name), label);
  }
  await withGit(bundle);
  const events = [];
  const failure = await refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent, updateGit: gitRecorder(events),
    updateNetwork: async ({ runtime: installed, manifest }) => { await pinnedBackend(installed, { manifest, verify: async () => {} }); } })
    .then(() => assert.fail('the update must fail'), error => error);
  assert.deepEqual(events, [`git:${gitArch}`, 'git-rollback']);
  assert.equal(await text(join(runtime, 'bin/miniclaw-server.exe')), 'old server');
  assert.match(failure.message, /every change was undone/i);
  assert.match(failure.message, /0\.1\.0/, 'names the version that is still installed');
  assert.match(failure.message, /does not match its manifest/);
  assert.match(failure.message, /extract/i, 'says what to try');
  assert.doesNotMatch(failure.message, /reinstall/i);
  assert.equal(failure.rolledBack, true);
});

// Only when the undo itself failed is the installation in doubt; then reinstalling is the advice.
test('an update whose rollback failed keeps the reinstall advice', async t => {
  const { destination, runtime, bundle } = await fixture(t, { vm: false });
  await writeFile(join(runtime, 'manifest.json'), installedWindows);
  await windowsFiles(runtime, bundle);
  const failure = await refreshRuntime({ destination, bundle, platform: 'win32', ensureRuntime: runtimePresent,
    updateGit: async () => ({ rollback: async () => { throw new Error('Git runtime folder is locked'); }, commit: async () => {} }),
    updateNetwork: async () => { throw new Error('Administrator approval was declined'); } }).then(() => assert.fail('the update must fail'), error => error);
  assert.match(failure.message, /could not be (fully )?undone/i);
  assert.match(failure.message, /Administrator approval was declined/);
  assert.match(failure.message, /Git runtime folder is locked/);
  assert.match(failure.message, /reinstall/i);
  assert.notEqual(failure.rolledBack, true);
});
