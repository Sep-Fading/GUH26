// Checks the actual generated guest media.
import assert from 'node:assert/strict';
// Runs an explicitly selected native integration.
import { test } from 'node:test';
// Mounts generated media for read-only verification.
import { execFileSync } from 'node:child_process';
// Creates fixtures and reads mounted contents.
import { mkdtemp, readFile, readdir, rm, stat, mkdir } from 'node:fs/promises';
// Locates disposable media test output directories.
import { tmpdir } from 'node:os';
// Resolves independent source and output paths.
import { join, resolve } from 'node:path';
// Builds the same media as setup.
import { prepareMasterMedia } from '../scripts/vm/master-media.mjs';
// Checks copied bytes against vendor metadata.
import { verifyDownload } from '../scripts/setup/download.mjs';
// Uses the reviewed Apple silicon package.
import { masterCodex } from '../scripts/vm/codex-package.mjs';
// Finds the inputs setup already downloaded.
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';

// These tests need the verified Codex package, and two need the vendor-signed Computer Use
// app. Both come from the environment, or from where setup and Codex keep them. Without
// them the tests are skipped with the reason, not failed.
const found = path => (existsSync(path) ? path : undefined);
const archive = process.env.MINICLAW_MASTER_ARCHIVE
  ?? found(join(homedir(), 'Library/Caches/MiniClaw-setup/codex-package-aarch64-apple-darwin.tar.gz'));
const computerApp = process.env.MINICLAW_COMPUTER_APP
  ?? found(join(homedir(), '.codex/computer-use/Codex Computer Use.app'));
const needsArchive = archive ? false
  : 'set MINICLAW_MASTER_ARCHIVE to the verified Codex package, or run setup once to cache it';
const needsApp = needsArchive || (computerApp ? false
  : 'set MINICLAW_COMPUTER_APP to the signed Codex Computer Use app');

test('computer media refuses unsigned application sources', { timeout: 120_000, skip: needsArchive }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-computer-media-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await assert.rejects(prepareMasterMedia({ archive,
    output: join(root, 'media'), computerApp: root }), /Computer Use signature verification failed/);
});

test('mounted computer media preserves the vendor signature', { timeout: 120_000, skip: needsApp }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-computer-signed-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const result = await prepareMasterMedia({ archive,
    output: join(root, 'media'), computerApp: computerApp });
  const mount = join(root, 'mount'), extracted = join(root, 'extracted');
  await mkdir(extracted);
  execFileSync('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, result.image]);
  try {
    assert.ok((await stat(join(mount, 'computer-use.tar.gz'))).isFile());
    execFileSync('/usr/bin/tar', ['-xzf', join(mount, 'computer-use.tar.gz'), '-C', extracted]);
    execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict',
      '-R=anchor apple generic and certificate leaf[subject.OU] = "2DC432GLL2" and identifier "com.openai.sky.CUAService"',
      join(extracted, 'Codex Computer Use.app')]);
  } finally { execFileSync('/usr/bin/hdiutil', ['detach', mount]); }
});

test('Master media retains verified software without credentials', { timeout: 120_000, skip: needsArchive }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-master-media-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const result = await prepareMasterMedia({ archive,
    output: join(root, 'media'), signal: AbortSignal.timeout(90_000) });
  assert.equal((await stat(result.image)).mode & 0o077, 0);
  const mount = join(root, 'mount');
  execFileSync('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount,
    result.image], { timeout: 15_000 });
  try {
    await verifyDownload(join(mount, 'codex.tar.gz'), masterCodex);
    assert.equal(await readFile(join(mount, 'Install Codex.command'), 'utf8'),
      await readFile(resolve('scripts/vm/install-master.command'), 'utf8'));
    assert.ok((await stat(join(mount, 'Install Codex.command'))).mode & 0o100);
    assert.ok((await stat(join(mount, 'socket-probe'))).mode & 0o100);
    assert.deepEqual((await readdir(mount)).filter(name => !name.startsWith('.')).sort(),
      ['Install Codex.command', 'archive.sha256', 'codex.tar.gz', 'socket-probe']);
  } finally {
    execFileSync('/usr/bin/hdiutil', ['detach', mount], { timeout: 15_000 });
  }
});

test('worker media carries its binary and private pairing settings', { timeout: 120_000, skip: needsArchive }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-worker-media-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const settings = { key: 'ab'.repeat(32), cwd: '/Users/Shared/MiniClaw/Workspace' };
  const binary = resolve('target/release/miniclaw-worker');
  const result = await prepareMasterMedia({ archive,
    output: join(root, 'media'), worker: { binary, settings } });
  const mount = join(root, 'mount');
  execFileSync('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount,
    result.image], { timeout: 15_000 });
  try {
    assert.equal((await stat(mount)).mode & 0o005, 0o005);
    assert.equal((await stat(join(mount, 'worker.json'))).mode & 0o004, 0o004);
    assert.deepEqual(JSON.parse(await readFile(join(mount, 'worker.json'), 'utf8')), settings);
    assert.deepEqual(await readFile(join(mount, 'miniclaw-worker')), await readFile(binary));
    assert.ok((await stat(join(mount, 'miniclaw-desktop-stream'))).mode & 0o100);
    const installed = join(root, 'miniclaw-desktop-stream');
    execFileSync('/bin/cp', ['-X', join(mount, 'miniclaw-desktop-stream'), installed]);
    execFileSync('/usr/bin/codesign', ['--verify', '--strict',
      installed]);
    execFileSync('/usr/bin/shasum', ['-a', '256', '-c', 'worker.sha256'], { cwd: mount });
  } finally { execFileSync('/usr/bin/hdiutil', ['detach', mount], { timeout: 15_000 }); }
});
