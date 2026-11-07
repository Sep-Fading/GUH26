// Compares native configuration with isolation requirements.
import assert from 'node:assert/strict';
// Compiles and executes the native socket probe.
import { execFileSync } from 'node:child_process';
// Creates disposable compiler output and caches.
import { mkdtemp, rm } from 'node:fs/promises';
// Resolves paths within the disposable directory.
import { join } from 'node:path';
// Locates temporary storage outside retained guests.
import { tmpdir } from 'node:os';
// Runs the probe without starting guests.
import { test } from 'node:test';

test('guest network uses a private socket, never native NAT', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-network-probe-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'probe');
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6', '-warnings-as-errors',
    '-module-cache-path', join(root, 'cache'), '-parse-as-library',
    'scripts/vm/guest-network.swift', 'tests/support/network-probe.swift', '-o', binary], { timeout: 90000 });
  assert.match(execFileSync(binary, { encoding: 'utf8', timeout: 5000 }), /private network checks passed/);
});
