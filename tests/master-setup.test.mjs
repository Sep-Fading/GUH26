// Checks guest setup without installing software.
import assert from 'node:assert/strict';
// Runs isolated setup command acceptance cases.
import { test } from 'node:test';
// Invokes entry points with bounded execution.
import { spawnSync } from 'node:child_process';
// Creates and removes disposable installation destinations.
import { mkdtemp, readdir, rm } from 'node:fs/promises';
// Resolves temporary fixture and script paths.
import { join, resolve } from 'node:path';
// Locates disposable storage outside real installations.
import { tmpdir } from 'node:os';

test('noninteractive setup cannot approve guest downloads', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-master-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const result = spawnSync(process.execPath, [resolve('scripts/cli.mjs'),
    'setup', '--destination', join(dir, 'media')],
  { encoding: 'utf8', timeout: 5000 });
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /Interactive terminal required/);
  assert.deepEqual(await readdir(dir), []);
});

test('guest installer refuses the host before creating files', {
  skip: process.platform === 'win32' && 'macOS VM guest installer only: a bash script that checks the Mac hw.model with sysctl',
}, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-not-guest-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const result = spawnSync('/bin/bash', [resolve('scripts/vm/install-master.command')],
    { env: { HOME: dir, PATH: '/usr/bin:/bin:/usr/sbin:/sbin' },
      encoding: 'utf8', timeout: 5000 });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /inside the macOS VM/);
  assert.deepEqual(await readdir(dir), []);
});
