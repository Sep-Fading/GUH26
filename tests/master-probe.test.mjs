// Checks the fixed guest handshake independently.
import assert from 'node:assert/strict';
// Runs native handshake cases without VMs.
import { test } from 'node:test';
// Compiles and runs the bounded probe.
import { execFileSync } from 'node:child_process';
// Allocates disposable native build output directories.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates temporary storage for compiled fixtures.
import { tmpdir } from 'node:os';
// Resolves probe sources and disposable binaries.
import { join, resolve } from 'node:path';

test('guest probe accepts only the fixed handshake', {
  skip: process.platform === 'win32' && 'macOS VM guest probe only: compiled with Xcode clang (/usr/bin/xcrun) for the Mac guest',
}, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-master-probe-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = join(dir, 'probe');
  execFileSync('/usr/bin/xcrun', ['clang', '-Wall', '-Wextra', '-Werror',
    '-DMASTER_PROBE_TEST', resolve('scripts/vm/master-probe.c'),
    resolve('tests/support/master-probe.c'), '-o', binary], { timeout: 30_000 });
  const result = execFileSync(binary, { encoding: 'utf8', timeout: 5000 });
  assert.match(result, /valid, wrong greeting, closed peer: passed/);
});
