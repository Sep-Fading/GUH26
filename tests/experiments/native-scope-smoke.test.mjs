// Verifies refusal paths without native calls.
import assert from 'node:assert/strict';
// Compiles and invokes only disabled checks.
import { execFileSync, spawnSync } from 'node:child_process';
// Creates disposable manifests and compiler output.
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
// Locates test artifacts outside retained guests.
import { tmpdir } from 'node:os';
// Constructs paths for the isolated runner.
import { join } from 'node:path';
// Runs explicit disabled-runner refusal contract cases.
import { test } from 'node:test';
// Resolves sources without invoking native APIs.
import { fileURLToPath } from 'node:url';

test('native smoke refuses unsafe setup', { timeout: 120_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-native-smoke-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = join(dir, 'native-smoke');
  const sources = ['native-scope-policy.swift', 'native-scope-worker.swift',
    'native-scope-smoke.swift'].map(name => fileURLToPath(new URL(
      `../../scripts/experiments/${name}`, import.meta.url,
    )));
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6',
    '-warnings-as-errors', '-module-cache-path', join(dir, 'cache'),
    ...sources, '-o', binary], { timeout: 90_000 });

  // Calls only refusal or validation modes, never an enabled native operation.
  function check(args) {
    return spawnSync(binary, args, {
      env: { PATH: '/usr/bin:/bin' }, encoding: 'utf8', timeout: 5_000,
    });
  }

  await t.test('native commands require explicit opt-in', () => {
    const result = check(['observe', join(dir, 'absent.json')]);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /native smoke is disabled/i);
  });

  await t.test('validation refuses the fixture owner', async () => {
    const fixture = join(dir, 'native-scope-fixture.json');
    await writeFile(fixture, JSON.stringify({ version: 1,
      fixtureId: 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa',
      scopeUserUID: process.getuid(), ownerUID: process.getuid(), importsDirectory: dir,
    }), { mode: 0o600 });
    const result = check(['validate', fixture]);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /run as the disposable user/i);
  });

  await t.test('validation refuses extra manifest fields', async () => {
    const fixture = join(dir, 'extra.json');
    await writeFile(fixture, JSON.stringify({ version: 1,
      fixtureId: 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa',
      scopeUserUID: process.getuid() + 1, ownerUID: process.getuid(), importsDirectory: dir,
      helper: '/untrusted/app',
    }), { mode: 0o600 });
    const result = check(['validate', fixture]);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /invalid fixture manifest/i);
  });
});
