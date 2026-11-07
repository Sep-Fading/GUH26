// Checks native preflight output without assumptions.
import assert from 'node:assert/strict';
// Runs the isolated Apple framework probe.
import { test } from 'node:test';
// Compiles and executes only our probe.
import { execFile } from 'node:child_process';
// Manages temporary binaries and module caches.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates disposable directories and host resources.
import { tmpdir, totalmem, cpus } from 'node:os';
// Resolves fixture sources and temporary outputs.
import { join } from 'node:path';
// Converts source URLs into executable paths.
import { fileURLToPath } from 'node:url';
// Awaits subprocess completion with bounded output.
import { promisify } from 'node:util';

const run = promisify(execFile);

test('Apple VM framework preflight', { timeout: 120_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-vm-preflight-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/vm-preflight.swift', import.meta.url,
  ));
  const binary = join(dir, 'preflight');
  await run('/usr/bin/xcrun', ['swiftc', '-module-cache-path', join(dir, 'cache'),
    source, '-o', binary], { timeout: 90_000, maxBuffer: 1024 * 1024 });
  const { stdout } = await run(binary, [], { timeout: 10_000 });
  const report = JSON.parse(stdout);
  t.diagnostic(JSON.stringify(report));
  assert.equal(report.virtualizationSupported, true);
  assert.equal(report.hostMemoryBytes, totalmem());
  assert.equal(report.hostCPUCount, cpus().length);
  assert.ok(report.minimumCPUCount > 0);
  assert.ok(report.maximumCPUCount >= report.minimumCPUCount);
  assert.ok(report.minimumMemoryBytes > 0);
  assert.ok(report.maximumMemoryBytes >= report.minimumMemoryBytes);
});
