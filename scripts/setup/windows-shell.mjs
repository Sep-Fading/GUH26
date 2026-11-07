// Includes the upstream license beside corresponding source.
import { readFile } from 'node:fs/promises';
// Loads bounded, checksum-verified upstream dependency files.
import { readPinned } from './pinned-bytes.mjs';

export const shellSourceName = 'busybox-w32-FRP-6075-g169694ebd.tgz';
export const shellNames = ['busybox.exe', shellSourceName, 'LICENSE.txt'];

// Pins official Unicode-capable BusyBox builds and their complete corresponding source.
export function windowsShellPackage(arch) {
  const builds = {
    arm64: { suffix: 'w64a', bytes: 646656, digest: 'e67f873d19d58c535cc9f0c4965ffd622e19b7bab87e3da89cb2185fb54464d7' },
    x64: { suffix: 'w64u', bytes: 675840, digest: '6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c' },
  };
  if (!Object.hasOwn(builds, arch)) throw new Error('Unsupported shell architecture');
  const build = builds[arch], base = 'https://frippery.org/files/busybox/';
  return {
    binary: { name: 'BusyBox binary', url: `${base}busybox-${build.suffix}-FRP-6075-g169694ebd.exe`,
      bytes: build.bytes, digest: build.digest },
    source: { name: 'BusyBox corresponding source', url: base + shellSourceName,
      bytes: 3690927, digest: '44401413c86a839deeec3eba088af244a1594f18ff9fd0622811100e4cc2e7b4' },
  };
}

// Packages no shell unless its matching source and GPL notice can accompany it.
export async function shellFiles({ arch, binary = process.env.MINICLAW_BUSYBOX_BINARY,
  source = process.env.MINICLAW_BUSYBOX_SOURCE, binaryBytes, sourceBytes }) {
  const pin = windowsShellPackage(arch);
  const executable = await readPinned(pin.binary, { archive: binary, bytes: binaryBytes });
  const corresponding = await readPinned(pin.source, { archive: source, bytes: sourceBytes });
  const license = await readFile(new URL('./licenses/busybox-LICENSE.txt', import.meta.url));
  return new Map([['busybox.exe', executable], [shellSourceName, corresponding], ['LICENSE.txt', license]]);
}
