// Checks the official Node archive and the files taken from it.
import { createHash } from 'node:crypto';
// Reads the shipped OpenSSL configuration and the built seed library.
import { lstat, readFile } from 'node:fs/promises';
// Inflates the two archive members MiniClaw needs.
import { inflateRawSync } from 'node:zlib';
// Loads bounded, checksum-verified offline or downloaded archives.
import { readPinned } from './pinned-bytes.mjs';
// Confirms the seed library was built for the bundle's processor.
import { peArch } from './pe-machine.mjs';
// Supplies the pinned BusyBox that npm runs package scripts with.
import { shellFiles, windowsShellPackage } from './windows-shell.mjs';

// The files of MiniClaw's private Node, in `bin/node` of a Windows bundle. The backend checks
// node.exe, the seed library, openssl.cnf and the child-process preload (child-pipes.cjs) again
// before any sandboxed run uses them, and the npm launchers, sh.exe and npm's tree
// (`node_modules/npm/...`) before a run uses npm.
export const sandboxNodeNames = ['node.exe', 'LICENSE', 'miniclaw_openssl_seed.dll', 'openssl.cnf', 'child-pipes.cjs', 'npm', 'npx', 'npm-sandbox.cjs', 'sh.exe'];

// npm's tree in the Node 24.21.0 archives (the same for ARM64 and x64): its file count and the
// digest of its listing (npmTreeDigest), also pinned in server/src/windows_runtimes.rs.
export const sandboxNpm = { prefix: 'node_modules/npm/', files: 1926, digest: '609999b72b258436fecbad1b66990c83097f8eae5a15f72e4475a34074e43a80' };

// Digests a tree as SHA-256 over "<sha256>  <path>\n" lines sorted bytewise by path, as the backend does.
export function npmTreeDigest(files) {
  const lines = [...files].map(([path, bytes]) => [Buffer.from(path), `${createHash('sha256').update(bytes).digest('hex')}  ${path}\n`])
    .sort(([left], [right]) => Buffer.compare(left, right)).map(([, line]) => line);
  return createHash('sha256').update(lines.join('')).digest('hex');
}

// Pins the official Node.js 24.21.0 archive and the two files taken from it, per architecture.
// Archive checksums: nodejs.org SHASUMS256.txt (also in package-release.mjs); member checksums
// were computed from those archives on 2026-10-05.
export function sandboxNodePackage(arch) {
  const pins = {
    arm64: { bytes: 33679608, digest: '8779b1bde1d39f8d420e3b57aa657b39891af434d3de44a919044cec06785921',
      node: 'dff59da18b6ffe1bf1ca99e1d2af4906080c481740619f5b5098c0fca28bd9b7' },
    x64: { bytes: 37618919, digest: '158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541',
      node: 'ba4e6d110e8c1592a1ecd390f6b05f3da124b13871a5be62b341a07a853c6c32' },
  };
  if (!Object.hasOwn(pins, arch)) throw new Error('Unsupported Node architecture');
  const pin = pins[arch], folder = `node-v24.21.0-win-${arch}`;
  return { name: 'Node.js archive', url: `https://nodejs.org/dist/v24.21.0/${folder}.zip`, bytes: pin.bytes, digest: pin.digest,
    files: [
      { name: 'node.exe', path: `${folder}/node.exe`, digest: pin.node },
      { name: 'LICENSE', path: `${folder}/LICENSE`, digest: 'ed34dd8e3f0a78dbaf00d0444ce8e285b015b765379c2e17880455f70370f8e9' },
    ] };
}

// Reads exactly the named members of a ZIP archive whose bytes were already verified. Only
// stored or deflated members are accepted, each name must occur once, and output is bounded.
export function zipMembers(bytes, wanted, limit = 128 * 1024 ** 2) {
  const found = zipEntries(bytes, name => wanted.includes(name), limit);
  for (const name of wanted) if (!found.has(name)) throw new Error(`The Node archive has no ${name}`);
  return found;
}

// Reads every file member under a folder of a verified ZIP archive, keyed by its path below that
// folder. Folder entries are skipped; a path with an empty, `.` or `..` part is refused.
export function zipTree(bytes, prefix, limit = 128 * 1024 ** 2) {
  const found = zipEntries(bytes, name => name.startsWith(prefix) && !name.endsWith('/'), limit);
  const tree = new Map();
  for (const [name, content] of found) {
    const path = name.slice(prefix.length);
    if (path.split('/').some(part => ['', '.', '..'].includes(part)) || path.includes('\\')) throw new Error('Invalid Node archive');
    tree.set(path, content);
  }
  return tree;
}

