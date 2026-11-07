// Verifies the packaged shell and corresponding source.
import assert from 'node:assert/strict';
// Supplies executables whose PE header names the packaged architecture.
import { peFixture } from './support/provider-fixture-paths.mjs';
import { test } from 'node:test';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Supplies a stand-in bundled Git runtime.
import { gitFixture } from './support/windows-git-fixture.mjs';
// Supplies stand-in private Node files.
import { nodeFixture } from './support/windows-node-fixture.mjs';
import { assemble } from '../scripts/backend-package.mjs';
import { windowsShellPackage, shellFiles } from '../scripts/setup/windows-shell.mjs';

test('shell pins reject unknown architectures and changed bytes', async () => {
  for (const arch of ['ia32', 'constructor']) assert.throws(() => windowsShellPackage(arch), /architecture/);
  await assert.rejects(shellFiles({ arch: 'arm64', binaryBytes: Buffer.from('changed') }), /checksum/);
});

test('Windows packages ship shell source and license together', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-shell-package-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'web'));
  await writeFile(join(root, 'web/index.html'), 'page');
  await writeFile(join(root, 'server.exe'), peFixture('arm64'));
  const manifest = await assemble({ binary: join(root, 'server.exe'), assets: join(root, 'web'),
    output: join(root, 'out'), version: 'test', platform: 'win32', arch: 'arm64', gitFiles: gitFixture, nodeFiles: nodeFixture,
    terminalFiles: async () => new Map([['conpty.dll', Buffer.from('dll')], ['OpenConsole.exe', Buffer.from('console')]]),
    shellFiles: async () => new Map([['busybox.exe', Buffer.from('shell')],
      ['busybox-w32-FRP-6075-g169694ebd.tgz', Buffer.from('source')], ['LICENSE.txt', Buffer.from('license')]]) });
  for (const name of ['busybox.exe', 'busybox-w32-FRP-6075-g169694ebd.tgz', 'LICENSE.txt']) {
    assert.match(manifest.files[`bin/shell/${name}`], /^[a-f0-9]{64}$/);
    assert.ok((await readFile(join(root, 'out/bin/shell', name))).length);
  }
});

// Offline acceptance uses the exact official binary and complete source used by release builds.
test('official shell binary requires its pinned corresponding source',
  { skip: !process.env.MINICLAW_BUSYBOX_BINARY || !process.env.MINICLAW_BUSYBOX_SOURCE }, async () => {
    const files = await shellFiles({ arch: process.arch });
    assert.equal(files.get('busybox.exe').subarray(0, 2).toString(), 'MZ');
    assert.match(files.get('LICENSE.txt').toString(), /GNU GENERAL PUBLIC LICENSE/);
    assert.equal(files.get('busybox-w32-FRP-6075-g169694ebd.tgz').length, windowsShellPackage(process.arch).source.bytes);
    await assert.rejects(shellFiles({ arch: process.arch, sourceBytes: Buffer.from('changed source') }), /source checksum/);
  });
