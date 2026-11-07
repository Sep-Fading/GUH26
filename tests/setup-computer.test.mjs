// Verifies source validation and storage accounting.
import assert from 'node:assert/strict';
// Runs helper inspection without changing permissions.
import { test } from 'node:test';
// Builds isolated unsigned application-shaped test fixtures.
import { mkdtemp, mkdir, writeFile, symlink, rm } from 'node:fs/promises';
// Locates disposable directories outside installed helpers.
import { tmpdir } from 'node:os';
// Resolves local fixture application and payload paths.
import { join } from 'node:path';
// Inspects only the explicitly selected helper.
import { inspectComputerApp } from '../scripts/vm/computer-source.mjs';
// Migrates and passes on the saved Windows helper choice.
import { windowsHelperSetting, helperEnvironment, readSettings } from '../scripts/start-host.mjs';
// Reads back migrated settings files.
import { readFile, stat } from 'node:fs/promises';

test('helper size counts app files without following links', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-helper-size-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const app = join(root, 'Helper.app');
  await mkdir(app);
  await writeFile(join(app, 'binary'), Buffer.alloc(100));
  await symlink('binary', join(app, 'alias'));
  let verified;
  const result = await inspectComputerApp(app, { verify: async path => { verified = path; } });
  assert.equal(verified, app);
  assert.equal(result.path, app);
  assert.equal(result.bytes, 106);
  await symlink(app, join(root, 'linked.app'));
  await assert.rejects(inspectComputerApp(join(root, 'linked.app')), /directory/i);
  await assert.rejects(inspectComputerApp('relative.app'), /absolute/i);
});

test('unsigned helper is rejected before it can enter a plan', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-helper-unsigned-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await assert.rejects(inspectComputerApp(root), /signature/i);
});

test('a wrong Computer Use app path names its guide', async () => {
  const error = await inspectComputerApp('relative.app').catch(error => error);
  assert.equal(error.guide, 'computer-use');
  assert.match(error.message, /Codex Computer Use\.app/);
});

const family = 'OpenAI.Codex_2p2nqsd0c76g0';
const versioned = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0\\app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows\\codex-computer-use-arm64.exe';

test('a saved helper path inside the Codex app becomes its package family and file name', () => {
  const migrated = { enabled: true, packageFamily: family, file: 'codex-computer-use-arm64.exe' };
  assert.deepEqual(windowsHelperSetting(versioned), migrated);
  assert.deepEqual(windowsHelperSetting(versioned.replace('@oai\\cua', '@oai\\sky').replace('_arm64_', '_x64_')
    .replace('codex-computer-use-arm64.exe', 'codex-computer-use.exe')), { ...migrated, file: 'codex-computer-use.exe' });
  // Already migrated, absent, or not inside a Codex package: kept as saved.
  assert.deepEqual(windowsHelperSetting(migrated), migrated);
  assert.equal(windowsHelperSetting(null), null);
  assert.equal(windowsHelperSetting(undefined), null);
  for (const path of ['D:\\Tools\\codex-computer-use.exe',
    'C:\\Program Files\\WindowsApps\\Other.App_1.0.0.0_arm64__2p2nqsd0c76g0\\codex-computer-use-arm64.exe',
    'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0\\app\\other.exe']) {
    assert.equal(windowsHelperSetting(path), path);
  }
});

test('the backend gets the package family, or an explicit path, or nothing', () => {
  assert.deepEqual(helperEnvironment({ enabled: true, packageFamily: family, file: 'codex-computer-use-arm64.exe' }),
    { MINICLAW_HELPER_PACKAGE: family, MINICLAW_HELPER_FILE: 'codex-computer-use-arm64.exe' });
  assert.deepEqual(helperEnvironment('D:\\Tools\\codex-computer-use.exe'), { MINICLAW_HELPER_BIN: 'D:\\Tools\\codex-computer-use.exe' });
  assert.deepEqual(helperEnvironment(null), {});
  assert.deepEqual(helperEnvironment({ enabled: false, packageFamily: family, file: 'codex-computer-use-arm64.exe' }), {});
  // Only the Codex app's family and the helper's own names reach the backend.
  assert.throws(() => helperEnvironment({ enabled: true, packageFamily: 'Other.App_2p2nqsd0c76g0', file: 'codex-computer-use-arm64.exe' }), /helper/);
  assert.throws(() => helperEnvironment({ enabled: true, packageFamily: family, file: 'cmd.exe' }), /helper/);
});

test('loading settings migrates a saved versioned helper path once and keeps everything else', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-helper-settings-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const file = join(root, 'settings.json');
  const before = { schemaVersion: 1, port: 8787, backend: 'host', hostReady: true, codexBin: 'C:\\Codex\\codex.exe',
    windowsComputerHelper: versioned, windowsNetwork: { serviceName: 'MiniClawNetwork-x' } };
  await writeFile(file, `${JSON.stringify(before, null, 2)}\n`);
  const loaded = await readSettings(file);
  const expected = { ...before, windowsComputerHelper: { enabled: true, packageFamily: family, file: 'codex-computer-use-arm64.exe' } };
  assert.deepEqual(loaded, expected);
  assert.deepEqual(JSON.parse(await readFile(file, 'utf8')), expected, 'the migrated form is saved');
  const saved = await stat(file);
  assert.deepEqual(await readSettings(file), expected);
  assert.equal((await stat(file)).mtimeMs, saved.mtimeMs, 'a migrated file is not written again');
  await assert.rejects(stat(`${file}.tmp`), { code: 'ENOENT' });
});
