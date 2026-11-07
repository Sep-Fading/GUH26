// Checks official terminal archive and binary integrity.
import { createHash } from 'node:crypto';
// Loads bounded, checksum-verified offline or downloaded dependencies.
import { readPinned } from './pinned-bytes.mjs';
// Reuses the strict ordinary-file ZIP decoder.
import { gitArchiveEntries } from './windows-git.mjs';

// Pins Microsoft's terminal runtime for supported native architectures.
export function conptyPackage(arch) {
  const hashes = {
    arm64: ['db3d173640b172bafd42d5b541b638a9aeec1c7d0e40dd636bf02822a32c912c', 'ed7622fd0d3bedc9ab9f122f5e58edf0def9e7999224f52dd395ba9f54edbe09'],
    x64: ['39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8', 'b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160'],
  };
  if (!Object.hasOwn(hashes, arch)) throw new Error('Unsupported ConPTY architecture');
  return { name: 'ConPTY archive', url: 'https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.24.260710001/microsoft.windows.console.conpty.1.24.260710001.nupkg',
    bytes: 1732296, digest: '175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e',
    files: [
      { name: 'conpty.dll', path: `runtimes/win-${arch}/native/conpty.dll`, digest: hashes[arch][0] },
      { name: 'OpenConsole.exe', path: `build/native/runtimes/${arch}/OpenConsole.exe`, digest: hashes[arch][1] },
    ] };
}

// Returns only the verified runtime pair, never NuGet build scripts or arbitrary archive paths.
export async function conptyFiles({ arch, archive = process.env.MINICLAW_CONPTY_ARCHIVE, bytes }) {
  const pin = conptyPackage(arch);
  const content = await readPinned(pin, { archive, bytes });
  const entries = gitArchiveEntries(content), result = new Map();
  for (const file of pin.files) {
    const data = entries.get(file.path);
    if (!data || createHash('sha256').update(data).digest('hex') !== file.digest) {
      throw new Error(`ConPTY binary checksum mismatch: ${file.name}`);
    }
    result.set(file.name, data);
  }
  return result;
}
