// Verifies metadata before considering guest installation.
import assert from 'node:assert/strict';
// Runs the explicit Apple metadata experiment.
import { test } from 'node:test';
// Compiles and signs our temporary probe.
import { execFile } from 'node:child_process';
// Removes temporary binaries and compiler caches.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates the operating system temporary directory.
import { tmpdir } from 'node:os';
// Builds paths within the temporary workspace.
import { join } from 'node:path';
// Resolves source and entitlement file locations.
import { fileURLToPath } from 'node:url';
// Awaits bounded compiler and probe subprocesses.
import { promisify } from 'node:util';

const run = promisify(execFile);

test('Apple supplies supported restore metadata', { timeout: 150_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-restore-metadata-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/restore-metadata.swift', import.meta.url,
  ));
  const entitlement = fileURLToPath(new URL(
    '../../scripts/experiments/virtualization.entitlements', import.meta.url,
  ));
  const binary = join(dir, 'restore-metadata');
  await run('/usr/bin/xcrun', ['swiftc', '-module-cache-path', join(dir, 'cache'),
    source, '-o', binary], { timeout: 90_000, maxBuffer: 1024 * 1024 });
  await run('/usr/bin/codesign', ['--sign', '-', '--entitlements', entitlement, binary],
    { timeout: 10_000 });
  const { stdout } = await run(binary, [], { timeout: 45_000, maxBuffer: 1024 * 1024 });
  const image = JSON.parse(stdout);
  t.diagnostic(JSON.stringify(image));
  assert.equal(image.supported, true);
  assert.ok(image.majorVersion >= 15, 'fresh macOS 15+ image is required for guest iCloud');
  assert.ok(image.minimumCPUCount > 0);
  assert.ok(image.minimumMemoryBytes > 0);
  assert.equal(new URL(image.url).protocol, 'https:');
});
