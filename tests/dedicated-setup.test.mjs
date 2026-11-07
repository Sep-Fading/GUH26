// Verifies fresh project guests use only approved installation inputs.
import assert from 'node:assert/strict';
// Runs isolated setup calls without launching macOS.
import { test } from 'node:test';
// Names one persistent VM across separate installation attempts.
import { randomUUID } from 'node:crypto';
// Owns only temporary guest and media fixtures.
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
// Places fixtures away from the retained installation.
import { tmpdir } from 'node:os';
// Resolves fresh attempt paths for comparison.
import { join } from 'node:path';
// Exercises the native install orchestration with stub commands.
import { prepareDedicatedGuest, packageDedicatedSupport } from '../scripts/vm/dedicated-setup.mjs';

test('each project guest installs from the cached restore without Master data', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-fresh-guest-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const masterGuest = join(root, 'master', 'Guest');
  await mkdir(masterGuest, { recursive: true });
  const vmId = randomUUID();
  const calls = [], media = [], progress = [];
  const pair = { root, master: { binary: '/trusted/master-vm', guest: masterGuest },
    provisioning: { restoreImage: '/cache/macos.ipsw',
      codexArchive: '/cache/codex.tar.gz', workerBinary: '/trusted/miniclaw-worker',
      computerApp: '/trusted/Codex Computer Use.app',
      resources: { cpuCount: 2, memoryGiB: 4, diskGiB: 64 } } };
  const prepare = async attempt => prepareDedicatedGuest(pair,
    { vmId, slot: 0, key: 'a'.repeat(64), attempt }, {
      verify: async path => { assert.match(path, /^\/cache\//); },
      media: async input => { media.push(input); return { image: join(input.output, 'Setup.dmg') }; },
      runNative: async (binary, args, options) => {
        calls.push({ binary, args });
        if (args[0] === 'install') options.onOutput('{"status":"installing","percent":42}\n');
      },
      report: async (stage, percent) => progress.push([stage, percent]),
    });
  const first = await prepare(1);
  const second = await prepare(2);
  assert.notEqual(first.guest, second.guest);
  assert.equal(first.guest.startsWith(join(root, 'dedicated-vms', vmId)), true);
  assert.equal(second.guest.startsWith(join(root, 'dedicated-vms', vmId)), true);
  assert.equal(calls.filter(call => call.args[0] === 'init').length, 2);
  assert.equal(calls.filter(call => call.args[0] === 'install').length, 2);
  assert.equal(calls.some(call => call.args.includes(masterGuest)), false);
  assert.equal(calls.every(call => call.binary === pair.master.binary), true);
  assert.equal(media.every(input => input.worker.settings.key === 'a'.repeat(64)), true);
  assert.equal(media.every(input => input.worker.settings.cwd ===
    '/Users/Shared/MiniClaw/Workspace'), true);
  // A project VM gets the same Computer Use helper as the Master VM.
  assert.equal(media.every(input => input.computerApp === '/trusted/Codex Computer Use.app'), true);
  assert.equal(progress.some(([stage, percent]) => stage === 'installing' && percent === 42), true);
});

test('installed runtime includes its own fresh-media dependencies', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-dedicated-package-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const worker = join(root, 'built-worker');
  await writeFile(worker, 'worker');
  const packaged = await packageDedicatedSupport(root,
    new URL('../scripts/', import.meta.url), worker);
  assert.equal(packaged, join(root, 'runtime/miniclaw-worker'));
  assert.equal(await readFile(packaged, 'utf8'), 'worker');
  const module = await import(`file://${join(root, 'runtime/dedicated-setup.mjs')}`);
  assert.equal(typeof module.prepareDedicatedGuest, 'function');
});
