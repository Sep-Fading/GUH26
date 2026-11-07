// Checks rebuilding the VM setup disk for an installation.
import assert from 'node:assert/strict';
// Runs isolated setup disk update cases.
import { test } from 'node:test';
// Creates disposable installation fixtures and files.
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
// Resolves paths inside each isolated fixture.
import { join } from 'node:path';
// Locates temporary storage outside real installations.
import { tmpdir } from 'node:os';
// Rebuilds the setup disk under test.
import { updateMasterMedia } from '../scripts/vm/update-media.mjs';

const key = 'ab'.repeat(32);

// Creates an installed VM folder with an existing setup disk.
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-update-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'master-setup'));
  await mkdir(join(root, 'runtime'));
  await writeFile(join(root, 'master-setup/MasterSetup.dmg'), 'old disk');
  await writeFile(join(root, 'runtime/miniclaw-worker'), 'old worker');
  await writeFile(join(root, 'worker.json'),
    JSON.stringify({ key, cwd: '/Users/Shared/MiniClaw/Workspace' }));
  await writeFile(join(root, 'settings.json'), JSON.stringify({ vmReady: true,
    pair: { provisioning: { restoreImage: '/cache/macos.ipsw' } },
    downloads: [{ path: '/cache/macos.ipsw' }, { path: '/cache/codex-package-aarch64-apple-darwin.tar.gz' }] }));
  const worker = join(root, 'built-worker');
  await writeFile(worker, 'new worker');
  return { root, worker };
}

test('the new setup disk keeps the pairing key and replaces the old one without a copy', async t => {
  const { root, worker } = await fixture(t);
  let built;
  const result = await updateMasterMedia({ destination: root, vmRunning: async () => false,
    buildWorker: async () => worker, now: () => 42,
    buildMedia: async input => {
      built = input;
      await mkdir(input.output);
      await writeFile(join(input.output, 'MasterSetup.dmg'), 'new disk');
    } });
  assert.equal(built.worker.settings.key, key);
  assert.equal(built.worker.binary, worker);
  assert.equal(built.archive, '/cache/codex-package-aarch64-apple-darwin.tar.gz');
  assert.equal(await readFile(join(root, 'master-setup/MasterSetup.dmg'), 'utf8'), 'new disk');
  // An update takes no extra disk space: no backup, no leftover staging folder, no second VM.
  assert.deepEqual((await readdir(root)).sort(),
    ['built-worker', 'master-setup', 'runtime', 'settings.json', 'worker.json']);
  assert.equal(await readFile(join(root, 'runtime/miniclaw-worker'), 'utf8'), 'new worker');
  assert.equal(result.media, join(root, 'master-setup/MasterSetup.dmg'));
});

test('a running VM or a failed build leaves the setup disk alone', async t => {
  const { root, worker } = await fixture(t);
  await assert.rejects(updateMasterMedia({ destination: root, vmRunning: async () => true,
    buildWorker: async () => worker, buildMedia: async () => assert.fail('must not build') }),
  /Shut the VM down first/);
  await assert.rejects(updateMasterMedia({ destination: root, vmRunning: async () => false,
    buildWorker: async () => worker, buildMedia: async input => {
      await mkdir(input.output);
      throw new Error('disk build failed');
    } }), /disk build failed/);
  assert.equal(await readFile(join(root, 'master-setup/MasterSetup.dmg'), 'utf8'), 'old disk');
  assert.deepEqual((await readdir(root)).filter(name => name.startsWith('master-setup')), ['master-setup']);
});

test('the update can add the Computer Use helper and remembers it for later updates', async t => {
  const { root, worker } = await fixture(t);
  const app = join(root, 'Codex Computer Use.app');
  await mkdir(app);
  const builds = [];
  const build = async input => {
    builds.push(input.computerApp);
    await mkdir(input.output);
    await writeFile(join(input.output, 'MasterSetup.dmg'), 'new disk');
  };
  let tick = 0;
  const options = { destination: root, vmRunning: async () => false,
    buildWorker: async () => worker, buildMedia: build, now: () => ++tick };
  await updateMasterMedia({ ...options, computerApp: app });
  const settings = JSON.parse(await readFile(join(root, 'settings.json'), 'utf8'));
  assert.equal(settings.computer.path, app);
  assert.equal(settings.masterSetup.computerUse, true);
  assert.equal(settings.pair.provisioning.computerApp, app);
  // A later plain update keeps the helper on the disk.
  await updateMasterMedia(options);
  assert.deepEqual(builds, [app, app]);
  await assert.rejects(updateMasterMedia({ ...options, computerApp: join(root, 'missing.app') }),
    /Computer Use application was not found/);
});
