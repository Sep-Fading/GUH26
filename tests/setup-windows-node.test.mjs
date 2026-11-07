// Checks how a Windows bundle gets MiniClaw's private Node for the sandbox.
import assert from 'node:assert/strict';
// Runs the packaging cases.
import { test } from 'node:test';
// Builds archive fixtures in memory.
import { deflateRawSync } from 'node:zlib';
// Writes a disposable seed library fixture.
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
// Locates temporary storage.
import { tmpdir } from 'node:os';
// Joins fixture paths.
import { join } from 'node:path';
// Names the preload script for a child Node.
import { fileURLToPath } from 'node:url';
// The module under test.
import { npmTreeDigest, sandboxNodeFiles, sandboxNodeNames, sandboxNodePackage, sandboxNpm, zipMembers, zipTree } from '../scripts/setup/windows-node-sandbox.mjs';

// Builds a ZIP archive of named members, deflated unless `stored` is set.
function zip(members, { stored = false } = {}) {
  const locals = [], central = [];
  let offset = 0;
  for (const [name, content] of members) {
    const data = stored ? content : deflateRawSync(content);
    const local = Buffer.alloc(30 + name.length);
    local.writeUInt32LE(0x04034b50, 0); local.writeUInt16LE(stored ? 0 : 8, 8);
    local.writeUInt32LE(data.length, 18); local.writeUInt32LE(content.length, 22); local.writeUInt16LE(name.length, 26);
    local.write(name, 30);
    const entry = Buffer.alloc(46 + name.length);
    entry.writeUInt32LE(0x02014b50, 0); entry.writeUInt16LE(stored ? 0 : 8, 10);
    entry.writeUInt32LE(data.length, 20); entry.writeUInt32LE(content.length, 24); entry.writeUInt16LE(name.length, 28);
    entry.writeUInt32LE(offset, 42); entry.write(name, 46);
    locals.push(local, data); central.push(entry);
    offset += local.length + data.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(members.length, 8); end.writeUInt16LE(members.length, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

test('only the named archive members are read, each once', () => {
  const archive = zip([['n/node.exe', Buffer.from('exe')], ['n/LICENSE', Buffer.from('mit')], ['n/node_modules/npm/x.js', Buffer.from('x')]]);
  const found = zipMembers(archive, ['n/node.exe', 'n/LICENSE']);
  assert.deepEqual([...found.keys()].sort(), ['n/LICENSE', 'n/node.exe']);
  assert.equal(found.get('n/node.exe').toString(), 'exe');
  assert.equal(zipMembers(zip([['a', Buffer.from('stored')]], { stored: true }), ['a']).get('a').toString(), 'stored');
  assert.throws(() => zipMembers(archive, ['n/node.exe', 'n/missing']), /no n\/missing/);
  assert.throws(() => zipMembers(zip([['a', Buffer.from('1')], ['a', Buffer.from('2')]]), ['a']), /Invalid Node archive/);
  assert.throws(() => zipMembers(zip([['a', Buffer.alloc(64)]]), ['a'], 16), /Invalid Node archive|buffer|length/i);
  assert.throws(() => zipMembers(Buffer.from('not a zip at all, but long enough'), ['a']), /Invalid Node archive/);
});

test('the private Node is the pinned official 24.21.0 build for the bundle processor', () => {
  for (const arch of ['arm64', 'x64']) {
    const pin = sandboxNodePackage(arch);
    assert.equal(pin.url, `https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-${arch}.zip`);
    assert.deepEqual(pin.files.map(file => file.path), [`node-v24.21.0-win-${arch}/node.exe`, `node-v24.21.0-win-${arch}/LICENSE`]);
    for (const value of [pin.digest, ...pin.files.map(file => file.digest)]) assert.match(value, /^[a-f0-9]{64}$/);
  }
  assert.throws(() => sandboxNodePackage('ia32'), /Unsupported/);
  assert.deepEqual(sandboxNodeNames, ['node.exe', 'LICENSE', 'miniclaw_openssl_seed.dll', 'openssl.cnf', 'child-pipes.cjs', 'npm', 'npx', 'npm-sandbox.cjs', 'sh.exe']);
});

test('npm is read from the archive as a whole tree and pinned by the digest of its listing', async () => {
  const archive = zip([['n/node_modules/npm/bin/npm-cli.js', Buffer.from('cli')], ['n/node_modules/npm/package.json', Buffer.from('{}')],
    ['n/node_modules/corepack/x.js', Buffer.from('no')], ['n/node.exe', Buffer.from('exe')]]);
  const tree = zipTree(archive, 'n/node_modules/npm/');
  assert.deepEqual([...tree.keys()].sort(), ['bin/npm-cli.js', 'package.json']);
  assert.throws(() => zipTree(zip([['n/node_modules/npm/../evil.js', Buffer.from('x')]]), 'n/node_modules/npm/'), /Invalid Node archive/);
  // The backend's digest: SHA-256 over "<sha256>  <path>\n" lines sorted by path.
  const { createHash } = await import('node:crypto');
  const sha = text => createHash('sha256').update(text).digest('hex');
  assert.equal(npmTreeDigest(tree), sha(`${sha('cli')}  bin/npm-cli.js\n${sha('{}')}  package.json\n`));
  assert.equal(npmTreeDigest(new Map([...tree].reverse())), npmTreeDigest(tree));
  assert.deepEqual([sandboxNpm.prefix, sandboxNpm.files], ['node_modules/npm/', 1926]);
  // The launchers are sh scripts that run npm's own scripts with the private Node.
  for (const name of ['npm', 'npx']) {
    const text = await readFile(new URL(`../scripts/setup/npm-launchers/${name}`, import.meta.url), 'utf8');
    assert.ok(text.startsWith('#!/bin/sh\n') && text.includes(`cli="$here/node_modules/npm/bin/${name}-cli.js"\n`)
      && text.endsWith('exec "$here/node.exe" --require "$here/npm-sandbox.cjs" "$cli" "$@"\n'), text);
  }
});

test('the npm preload answers only refused checks of folders above the run\'s own folders', { skip: process.platform === 'win32' || process.getuid?.() === 0 }, async t => {
  // A folder the process may not search stands in for C:\\Users in the sandbox.
  const { chmod, mkdir } = await import('node:fs/promises');
  const { execFileSync } = await import('node:child_process');
  const root = await mkdtemp(join(tmpdir(), 'mc-npm-preload-'));
  await mkdir(join(root, 'locked', 'inner', 'other'), { recursive: true });
  await chmod(join(root, 'locked'), 0o000);
  t.after(async () => { await chmod(join(root, 'locked'), 0o755); await rm(root, { recursive: true, force: true }); });
  const preload = fileURLToPath(new URL('../scripts/setup/npm-launchers/npm-sandbox.cjs', import.meta.url));
  const probe = `const fs=require('fs');const p=require('path');const r=[];
    for (const f of [p.join(${JSON.stringify(root)},'locked','inner'), p.join(${JSON.stringify(root)},'locked','inner','other')]) {
      try { const s=fs.lstatSync(f); r.push(s.isDirectory()&&!s.isSymbolicLink()?'folder':'other'); } catch (e) { r.push(e.code); } }
    fs.promises.lstat(p.join(${JSON.stringify(root)},'locked','inner')).then(s=>{r.push(s.isDirectory()?'folder':'other');console.log(JSON.stringify(r));});`;
  const output = execFileSync(process.execPath, ['--require', preload, '-e', probe], { cwd: tmpdir(), encoding: 'utf8',
    env: { ...process.env, npm_config_cache: join(root, 'locked', 'inner', 'cache') } });
  // Above npm's cache: answered as a folder. Not above any run folder: the refusal stands.
  assert.deepEqual(JSON.parse(output), ['folder', 'EACCES', 'folder']);
});

test('the archive checksum and the seed library are checked before anything is returned', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-node-sandbox-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  await assert.rejects(sandboxNodeFiles({ arch: 'arm64' }), /seed library was not built/);
  const seed = join(folder, 'seed.dll');
  await writeFile(seed, 'not a library');
  await assert.rejects(sandboxNodeFiles({ arch: 'arm64', seedLibrary: seed, bytes: zip([['x', Buffer.from('x')]]) }), /checksum mismatch/);
  // With the real archive (MINICLAW_NODE_ARCHIVE_ARM64 names a local copy) the whole path runs.
  const archive = process.env.MINICLAW_NODE_ARCHIVE_ARM64;
  if (!archive) return t.diagnostic('MINICLAW_NODE_ARCHIVE_ARM64 not set: the real archive was not read');
  await assert.rejects(sandboxNodeFiles({ arch: 'arm64', seedLibrary: seed, archive }), /Invalid PE executable/);
  const library = Buffer.alloc(128);
  library.write('MZ', 0, 'ascii'); library.writeUInt32LE(64, 60); library.writeUInt32LE(0x4550, 64);
  library.writeUInt16LE(0x8664, 68);
  await writeFile(seed, library);
  await assert.rejects(sandboxNodeFiles({ arch: 'arm64', seedLibrary: seed, archive }), /not built for arm64/);
  library.writeUInt16LE(0xaa64, 68);
  await writeFile(seed, library);
  await assert.rejects(sandboxNodeFiles({ arch: 'arm64', seedLibrary: seed, archive, shell: Buffer.from('not busybox') }), /pinned BusyBox/);
  const shell = process.env.MINICLAW_BUSYBOX_BINARY_ARM64;
  if (!shell) return t.diagnostic('MINICLAW_BUSYBOX_BINARY_ARM64 not set: the npm tree was not read');
  const files = await sandboxNodeFiles({ arch: 'arm64', seedLibrary: seed, archive, shell: await readFile(shell) });
  assert.deepEqual([...files.keys()].slice(0, sandboxNodeNames.length), sandboxNodeNames);
  const npm = [...files.keys()].slice(sandboxNodeNames.length);
  assert.equal(npm.length, sandboxNpm.files);
  assert.ok(npm.every(path => path.startsWith('node_modules/npm/')) && npm.includes('node_modules/npm/bin/npm-cli.js'));
  assert.equal(files.get('openssl.cnf').toString(), await readFile(new URL('../openssl-seed/openssl.cnf', import.meta.url), 'utf8'));
  assert.equal(files.get('child-pipes.cjs').toString(), await readFile(new URL('../scripts/setup/node-sandbox/child-pipes.cjs', import.meta.url), 'utf8'));
});
