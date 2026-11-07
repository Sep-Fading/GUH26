// Selects native test executable and control transport names.
import { join } from 'node:path';

// Keeps Unix sockets on Unix and uses process-scoped named pipes on Windows.
export function providerFixturePaths(dir, id, platform = process.platform) {
  return { script: join(dir, 'codex-fixture.mjs'),
    binary: join(dir, platform === 'win32' ? 'codex-fixture.exe' : 'codex-fixture.mjs'),
    socket: platform === 'win32' ? `\\\\.\\pipe\\miniclaw-test-${id}` : join(dir, 'peer.sock') };
}

// Chooses one architecture directory without a fallback to host build output.
export function nativeArtifacts(root, target = '', platform = process.platform) {
  const machines = { 'aarch64-pc-windows-msvc': 0xaa64, 'x86_64-pc-windows-msvc': 0x8664 };
  if (target && !Object.hasOwn(machines, target)) throw new Error('Unsupported native test target');
  const directory = target ? join(root, 'target', target, 'debug') : join(root, 'target/debug');
  return { backend: join(directory, `miniclaw-server${platform === 'win32' ? '.exe' : ''}`),
    provider: join(directory, 'examples/provider_fixture.exe'), machine: machines[target] };
}

// Shares the packager's PE header reader, so tests and releases agree.
export { peMachine } from '../../scripts/setup/pe-machine.mjs';

// Builds the smallest image whose PE header names one architecture, for packaging fixtures.
export function peFixture(arch) {
  const bytes = Buffer.alloc(128);
  bytes.write('MZ', 0, 'ascii');
  bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE({ x64: 0x8664, arm64: 0xaa64 }[arch], 68);
  return bytes;
}
