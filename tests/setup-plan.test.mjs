// Checks recommendations against independent installation requirements.
import assert from 'node:assert/strict';
// Runs deterministic installation planning acceptance cases.
import { test } from 'node:test';
// Loads the installation planner under test.
import { recommend, planSetup, GiB } from '../scripts/setup/plan.mjs';

const host = { platform: 'darwin', arch: 'arm64', cpuCount: 8,
  memoryBytes: 8 * GiB, diskBytes: 256 * GiB, freeBytes: 150 * GiB };
const large = { ...host, cpuCount: 12, memoryBytes: 32 * GiB, diskBytes: 1024 * GiB, freeBytes: 800 * GiB };
const catalog = [{ id: 'macos', bytes: 20 * GiB, expandedBytes: 0 }];
const choice = { port: 8787, providers: { codex: true, claude: true }, vmEnabled: false, master: null,
  limits: { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 } };
const master = { cpuCount: 2, memoryGiB: 4, diskGiB: 64 };

test('the VM is opt-in on every Mac, with a Master size that fits the machine', () => {
  assert.equal(recommend(host).vmEnabled, false);
  assert.equal(recommend(large).vmEnabled, false);
  assert.equal(recommend(host).master.memoryGiB, 4);
  assert.equal(recommend(large).master.memoryGiB, 8);
  assert.equal(recommend(host).masterFits, false);
  assert.equal(recommend(large).masterFits, true);
  assert.equal(recommend(host).port, 8787);
});

test('a plan without a VM has no image or virtual disk work', () => {
  const plan = planSetup(choice, host, catalog);
  assert.deepEqual(plan.images, []);
  assert.deepEqual(plan.packages, []);
  assert.equal(plan.downloadBytes, 0);
  assert.equal(plan.vmDiskBytes, 0);
  assert.equal(plan.backend, 'host');
  assert.equal(plan.url, 'http://127.0.0.1:8787');
  assert.equal(plan.status, 'host');
});

test('the backend always runs on the Mac', () => {
  assert.throws(() => planSetup({ ...choice, backend: 'linux' }, host, catalog), /Mac/);
  assert.throws(() => planSetup({ ...choice, linux: { cpuCount: 1, memoryGiB: 1, diskGiB: 4 } }, host, catalog), /Mac/);
});

test('at least one provider must be enabled', () => {
  assert.throws(() => planSetup({ ...choice, providers: { codex: false, claude: false } }, host, catalog), /provider/i);
  assert.throws(() => planSetup({ ...choice, providers: undefined }, host, catalog), /provider/i);
  assert.doesNotThrow(() => planSetup({ ...choice, providers: { codex: false, claude: true } }, host, catalog));
});

test('the VM choice and the Master allocation must agree', () => {
  assert.throws(() => planSetup({ ...choice, master }, host, catalog), /VM/);
  assert.throws(() => planSetup({ ...choice, vmEnabled: true }, host, catalog), /Master/);
});

test('a VM plan accounts for the restore image, Codex and the network build', () => {
  const plan = planSetup({ ...choice, vmEnabled: true, master,
    limits: { ...choice.limits, diskPercent: 50 } }, host, catalog);
  assert.deepEqual(plan.images.map(image => image.id), ['macos']);
  assert.equal(plan.vmDiskBytes, 64 * GiB);
  assert.equal(plan.retainedImageBytes, 20 * GiB);
  assert.deepEqual(plan.packages.map(item => item.id), ['codex', 'network-go']);
  const go = plan.packages.find(item => item.id === 'network-go');
  assert.equal(go.digest, 'ee215d57e0ec269c60cc9ceca68e6bda321ba9ee5afe24f4b0988703c2d87d12');
  assert.equal(plan.downloadBytes, 20 * GiB + plan.packageDownloadBytes);
  assert.ok(plan.peakBytes >= plan.vmDiskBytes + plan.retainedImageBytes + plan.retainedPackageBytes);
  assert.equal(plan.status, 'vm-preparation');
});

test('resource budgets cannot be exceeded silently', () => {
  const input = { ...choice, vmEnabled: true, master: { ...master, memoryGiB: 6 },
    limits: { ...choice.limits, diskPercent: 50 } };
  assert.throws(() => planSetup(input, host, catalog), /RAM budget/);
  assert.doesNotThrow(() => planSetup({ ...input, limits: { ...input.limits, memoryPercent: 80 } }, host, catalog));
  assert.throws(() => planSetup({ ...choice, vmEnabled: true, master }, host, catalog), /disk budget/);
  assert.throws(() => planSetup({ ...input, master }, { ...host, freeBytes: 4 * GiB }, catalog), /free disk/);
  assert.throws(() => planSetup({ ...input, master: { ...master, diskGiB: 32 } }, host, catalog), /disk/);
});

test('ports and unsupported platforms fail closed', () => {
  for (const port of [0, 80, 65536, 8787.5, '8787', NaN]) {
    assert.throws(() => planSetup({ ...choice, port }, host, catalog), /port/i);
  }
  assert.throws(() => recommend({ ...host, arch: 'x64' }), /Apple silicon/);
});

test('Computer Use requires the VM and reserves local media space', () => {
  const computer = { path: '/test/Codex Computer Use.app', bytes: 100_000_000 };
  assert.throws(() => planSetup({ ...choice, computer }, host, catalog), /Computer Use/);
  const input = { ...choice, vmEnabled: true, master, limits: { ...choice.limits, diskPercent: 50 } };
  const base = planSetup(input, host, catalog);
  const selected = planSetup({ ...input, computer }, host, catalog);
  assert.equal(selected.downloadBytes, base.downloadBytes);
  assert.equal(selected.peakBytes - base.peakBytes, selected.retainedComputerBytes);
  assert.throws(() => planSetup({ ...input, computerApp: computer.path }, host, catalog), /verified/);
});
