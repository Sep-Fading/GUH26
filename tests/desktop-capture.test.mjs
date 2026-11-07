// Checks capture settings, permission requests and helper updates.
import assert from 'node:assert/strict';
// Runs isolated desktop capture regression cases.
import { test } from 'node:test';
// Executes the installer's helper function alone.
import { execFileSync } from 'node:child_process';
// Creates disposable media and installation folders.
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
// Resolves paths inside each isolated fixture.
import { join } from 'node:path';
// Locates temporary storage outside real installations.
import { tmpdir } from 'node:os';

test('the helper captures a local VM at 30 frames per second and full HD', async () => {
  const source = await readFile('scripts/vm/desktop-stream.swift', 'utf8');
  assert.match(source, /CMTime\(value: 1, timescale: 30\)/);
  assert.match(source, /1920\.0 \/ Double/);
  assert.match(source, /for quality in \[0\.7, 0\.45\]/);
  // A frame still being encoded is dropped, so a slow guest lags no further behind.
  assert.match(source, /guard !encoding else \{ return \}/);
});

test('the worker asks macOS for Screen Recording itself, once, when capture is refused', async () => {
  const source = await readFile('server/src/worker.rs', 'utf8');
  assert.match(source, /static AUTHORIZATION_REQUESTED: AtomicBool/);
  assert.match(source, /\.arg\("--authorize"\)/);
  const capture = source.slice(source.indexOf('async fn capture_desktop'));
  assert.match(capture.slice(0, capture.indexOf('\n}\n')), /request_capture_access/);
});

// Runs only the installer's helper step against fixture folders.
async function installHelper(t, existing) {
  const root = await mkdtemp(join(tmpdir(), 'mc-capture-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const media = join(root, 'media'), runtime = join(root, 'base', 'Runtime');
  await mkdir(media);
  await mkdir(runtime, { recursive: true });
  await writeFile(join(media, 'miniclaw-desktop-stream'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const installed = join(runtime, 'miniclaw-desktop-stream');
  if (existing !== undefined) await writeFile(installed, existing, { mode: 0o700 });
  const source = await readFile('scripts/vm/install-master.command', 'utf8');
  const definitions = source.slice(0, -'install_codex\n'.length);
  // The stub records what was verified; files on the setup disk carry Finder metadata
  // that a strict signature check refuses, so only stripped copies may be checked.
  const checked = join(root, 'checked');
  execFileSync('/bin/bash', ['-c', definitions
    + `\ncodesign_check() { printf '%s\\n' "$1" >> '${checked}'; }\ninstall_desktop_helper "$@"`,
  'fixture', media, join(root, 'base')]);
  const paths = (await readFile(checked, 'utf8')).trim().split('\n');
  assert.ok(paths.length > 0, 'the installed helper is verified');
  assert.ok(paths.every(path => !path.startsWith(media)), `verified a file on the setup disk: ${paths}`);
  return installed;
}

test('the installer replaces an older capture helper and keeps it private', async t => {
  const installed = await installHelper(t, 'old helper');
  assert.equal(await readFile(installed, 'utf8'), '#!/bin/sh\nexit 0\n');
  assert.equal((await stat(installed)).mode & 0o777, 0o700);
});

test('the installer installs a first capture helper', async t => {
  const installed = await installHelper(t);
  assert.equal((await stat(installed)).mode & 0o777, 0o700);
});
