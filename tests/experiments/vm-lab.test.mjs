// Checks isolation and persistent file protection.
import assert from 'node:assert/strict';
// Runs independent native VM lab checks.
import { test } from 'node:test';
// Compiles and executes the native supervisor.
import { execFile } from 'node:child_process';
// Awaits readiness from the independent lockholder.
import { once } from 'node:events';
// Creates disposable paths without touching guests.
import { mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
// Locates disposable operating system test storage.
import { tmpdir } from 'node:os';
// Builds paths to isolated test fixtures.
import { join } from 'node:path';
// Resolves the supervisor and entitlement sources.
import { fileURLToPath } from 'node:url';
// Awaits bounded compiler and supervisor processes.
import { promisify } from 'node:util';

const run = promisify(execFile);

test('offline VM lab', { timeout: 120_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-vm-lab-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/vm-lab.swift', import.meta.url,
  ));
  const entitlement = fileURLToPath(new URL(
    '../../scripts/experiments/virtualization.entitlements', import.meta.url,
  ));
  const window = fileURLToPath(new URL(
    '../../scripts/experiments/vm-window.swift', import.meta.url,
  ));
  const headless = fileURLToPath(new URL(
    '../../scripts/experiments/vm-headless.swift', import.meta.url,
  ));
  const binary = join(dir, 'vm-lab');
  await run('/usr/bin/xcrun', ['swiftc', '-swift-version', '6', '-warnings-as-errors',
    '-module-cache-path', join(dir, 'cache'),
    source, window, headless, 'scripts/vm/host-relay.swift', 'scripts/vm/guest-bridge.swift', 'scripts/vm/guest-network.swift',
    '-o', binary], { timeout: 90_000 });
  await run('/usr/bin/codesign', ['--sign', '-', '--entitlements', entitlement, binary]);
  const lockSource = fileURLToPath(new URL('./support/guest-lock.c', import.meta.url));
  const lockBinary = join(dir, 'guest-lock');
  await run('/usr/bin/xcrun', ['clang', '-Wall', '-Wextra', '-Werror', lockSource,
    '-o', lockBinary], { timeout: 30_000 });

  await t.test('configuration has no host connections', async () => {
    const { stdout } = await run(binary, ['policy']);
    const policy = JSON.parse(stdout);
    assert.equal(policy.cpuCount, 4);
    assert.equal(policy.memoryBytes, 8 * 1024 ** 3);
    assert.equal(policy.diskBytes, 64 * 1024 ** 3);
    for (const key of ['networks', 'shares', 'sockets', 'serialPorts', 'consoles',
      'audioDevices', 'usbControllers']) assert.equal(policy[key], 0, key);
    assert.equal(policy.displays, 1);
    assert.equal(policy.keyboards, 1);
    assert.equal(policy.pointingDevices, 1);
  });

  await t.test('setup media is read-only without a network or share', async () => {
    const media = join(dir, 'setup.img');
    await writeFile(media, Buffer.alloc(4096));
    const { stdout } = await run(binary, ['policy', '--media', media]);
    const policy = JSON.parse(stdout);
    assert.deepEqual(policy.storageReadOnly, [true]);
    assert.equal(policy.sockets, 1);
    assert.equal(policy.networks, 0);
    assert.equal(policy.shares, 0);
    const alias = join(dir, 'media-link');
    await symlink(media, alias);
    await assert.rejects(run(binary, ['policy', '--media', alias]),
      error => /Symbolic links are not allowed/.test(error.stderr));
  });

  await t.test('approved Master resources determine the native configuration', async () => {
    const { stdout } = await run(binary, ['policy', '--resources', '2', '4', '96']);
    const policy = JSON.parse(stdout);
    assert.equal(policy.cpuCount, 2);
    assert.equal(policy.memoryBytes, 4 * 1024 ** 3);
    assert.equal(policy.diskBytes, 96 * 1024 ** 3);
    for (const args of [['0', '4', '64'], ['2', '1', '64'], ['2', '4', '1']]) {
      await assert.rejects(run(binary, ['policy', '--resources', ...args]), /resources/i);
    }
  });

  await t.test('init refuses existing files', async () => {
    const sentinel = join(dir, 'keep.txt');
    await writeFile(sentinel, 'retain this VM until all testing is complete');
    await assert.rejects(run(binary, ['init', dir, join(dir, 'missing.ipsw')]),
      error => /Guest directory already exists/.test(error.stderr));
    assert.equal(await readFile(sentinel, 'utf8'),
      'retain this VM until all testing is complete');
  });

  await t.test('inspect never creates missing guests', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['inspect', join(dir, 'missing')]),
      error => /Guest directory does not exist/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('socket probe requires an existing guest', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['inspect', join(dir, 'missing'), '--socket']),
      error => /Guest directory does not exist/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('inspect refuses unrelated directories without writes', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['inspect', dir]),
      error => /Not a MiniClaw test guest/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('run cannot create a missing guest', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['run', join(dir, 'missing')]),
      error => /Guest directory does not exist/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('headless cannot create a missing guest', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['headless', join(dir, 'missing')]),
      error => /Guest directory does not exist/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('headless refuses unrelated directories', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['headless', dir]),
      error => /Not a MiniClaw test guest/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('guest paths cannot alias another directory', async () => {
    const alias = join(dir, 'alias');
    await symlink(dir, alias);
    await assert.rejects(run(binary, ['inspect', alias]),
      error => /Symbolic links are not allowed/.test(error.stderr));
  });

  await t.test('unknown commands do not create files', async () => {
    const before = await readdir(dir);
    await assert.rejects(run(binary, ['delete', dir]),
      error => /Usage:/.test(error.stderr));
    assert.deepEqual(await readdir(dir), before);
  });

  await t.test('guest marker cannot be a symlink', async () => {
    const fixture = join(dir, 'marker-link');
    await mkdir(fixture);
    const marker = join(dir, 'marker');
    await writeFile(marker, 'miniclaw-offline-vm-lab-v1\n');
    await symlink(marker, join(fixture, 'LabOwner'));
    await assert.rejects(run(binary, ['inspect', fixture]),
      error => /Symbolic links are not allowed/.test(error.stderr));
    assert.deepEqual(await readdir(fixture), ['LabOwner']);
  });

  await t.test('guest lock cannot redirect file access', async () => {
    const fixture = join(dir, 'lock-link');
    await mkdir(fixture);
    await writeFile(join(fixture, 'LabOwner'), 'miniclaw-offline-vm-lab-v1\n');
    const target = join(dir, 'untouched');
    await writeFile(target, 'untouched');
    await symlink(target, join(fixture, '.lock'));
    await assert.rejects(run(binary, ['inspect', fixture]),
      error => /Cannot open guest lock/.test(error.stderr));
    assert.equal(await readFile(target, 'utf8'), 'untouched');
  });

  await t.test('run rejects an incomplete installation', async () => {
    const fixture = join(dir, 'incomplete');
    await mkdir(fixture);
    await writeFile(join(fixture, 'LabOwner'), 'miniclaw-offline-vm-lab-v1\n');
    await writeFile(join(fixture, 'Status'), 'installing\n');
    await assert.rejects(run(binary, ['run', fixture]),
      error => /Guest installation is not complete/.test(error.stderr));
    assert.equal(await readFile(join(fixture, 'Status'), 'utf8'), 'installing\n');
  });

  await t.test('headless rejects incomplete installation', async () => {
    const fixture = join(dir, 'incomplete');
    await assert.rejects(run(binary, ['headless', fixture]),
      error => /Guest installation is not complete/.test(error.stderr));
    assert.equal(await readFile(join(fixture, 'Status'), 'utf8'), 'installing\n');
  });

  await t.test('only one process owns a guest', async () => {
    const fixture = join(dir, 'busy');
    await mkdir(fixture);
    await writeFile(join(fixture, 'LabOwner'), 'miniclaw-offline-vm-lab-v1\n');
    const holder = execFile(lockBinary, [join(fixture, '.lock')], { timeout: 10_000 });
    const exited = once(holder, 'exit');
    exited.catch(() => {});
    try {
      const [ready] = await once(holder.stdout, 'data', { signal: AbortSignal.timeout(5_000) });
      assert.match(String(ready), /locked/);
      await assert.rejects(run(binary, ['inspect', fixture]),
        error => /Guest is already in use/.test(error.stderr));
      await assert.rejects(run(binary, ['headless', fixture]),
        error => /Guest is already in use/.test(error.stderr));
    } finally {
      holder.stdin.end();
      await exited;
    }
    await assert.rejects(run(binary, ['inspect', fixture]),
      error => /HardwareModel/.test(error.stderr));
  });
});
