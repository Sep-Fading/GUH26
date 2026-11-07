// Checks allocation totals and admission decisions.
import assert from 'node:assert/strict';
// Runs isolated resource policy contract cases.
import { test } from 'node:test';
// Compiles and runs the native calculator.
import { execFileSync } from 'node:child_process';
// Creates and removes disposable compiler artifacts.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates temporary storage outside VM disks.
import { tmpdir } from 'node:os';
// Joins paths for disposable compiler output.
import { join } from 'node:path';
// Resolves native source from this test.
import { fileURLToPath } from 'node:url';

const GiB = 1024 ** 3;
const host = { cpuCount: 10, memoryBytes: 24 * GiB, diskBytes: 1024 * GiB };

// Returns a synthetic VM reservation, never a real guest.
function vm(id, state = 'running', overrides = {}) {
  return { id, state, cpuCount: 2, memoryBytes: 4 * GiB,
    diskBytes: 64 * GiB, ...overrides };
}

test('VM resource budget', { timeout: 120_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-vm-resources-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = join(dir, 'vm-resources');
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/vm-resources.swift', import.meta.url,
  ));
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6',
    '-warnings-as-errors', '-parse-as-library', '-module-cache-path',
    join(dir, 'cache'), source, '-o', binary], { timeout: 90_000 });

  // Evaluates a proposed complete inventory without starting any VM.
  function check(vms, options = {}) {
    return JSON.parse(execFileSync(binary, [], {
      input: JSON.stringify({ host, vms, extraDiskBytes: 0, ...options }),
      timeout: 5_000, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'],
    }));
  }

  await t.test('defaults are global percentages', () => {
    const result = check([]);
    assert.deepEqual(result.limits, { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 });
    assert.deepEqual(result.capacity, {
      cpuCount: 5, memoryBytes: 12 * GiB, diskBytes: 256 * GiB,
    });
    assert.equal(result.allowed, true);
  });

  await t.test('exact capacity is allowed', () => {
    const result = check([vm('master', 'running', {
      cpuCount: 5, memoryBytes: 12 * GiB, diskBytes: 256 * GiB,
    })]);
    assert.equal(result.allowed, true);
    assert.deepEqual(result.exceeded, []);
  });

  for (const [resource, overrides] of [
    ['cpu', { cpuCount: 6 }],
    ['memory', { memoryBytes: 12 * GiB + 1 }],
    ['disk', { diskBytes: 256 * GiB + 1 }],
  ]) {
    await t.test(`${resource} excess is refused`, () => {
      const result = check([vm('master', 'running', overrides)]);
      assert.equal(result.allowed, false);
      assert.deepEqual(result.exceeded, [resource]);
    });
  }

  await t.test('active VMs share one budget', () => {
    const result = check([vm('master'), vm('project'), vm('orphan', 'starting')]);
    assert.equal(result.allowed, false);
    assert.deepEqual(result.exceeded, ['cpu']);
    assert.equal(result.used.cpuCount, 6);
  });

  await t.test('backend and workspace fit the proposed budget', () => {
    const result = check([
      vm('backend', 'starting'),
      vm('workspace', 'running', { memoryBytes: 8 * GiB }),
    ]);
    assert.equal(result.allowed, true);
    assert.deepEqual(result.used, {
      cpuCount: 4, memoryBytes: 12 * GiB, diskBytes: 128 * GiB,
    });
  });

  await t.test('current lab leaves no room for the backend', () => {
    const result = check([
      vm('backend', 'starting'),
      vm('lab', 'running', { cpuCount: 4, memoryBytes: 8 * GiB }),
    ]);
    assert.equal(result.allowed, false);
    assert.deepEqual(result.exceeded, ['cpu']);
  });

  await t.test('Linux backend fits beside the retained macOS lab', () => {
    const result = check([
      vm('linux-backend', 'starting', {
        cpuCount: 1, memoryBytes: 2 * GiB, diskBytes: 16 * GiB,
      }),
      vm('lab', 'running', { cpuCount: 4, memoryBytes: 8 * GiB }),
    ]);
    assert.equal(result.allowed, true);
    assert.deepEqual(result.used, {
      cpuCount: 5, memoryBytes: 10 * GiB, diskBytes: 80 * GiB,
    });
  });

  await t.test('another workspace exceeds the default pair budget', () => {
    const result = check([
      vm('backend'),
      vm('workspace', 'running', { memoryBytes: 8 * GiB }),
      vm('project', 'starting'),
    ]);
    assert.equal(result.allowed, false);
    assert.deepEqual(result.exceeded, ['cpu', 'memory']);
  });

  await t.test('stopped VMs retain disk only', () => {
    const result = check([vm('master'), vm('project', 'stopped'), vm('orphan', 'stopped')]);
    assert.equal(result.allowed, true);
    assert.deepEqual(result.used, {
      cpuCount: 2, memoryBytes: 4 * GiB, diskBytes: 192 * GiB,
    });
  });

  for (const state of ['starting', 'paused', 'stopping']) {
    await t.test(`${state} retains compute reservation`, () => {
      const result = check([vm('master', state, { cpuCount: 6 })]);
      assert.equal(result.allowed, false);
      assert.deepEqual(result.exceeded, ['cpu']);
    });
  }

  await t.test('stopped disks can exhaust storage', () => {
    const result = check(Array.from({ length: 5 }, (_, i) => vm(String(i), 'stopped')));
    assert.deepEqual(result.exceeded, ['disk']);
  });

  await t.test('images and backups consume disk budget', () => {
    const result = check([vm('master', 'stopped', { diskBytes: 256 * GiB })],
      { extraDiskBytes: 1 });
    assert.deepEqual(result.exceeded, ['disk']);
  });

  await t.test('configured limits replace defaults', () => {
    const result = check([vm('master', 'running', { cpuCount: 6 })], {
      limits: { cpuPercent: 60, memoryPercent: 60, diskPercent: 30 },
    });
    assert.equal(result.allowed, true);
    assert.equal(result.capacity.cpuCount, 6);
  });

  await t.test('fractional cores round down', () => {
    const result = check([], { host: { ...host, cpuCount: 7 } });
    assert.equal(result.capacity.cpuCount, 3);
  });

  await t.test('invalid inventories fail closed', () => {
    for (const vms of [
      [vm('same'), vm('same')], [vm('')], [vm('bad', 'unknown')],
      [vm('bad', 'running', { cpuCount: 0 })],
      [vm('bad', 'running', { memoryBytes: -1 })],
      [vm('bad', 'running', { diskBytes: 0 })],
    ]) assert.throws(() => check(vms));
    assert.throws(() => check([], { host: { ...host, memoryBytes: 0 } }));
  });

  await t.test('invalid percentages fail closed', () => {
    for (const cpuPercent of [0, -1, 101, 50.5]) {
      assert.throws(() => check([], {
        limits: { cpuPercent, memoryPercent: 50, diskPercent: 25 },
      }));
    }
  });

  await t.test('overflow fails without approving', () => {
    assert.throws(() => check([vm('one', 'running', { diskBytes: 1e19 }),
      vm('two', 'running', { diskBytes: 1e19 })]));
  });
});
