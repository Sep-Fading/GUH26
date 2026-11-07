// Checks real native stream forwarding behavior.
import assert from 'node:assert/strict';
// Runs the relay without a guest.
import { test } from 'node:test';
// Compiles and executes the native stream probe.
import { execFileSync } from 'node:child_process';
// Creates disposable compiler output and caches.
import { mkdtemp, rm } from 'node:fs/promises';
// Resolves paths inside the temporary probe.
import { join } from 'node:path';
// Locates temporary storage outside retained guests.
import { tmpdir } from 'node:os';

test('native relay preserves streams and descriptor ownership', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-relay-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'probe');
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6', '-warnings-as-errors',
    '-module-cache-path', join(root, 'cache'), '-parse-as-library',
    'scripts/vm/host-relay.swift', 'tests/support/relay-probe.swift', '-o', binary], { timeout: 90000 });
  assert.match(execFileSync(binary, { encoding: 'utf8', timeout: 5000 }), /checks passed/);
});
