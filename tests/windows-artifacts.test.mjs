// Tests architecture selection independently from the host operating system.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { join } from 'node:path';
import { nativeArtifacts, peMachine } from './support/provider-fixture-paths.mjs';

test('explicit target never selects default host artifacts', () => {
  const root = '/fixture';
  assert.equal(nativeArtifacts(root, '', 'win32').backend, join(root, 'target/debug/miniclaw-server.exe'));
  const chosen = nativeArtifacts(root, 'x86_64-pc-windows-msvc', 'win32');
  assert.equal(chosen.backend, join(root, 'target/x86_64-pc-windows-msvc/debug/miniclaw-server.exe'));
  assert.equal(chosen.provider, join(root, 'target/x86_64-pc-windows-msvc/debug/examples/provider_fixture.exe'));
  assert.throws(() => nativeArtifacts(root, '../debug', 'win32'));
});

test('PE machine comes from executable header', () => {
  const bytes = Buffer.alloc(128);
  bytes.write('MZ'); bytes.writeUInt32LE(64, 60); bytes.write('PE\0\0', 64);
  bytes.writeUInt16LE(0xaa64, 68);
  assert.equal(peMachine(bytes), 0xaa64);
  bytes.writeUInt16LE(0x8664, 68);
  assert.equal(peMachine(bytes), 0x8664);
  assert.throws(() => peMachine(Buffer.from('not executable')));
  bytes.writeUInt32LE(1000, 60);
  assert.throws(() => peMachine(bytes));
});
