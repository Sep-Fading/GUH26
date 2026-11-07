// Verifies pinned terminal dependencies and package completeness.
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
import { conptyFiles, conptyPackage } from '../scripts/setup/windows-conpty.mjs';

test('ConPTY requires a pinned archive and supported architecture', async () => {
  for (const arch of ['ia32', 'toString', '__proto__']) assert.throws(() => conptyPackage(arch), /architecture/);
  await assert.rejects(conptyFiles({ arch: 'arm64', bytes: Buffer.from('tampered') }), /checksum/);
});

test('Windows packages declare both terminal runtime files', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-conpty-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'web'));
  await writeFile(join(root, 'web/index.html'), 'page');
  await writeFile(join(root, 'server.exe'), peFixture('arm64'));
  const manifest = await assemble({ binary: join(root, 'server.exe'), assets: join(root, 'web'),
    output: join(root, 'out'), version: 'test', platform: 'win32', arch: 'arm64', gitFiles: gitFixture, nodeFiles: nodeFixture,
    shellFiles: async () => new Map([['busybox.exe', Buffer.from('shell')], ['busybox-w32-FRP-6075-g169694ebd.tgz', Buffer.from('source')], ['LICENSE.txt', Buffer.from('license')]]),
    terminalFiles: async ({ arch }) => {
      assert.equal(arch, 'arm64');
      return new Map([['conpty.dll', Buffer.from('dll')], ['OpenConsole.exe', Buffer.from('console')]]);
    } });
  for (const name of ['conpty.dll', 'OpenConsole.exe', 'LICENSE.txt']) {
    assert.match(manifest.files[`bin/conpty/${name}`], /^[a-f0-9]{64}$/);
    assert.ok((await readFile(join(root, 'out/bin/conpty', name))).length);
  }
});

// Native acceptance uses the same official archive as production packaging.
test('official ConPTY archive contains both pinned architecture pairs',
  { skip: !process.env.MINICLAW_CONPTY_ARCHIVE }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'mc-real-conpty-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    await mkdir(join(root, 'web'));
    await writeFile(join(root, 'web/index.html'), 'page');
    for (const arch of ['arm64', 'x64']) {
      await writeFile(join(root, 'server.exe'), peFixture(arch));
      const files = await conptyFiles({ arch });
      assert.deepEqual([...files.keys()], ['conpty.dll', 'OpenConsole.exe']);
      for (const bytes of files.values()) assert.equal(bytes.subarray(0, 2).toString(), 'MZ');
      const manifest = await assemble({ binary: join(root, 'server.exe'), assets: join(root, 'web'),
        output: join(root, arch), version: 'test', platform: 'win32', arch, gitFiles: gitFixture, nodeFiles: nodeFixture, shellFiles: async () => new Map([['busybox.exe', Buffer.from('shell')], ['busybox-w32-FRP-6075-g169694ebd.tgz', Buffer.from('source')], ['LICENSE.txt', Buffer.from('license')]]) });
      for (const file of conptyPackage(arch).files) {
        assert.equal(manifest.files[`bin/conpty/${file.name}`], file.digest);
        assert.deepEqual(await readFile(join(root, arch, 'bin/conpty', file.name)), files.get(file.name));
      }
    }
  });
