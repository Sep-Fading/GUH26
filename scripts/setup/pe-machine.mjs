// Reads a Windows executable's architecture from its PE header.

// Rejects malformed headers instead of guessing architecture from a filename.
export function peMachine(bytes) {
  if (bytes.length < 64 || bytes.toString('ascii', 0, 2) !== 'MZ') throw new Error('Invalid PE executable');
  const offset = bytes.readUInt32LE(60);
  if (offset < 64 || offset + 6 > bytes.length || bytes.readUInt32LE(offset) !== 0x4550) throw new Error('Invalid PE executable');
  return bytes.readUInt16LE(offset + 4);
}

// Names the Node-style architecture of a supported Windows executable.
export function peArch(bytes) {
  const arch = { 0x8664: 'x64', 0xaa64: 'arm64' }[peMachine(bytes)];
  if (!arch) throw new Error('Unsupported Windows executable architecture');
  return arch;
}