// Reads the members a predicate selects, under the same rules as zipMembers.
function zipEntries(bytes, selected, limit) {
  const invalid = () => new Error('Invalid Node archive');
  if (bytes.length < 22) throw invalid();
  let end = bytes.length - 22;
  while (end >= Math.max(0, bytes.length - 65557) && bytes.readUInt32LE(end) !== 0x06054b50) end--;
  if (end < 0) throw invalid();
  const count = bytes.readUInt16LE(end + 10);
  let cursor = bytes.readUInt32LE(end + 16);
  const found = new Map();
  for (let index = 0; index < count; index++) {
    if (cursor + 46 > end || bytes.readUInt32LE(cursor) !== 0x02014b50) throw invalid();
    const flags = bytes.readUInt16LE(cursor + 8), method = bytes.readUInt16LE(cursor + 10);
    const packed = bytes.readUInt32LE(cursor + 20), unpacked = bytes.readUInt32LE(cursor + 24);
    const length = bytes.readUInt16LE(cursor + 28), extra = bytes.readUInt16LE(cursor + 30), comment = bytes.readUInt16LE(cursor + 32);
    const offset = bytes.readUInt32LE(cursor + 42);
    const name = bytes.subarray(cursor + 46, cursor + 46 + length).toString('utf8');
    if (selected(name)) {
      if (found.has(name) || flags & 1 || ![0, 8].includes(method) || unpacked > limit) throw invalid();
      if (offset + 30 > bytes.length || bytes.readUInt32LE(offset) !== 0x04034b50) throw invalid();
      const start = offset + 30 + bytes.readUInt16LE(offset + 26) + bytes.readUInt16LE(offset + 28);
      if (start + packed > bytes.length) throw invalid();
      const input = bytes.subarray(start, start + packed);
      const content = method === 8 ? inflateRawSync(input, { maxOutputLength: limit }) : Buffer.from(input);
      if (content.length !== unpacked) throw invalid();
      found.set(name, content);
    }
    cursor += 46 + length + extra + comment;
  }
  return found;
}

// Returns the private Node's files: the pinned official node.exe and its licence, the seed
// library built for the same architecture, the shipped OpenSSL configuration, the preload for
// sandboxed Node (child-pipes.cjs), the `npm` and `npx`
// sh launchers, the pinned BusyBox as sh.exe (`shell` holds its bytes, else it is read from its
// pin), and npm's tree from the same archive as `node_modules/npm/...`.
export async function sandboxNodeFiles({ arch, seedLibrary, archive = process.env.MINICLAW_NODE_ARCHIVE, bytes,
  config = new URL('../../openssl-seed/openssl.cnf', import.meta.url), shell }) {
  if (!seedLibrary) throw new Error('The OpenSSL seed library was not built for this bundle');
  const pin = sandboxNodePackage(arch);
  const zip = await readPinned(pin, { archive, bytes });
  const entries = zipMembers(zip, pin.files.map(file => file.path));
  const result = new Map();
  for (const file of pin.files) {
    const data = entries.get(file.path);
    if (createHash('sha256').update(data).digest('hex') !== file.digest) throw new Error(`Node checksum mismatch: ${file.name}`);
    result.set(file.name, data);
  }
  if (!(await lstat(seedLibrary)).isFile()) throw new Error('The OpenSSL seed library is not a regular file');
  const seed = await readFile(seedLibrary);
  if (peArch(seed) !== arch) throw new Error(`The OpenSSL seed library is not built for ${arch}`);
  result.set('miniclaw_openssl_seed.dll', seed);
  result.set('openssl.cnf', await readFile(config));
  // Refuses, at once, the piped children Node 24.21.0's libuv cannot create inside the sandbox.
  result.set('child-pipes.cjs', await readFile(new URL('./node-sandbox/child-pipes.cjs', import.meta.url)));
  for (const name of ['npm', 'npx', 'npm-sandbox.cjs']) result.set(name, await readFile(new URL(`./npm-launchers/${name}`, import.meta.url)));
  const busybox = shell ?? (await shellFiles({ arch })).get('busybox.exe');
  if (!busybox || createHash('sha256').update(busybox).digest('hex') !== windowsShellPackage(arch).binary.digest) {
    throw new Error('sh.exe must be the pinned BusyBox');
  }
  result.set('sh.exe', busybox);
  const npm = zipTree(zip, `node-v24.21.0-win-${arch}/${sandboxNpm.prefix}`);
  if (npm.size !== sandboxNpm.files || npmTreeDigest(npm) !== sandboxNpm.digest) throw new Error('npm in the Node archive differs from its pin');
  for (const [path, content] of npm) result.set(sandboxNpm.prefix + path, content);
  return result;
}
