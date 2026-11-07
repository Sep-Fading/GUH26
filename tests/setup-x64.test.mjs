// Host checks for the Windows x64 branch (docs/windows/x64-readiness.md). Native sandbox runs so far used
// Windows 11 ARM64 only, so these pin the x64 choices explicitly instead of following process.arch.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { access, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { peArch, peMachine } from '../scripts/setup/pe-machine.mjs';
import { windowsShellPackage, shellFiles } from '../scripts/setup/windows-shell.mjs';
import { conptyPackage, conptyFiles } from '../scripts/setup/windows-conpty.mjs';
import { windowsGitBundle, gitBundleFiles } from '../scripts/setup/windows-git-bundle.mjs';
import { gitArchiveEntries } from '../scripts/setup/windows-git.mjs';
import { releasePlatform } from '../scripts/package-release.mjs';
import { claudePlatform } from '../scripts/setup/claude-install.mjs';

// A minimal PE header naming one machine.
function pe(machine) {
  const bytes = Buffer.alloc(128); bytes.write('MZ', 0, 'ascii'); bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64); bytes.writeUInt16LE(machine, 68);
  return bytes;
}

test('x64 PE machine 0x8664 is recognised and never confused with ARM64 or x86', () => {
  assert.equal(peArch(pe(0x8664)), 'x64');
  assert.equal(peArch(pe(0xaa64)), 'arm64');
  // x86 and ARM64EC are not shipped architectures.
  for (const machine of [0x14c, 0xa641]) assert.throws(() => peArch(pe(machine)), /architecture/);
});

test('every x64 dependency pin names the x64 build, distinct from ARM64', () => {
  const shell = windowsShellPackage('x64');
  assert.match(shell.binary.url, /\/busybox-w64u-FRP-6075-g169694ebd\.exe$/, 'the Unicode x64 BusyBox build');
  assert.equal(shell.binary.bytes, 675840);
  assert.equal(shell.binary.digest, '6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c');
  assert.notEqual(shell.binary.digest, windowsShellPackage('arm64').binary.digest);
  const conpty = conptyPackage('x64');
  assert.deepEqual(conpty.files.map(file => file.path), ['runtimes/win-x64/native/conpty.dll', 'build/native/runtimes/x64/OpenConsole.exe']);
  assert.deepEqual(conpty.files.map(file => file.digest), ['39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8',
    'b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160']);
  const git = windowsGitBundle('x64');
  assert.match(git.file, /-x64\.zip$/);
  assert.notEqual(git.digest, windowsGitBundle('arm64').digest);
  assert.equal(releasePlatform('win32', 'x64').node.file, 'node-v24.21.0-win-x64.zip');
  assert.equal(claudePlatform('win32', 'x64'), 'win32-x64');
});

// With MINICLAW_X64_ARTIFACTS pointing at a folder holding the real downloaded or built files, the
// production readers verify the x64 pins against the bytes and every x64 executable's PE machine.
const artifacts = process.env.MINICLAW_X64_ARTIFACTS;
const present = async name => access(join(artifacts, name)).then(() => true, () => false);

test('real x64 artifacts match their pins and are all PE machine 0x8664', { skip: !artifacts && 'set MINICLAW_X64_ARTIFACTS' }, async t => {
  const shell = await shellFiles({ arch: 'x64', binary: join(artifacts, 'busybox-w64u-FRP-6075-g169694ebd.exe'),
    source: join(artifacts, 'busybox-w32-FRP-6075-g169694ebd.tgz') });
  assert.equal(peMachine(shell.get('busybox.exe')), 0x8664);
  const conpty = await conptyFiles({ arch: 'x64', archive: join(artifacts, 'conpty.1.24.260710001.nupkg') });
  for (const name of ['conpty.dll', 'OpenConsole.exe']) assert.equal(peMachine(conpty.get(name)), 0x8664, name);
  const pin = windowsGitBundle('x64');
  if (!await present(pin.file)) { t.diagnostic(`${pin.file} not in MINICLAW_X64_ARTIFACTS; Git not checked`); return; }
  const archive = (await gitBundleFiles({ arch: 'x64', archive: join(artifacts, pin.file) })).get(pin.file);
  const executables = [...gitArchiveEntries(archive)].filter(([, bytes]) => bytes.toString('ascii', 0, 2) === 'MZ');
  assert.ok(executables.some(([name]) => name === 'git/bin/git.exe'));
  assert.deepEqual(executables.filter(([, bytes]) => peMachine(bytes) !== 0x8664).map(([name]) => name), []);
  t.diagnostic(`${executables.length} x64 Git executables checked in ${pin.file}`);
  // Keeps the archive bytes and the file on disk the same object of review.
  assert.ok(archive.equals(await readFile(join(artifacts, pin.file))));
});
