// Prepares a checksum-pinned Git runtime without reading installed Git or account configuration.
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { inflateRawSync } from 'node:zlib';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { writePrivateFile } from './private-file.mjs';

// Pins the official noninteractive Git distribution for each supported Windows architecture.
// Setup does not use it (it cannot read its working folder inside the sandbox); CI and the native
// acceptance script pass it explicitly as a test fixture.
export function windowsGitPackage(arch) {
  const pins = {
    arm64: { prefix: 'clangarm64', suffix: 'arm64', bytes: 37900737, digest: 'cb3b0f2d486ea52673227151a5baf5bc13861ff80e74e94e46d614d1bfcd5c06' },
    x64: { prefix: 'ucrt64', suffix: '64-bit', bytes: 39602073, digest: '064b440ff870ed5198527e8f3a92cdf5bd2fd0fedf5e718af95e3fdaddeff718' },
  };
  const pin = pins[arch];
  if (!pin) throw new Error('Unsupported Git architecture');
  const file = `MinGit-2.56.0-${pin.suffix}.zip`;
  return { id: 'windows-git', name: 'Git 2.56.0 for workspace commands', file,
    url: `https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.1/${file}`,
    runtimePrefix: pin.prefix, algorithm: 'sha256', bytes: pin.bytes, digest: pin.digest, expandedBytes: 128 * 1024 ** 2 };
}

// Validates the complete ZIP before writing; only ordinary, unambiguous files are materialized.
export function gitArchiveEntries(bytes) {
  const invalid = () => new Error('Invalid Git archive');
  const result = new Map(), names = new Set();
  if (bytes.length < 22) throw invalid();
  let end = bytes.length - 22;
  while (end >= Math.max(0, bytes.length - 65557) && bytes.readUInt32LE(end) !== 0x06054b50) end--;
  if (end < 0 || bytes.readUInt32LE(end) !== 0x06054b50 || end + 22 + bytes.readUInt16LE(end + 20) !== bytes.length
    || bytes.readUInt16LE(end + 4) || bytes.readUInt16LE(end + 6)) throw invalid();
  const count = bytes.readUInt16LE(end + 10), size = bytes.readUInt32LE(end + 12);
  let cursor = bytes.readUInt32LE(end + 16), total = 0;
  if (count > 10000 || count !== bytes.readUInt16LE(end + 8) || cursor + size !== end) throw invalid();
  for (let index = 0; index < count; index++) {
    if (cursor + 46 > end || bytes.readUInt32LE(cursor) !== 0x02014b50) throw invalid();
    const flags = bytes.readUInt16LE(cursor + 8), method = bytes.readUInt16LE(cursor + 10);
    const packed = bytes.readUInt32LE(cursor + 20), unpacked = bytes.readUInt32LE(cursor + 24);
    const length = bytes.readUInt16LE(cursor + 28), extra = bytes.readUInt16LE(cursor + 30), comment = bytes.readUInt16LE(cursor + 32);
    const attributes = bytes.readUInt32LE(cursor + 38), offset = bytes.readUInt32LE(cursor + 42);
    if (cursor + 46 + length + extra + comment > end || flags & 1 || ![0, 8].includes(method)) throw invalid();
    const rawName = bytes.subarray(cursor + 46, cursor + 46 + length), name = rawName.toString('utf8');
    const directory = name.endsWith('/'), path = directory ? name.slice(0, -1) : name;
    const kind = (attributes >>> 16) & 0o170000;
    if (!path || name.includes('\\') || !Buffer.from(name).equals(rawName)
      || path.split('/').some(part => !part || part === '.' || part === '..' || /[\x00-\x1f<>:"|?*]/.test(part)
        || /[ .]$/.test(part) || /^(con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/i.test(part))
      || (kind && kind !== (directory ? 0o040000 : 0o100000)) || names.has(path.toLowerCase())) throw invalid();
    names.add(path.toLowerCase());
    if (offset + 30 > bytes.length || bytes.readUInt32LE(offset) !== 0x04034b50) throw invalid();
    const localLength = bytes.readUInt16LE(offset + 26), localExtra = bytes.readUInt16LE(offset + 28);
    const start = offset + 30 + localLength + localExtra;
    if (!bytes.subarray(offset + 30, offset + 30 + localLength).equals(rawName) || start + packed > end
      || unpacked > 64 * 1024 ** 2 || (total += unpacked) > 256 * 1024 ** 2) throw invalid();
    if (!directory && path.toLowerCase() !== 'etc/gitconfig') {
      const input = bytes.subarray(start, start + packed);
      const content = method === 8 ? inflateRawSync(input, { maxOutputLength: 64 * 1024 ** 2 }) : input;
      if (content.length !== unpacked) throw invalid();
      result.set(path, content);
    }
    cursor += 46 + length + extra + comment;
  }
  if (cursor !== end) throw invalid();
  return result;
}

// Checks archive provenance in memory so extraction cannot reopen a changed download.
async function verifiedEntries(archive, pin) {
  const bytes = await readFile(archive);
  if (bytes.length !== pin.bytes || createHash('sha256').update(bytes).digest('hex') !== pin.digest) {
    throw new Error('Git archive checksum mismatch');
  }
  return gitArchiveEntries(bytes);
}

// Writes new ordinary files, materializing every package entry independently.
async function materializeEntries(root, entries) {
  for (const [name, bytes] of entries) {
    const path = join(root, name);
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    await writeFile(path, bytes, { flag: 'wx', mode: 0o600 });
  }
}

// Publishes private configuration only after the entire verified runtime has been written.
// `pin` is required and selects the archive: production passes the bundled MiniClaw build
// (windowsGitBundle); the official MinGit pin above fails inside the sandbox and is only a test
// fixture, so it is never a silent default. `record` lets an update write the new record beside the live one.
export async function installWindowsGit({ runtime, data, archive, arch, backend = join(runtime, 'bin/miniclaw-server.exe'),
  pin, record = join(data, 'windows-git.json'),
  verify = verifiedEntries, materialize = materializeEntries,
  secure = path => promisify(execFile)(backend, ['--secure-data-dir', path]),
}) {
  if (!pin || typeof pin !== 'object') throw new Error(`Installing the ${arch} Git runtime needs an explicit pin`);
  const entries = await verify(archive, pin);
  const parent = join(data, 'windows-git-runtime');
  await mkdir(parent, { recursive: true, mode: 0o700 });
  const root = join(parent, randomUUID());
  const prefix = pin.runtimePrefix;
  try {
    await secure(root);
    await materialize(root, entries);
    for (const path of [`${prefix}/bin/git.exe`, `${prefix}/etc/ssl/certs/ca-bundle.crt`]) {
      if (!(await readFile(join(root, path))).length) throw new Error('Git runtime is incomplete');
    }
    const config = { root, program: `${prefix}/bin/git.exe`, execPath: `${prefix}/libexec/git-core` };
    await writePrivateFile(runtime, record, JSON.stringify(config), { backend });
    return { version: '2.56.0', ...config };
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    throw error;
  }
}
