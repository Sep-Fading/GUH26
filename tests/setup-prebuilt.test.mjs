import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { prebuiltIn } from '../scripts/setup/prebuilt.mjs';

const hash = text => createHash('sha256').update(text).digest('hex');
// A Windows release records its platform and ships the backend as an .exe with no VM worker;
// a Mac release predates the platform field, which therefore means darwin.
const windows = process.platform === 'win32';
const SERVER = `bundle/bin/miniclaw-server${windows ? '.exe' : ''}`;
const OTHER = windows ? 'bundle/web/index.html' : 'miniclaw-worker';

// A release folder with two prebuilt files and their manifest.
async function release(t, files = { [OTHER]: 'worker bytes', [SERVER]: 'server bytes' }) {
  const root = await mkdtemp(join(tmpdir(), 'mc-prebuilt-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'prebuilt/bundle/bin'), { recursive: true });
  if (windows) await mkdir(join(root, 'prebuilt/bundle/web'), { recursive: true });
  for (const [name, text] of Object.entries(files)) await writeFile(join(root, 'prebuilt', name), text);
  await writeFile(join(root, 'prebuilt/manifest.json'), JSON.stringify({ schemaVersion: 1, version: '0.1.0',
    ...(windows && { platform: 'win32' }), arch: process.arch, files: Object.fromEntries(Object.entries(files).map(([name, text]) => [name, hash(text)])) }));
  return root;
}

test('a checkout has no prebuilt files, so setup builds as before', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-checkout-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const prebuilt = prebuiltIn(root);
  assert.equal(await prebuilt.available(), false);
  assert.equal(await prebuilt.file(OTHER), null);
});

test('a release hands out a prebuilt file only when it matches its recorded checksum', async t => {
  const root = await release(t);
  const prebuilt = prebuiltIn(root);
  assert.equal(await prebuilt.available(), true);
  assert.equal(await prebuilt.file(OTHER), join(root, 'prebuilt', OTHER));
  assert.equal(await prebuilt.file(SERVER), join(root, 'prebuilt', SERVER));
  // A release that lacks a file is broken; it must not fall back to compiling silently.
  await assert.rejects(prebuilt.file('master-vm'), /not part of this release/);
  await writeFile(join(root, 'prebuilt', OTHER), 'tampered');
  await assert.rejects(prebuilt.file(OTHER), /does not match/);
  await assert.rejects(prebuilt.file('../settings.json'), /not part of this release/);
});

test('a release built for another processor is refused', async t => {
  const root = await release(t);
  await writeFile(join(root, 'prebuilt/manifest.json'), JSON.stringify({ schemaVersion: 1, version: '0.1.0',
    ...(windows && { platform: 'win32' }), arch: 'x64', files: {} }));
  await assert.rejects(prebuiltIn(root).available(), /built for x64/);
});

test('a release built for the other operating system is refused', async t => {
  const root = await release(t);
  const other = windows ? 'darwin' : 'win32';
  const manifest = { schemaVersion: 1, version: '0.1.0', arch: process.arch, files: {} };
  await writeFile(join(root, 'prebuilt/manifest.json'), JSON.stringify({ ...manifest, platform: other }));
  await assert.rejects(prebuiltIn(root).available(), new RegExp(`platform is ${other}, not ${process.platform}`));
  if (windows) {
    // A Mac release written before the platform field existed is a darwin release.
    await writeFile(join(root, 'prebuilt/manifest.json'), JSON.stringify(manifest));
    await assert.rejects(prebuiltIn(root).available(), /platform is darwin, not win32/);
  }
});
