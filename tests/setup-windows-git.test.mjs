// Verifies embedded Git provenance, archive boundaries and failed installation cleanup.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readdir, rename, rm, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { deflateRawSync } from 'node:zlib';
import { windowsGitPackage, gitArchiveEntries, installWindowsGit } from '../scripts/setup/windows-git.mjs';

// Creates small ZIP fixtures with matching local and central names.
function zip(entries) {
  const chunks = [], central = []; let offset = 0;
  for (const [name, text, mode = 0] of entries) {
    const path = Buffer.from(name), plain = Buffer.from(text), packed = deflateRawSync(plain);
    const local = Buffer.alloc(30); local.writeUInt32LE(0x04034b50); local.writeUInt16LE(8, 8);
    local.writeUInt32LE(packed.length, 18); local.writeUInt32LE(plain.length, 22); local.writeUInt16LE(path.length, 26);
    chunks.push(local, path, packed);
    const header = Buffer.alloc(46); header.writeUInt32LE(0x02014b50); header.writeUInt16LE(8, 10);
    header.writeUInt32LE(packed.length, 20); header.writeUInt32LE(plain.length, 24); header.writeUInt16LE(path.length, 28);
    header.writeUInt32LE(mode * 65536 >>> 0, 38); header.writeUInt32LE(offset, 42);
    central.push(header, path); offset += local.length + path.length + packed.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22); end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...chunks, directory, end]);
}

test('Git packages pin official architecture-specific releases', () => {
  assert.equal(windowsGitPackage('arm64').digest, 'cb3b0f2d486ea52673227151a5baf5bc13861ff80e74e94e46d614d1bfcd5c06');
  assert.equal(windowsGitPackage('x64').digest, '064b440ff870ed5198527e8f3a92cdf5bd2fd0fedf5e718af95e3fdaddeff718');
  assert.equal(windowsGitPackage('x64').runtimePrefix, 'ucrt64');
  assert.equal(windowsGitPackage('arm64').runtimePrefix, 'clangarm64');
  assert.throws(() => windowsGitPackage('ia32'), /architecture/);
});

test('Git archive preserves helpers and CA but rejects aliases and links', () => {
  const entries = gitArchiveEntries(zip([['cmd/git.exe', 'binary'], ['usr/bin/helper', '#!/bin/sh'], ['etc/gitconfig', 'do not import']]));
  assert.equal(entries.get('usr/bin/helper').toString(), '#!/bin/sh');
  assert.equal(entries.has('etc/gitconfig'), false);
  for (const name of ['../outside', 'C:/outside', 'cmd/GIT.exe.', 'cmd/aux.txt', 'cmd/file:stream', 'cmd\\bad', 'COM¹.log', 'LPT²']) {
    assert.throws(() => gitArchiveEntries(zip([[name, 'bad']])), /archive/i);
  }
  assert.throws(() => gitArchiveEntries(zip([['cmd/git.exe', 'a'], ['CMD/GIT.EXE', 'b']])), /archive/i);
  assert.throws(() => gitArchiveEntries(zip([['link', 'target', 0o120777]])), /archive/i);
  assert.throws(() => gitArchiveEntries(Buffer.from('truncated')), /archive/i);
});

test('failed Git extraction never publishes configuration or partial runtime', async t => {
  const data = await mkdtemp(join(tmpdir(), 'mc-git-install-'));
  t.after(() => rm(data, { recursive: true, force: true }));
  const archive = join(data, 'fixture.zip'); await writeFile(archive, 'fixture');
  await assert.rejects(installWindowsGit({ runtime: data, data, archive, arch: 'arm64', pin: windowsGitBundle('arm64'),
    verify: async () => new Map(), secure: async path => mkdir(path),
    materialize: async root => { await writeFile(join(root, 'partial'), 'partial'); throw new Error('forced extraction failure'); },
  }), /forced extraction failure/);
  await assert.rejects(readFile(join(data, 'windows-git.json')), { code: 'ENOENT' });
  assert.deepEqual(await readdir(join(data, 'windows-git-runtime')), []);
});

// R1 (v3 re-review): a caller that forgets the pin must not silently get the official MinGit, which fails in the sandbox.
test('installing Git requires an explicit pin and has no official MinGit default', async t => {
  const data = await mkdtemp(join(tmpdir(), 'mc-git-pin-'));
  t.after(() => rm(data, { recursive: true, force: true }));
  const archive = join(data, 'fixture.zip'); await writeFile(archive, 'fixture');
  await assert.rejects(installWindowsGit({ runtime: data, data, archive, arch: 'arm64',
    verify: async () => assert.fail('the archive was read'), secure: async () => assert.fail('a runtime folder was created') }), /explicit pin/);
  await assert.rejects(readdir(join(data, 'windows-git-runtime')), { code: 'ENOENT' });
  await assert.rejects(readFile(join(data, 'windows-git.json')), { code: 'ENOENT' });
});

// --- Bundled MiniClaw Git build (carried inside the release; never downloaded) ---
import { createHash } from 'node:crypto';
import { windowsGitBundle, gitBundleFiles, bundledGitArchive, replaceWindowsGit, recoverWindowsGit } from '../scripts/setup/windows-git-bundle.mjs';

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { nativeArtifacts } from './support/provider-fixture-paths.mjs';

// On Windows the private record and runtime go through the built backend, as in production.
const windows = process.platform === 'win32';
const nativeBackend = windows ? nativeArtifacts(fileURLToPath(new URL('../', import.meta.url)), process.env.MINICLAW_TEST_TARGET).backend : undefined;
const nativeOptions = windows ? { backend: nativeBackend } : { secure: async path => mkdir(path) };
// An update writes the earlier record through the backend too; the fixture runtime has none of its own.
const updateBackend = windows ? { backend: nativeBackend } : {};
// Creates a data folder the backend accepts as private (Windows) or a plain temporary one.
async function privateData(t, prefix) {
  const parent = await mkdtemp(join(tmpdir(), prefix));
  t.after(() => rm(parent, { recursive: true, force: true }));
  if (!windows) return parent;
  const data = join(parent, 'data');
  await promisify(execFile)(nativeBackend, ['--secure-data-dir', data]);
  return data;
}

const licenseNames = ['git-COPYING.txt', 'NOTICE.txt', 'SOURCE-OFFER.txt', 'git-lpac-path.patch', 'git-lpac-leading-directories.patch',
  'git-lpac-console-handle.patch'];
// A small runtime ZIP with the bundled layout and a pin computed for it.
function bundleFixture(label = 'one') {
  const bytes = zip([['git/bin/git.exe', `git ${label}`], ['git/etc/ssl/certs/ca-bundle.crt', 'ca'],
    ['git/libexec/git-core/git-remote-https.exe', 'helper'], ...licenseNames.map(name => [`git/licenses/${name}`, `${name} text`])]);
  const pin = { ...windowsGitBundle('arm64'), bytes: bytes.length, digest: createHash('sha256').update(bytes).digest('hex') };
  return { bytes, pin };
}

test('bundled Git pins name a private git/ runtime per architecture and its MiniClaw release asset', () => {
  for (const arch of ['arm64', 'x64']) {
    const pin = windowsGitBundle(arch);
    assert.equal(pin.file, `miniclaw-git-2.56.0-lpac5-${arch}.zip`);
    assert.equal(pin.runtimePrefix, 'git');
    // No generic download URL (setup never downloads Git); packaging may fetch MiniClaw's own v0.1.5 release asset.
    assert.equal(pin.url, undefined);
    assert.deepEqual(pin.release, { repository: 'Sep-Fading/miniclaw-windows', tag: 'v0.1.5', asset: { arm64: 616891706, x64: 616891738 }[arch] });
    assert.ok(Number.isSafeInteger(pin.bytes) && pin.bytes > 1024 ** 2, `${arch} size is pinned`);
    assert.match(pin.digest, /^[a-f0-9]{64}$/, `${arch} digest is pinned`);
  }
  assert.notEqual(windowsGitBundle('arm64').digest, windowsGitBundle('x64').digest);
  // lpac4 (2026-10-05): the same Git build; OpenSSL rebuilt with _WIN32_WINNT=0x0A00 so libcrypto seeds from
  // BCryptGenRandom, which works inside the sandbox (lpac3's CryptoAPI seeding failed: "Insufficient randomness").
  // lpac5 (2026-10-05): Git recompiled with git-lpac-console-handle.patch (no 0xC0000008 exit on the sandboxed terminal).
  assert.deepEqual([windowsGitBundle('arm64').bytes, windowsGitBundle('arm64').digest],
    [29526262, 'ca7c05a029d02682bb90b2b8eeb9adde157bfa3c5e389b53ed51e54201fc19a3']);
  assert.deepEqual([windowsGitBundle('x64').bytes, windowsGitBundle('x64').digest],
    [29632338, '66f3c549d5a5d995c562efdf4c722a468006e683d0c15e0d3a36f1f744f30d1c']);
  // lpac2 shipped vcruntime140/msvcp140 beside git.exe; lpac3 cannot do HTTPS in the sandbox; lpac4 exits
  // 0xC0000008 when its stdout is the sandboxed terminal. All are superseded.
  for (const digest of ['f255f30cb7dfa29d6b748dcac34c514395f5bdf01f84cdc83260d61db587ae17', '6693d5e04aced9683052bec2ec7c08c689db91acd6334d06257ed4ad979f52d5',
    '2bf25eb835516227d62cb048f679882de5f25bccaae5e6f9c1b15951bfc2b7bd', '393cbae5cd761860e10341dab6a68b323400dd2464941390ac9cbe786cd6647a',
    '4ed169a9b79c868af864b2c3681695833dcc30e5f9b06a72fb4459a450b3315c', 'f3f1525751a6f82fbde8da3c1c7b09533a69789ad7c5d2d40f25e76177766d94']) {
    assert.ok(!['arm64', 'x64'].some(arch => windowsGitBundle(arch).digest === digest));
  }
  // The rejected first ARM64 candidate (independent review 2026-10-04) is never pinned.
  assert.notEqual(windowsGitBundle('arm64').digest, 'b7fbe019aa72dbdaeeccb68bb0345b5d910f216349dee541d0dfb1443c74d125');
  for (const arch of ['ia32', 'constructor']) assert.throws(() => windowsGitBundle(arch), /architecture/);
});

test('without MINICLAW_GIT_ARCHIVE, packaging fetches the pinned release asset; the pin stays the gate', async t => {
  const { bytes, pin } = bundleFixture();
  const requests = [];
  // `null` means unset: an omitted or undefined option would take MINICLAW_GIT_ARCHIVE from this machine.
  const fetcher = async url => { requests.push(String(url)); return new Response(bytes); };
  const files = await gitBundleFiles({ arch: 'arm64', archive: null, pin, fetcher, env: {} });
  assert.ok(files.get(pin.file).equals(bytes));
  assert.deepEqual(requests, [`https://github.com/Sep-Fading/miniclaw-windows/releases/download/v0.1.5/${pin.file}`]);
  // Other bytes from the release never pass the pin.
  await assert.rejects(gitBundleFiles({ arch: 'arm64', archive: null, pin, env: {},
    fetcher: async () => new Response(Buffer.alloc(bytes.length)) }), /checksum/);
  // A failed download names both ways on: a token for the private repository, or the offline archive.
  await assert.rejects(gitBundleFiles({ arch: 'arm64', archive: null, pin, env: {}, fetcher: async () => new Response('', { status: 404 }) }),
    error => /MINICLAW_GIT_ARCHIVE/.test(error.message) && /GH_TOKEN/.test(error.message));
  // The offline archive (MINICLAW_GIT_ARCHIVE) wins and nothing is fetched.
  const root = await mkdtemp(join(tmpdir(), 'mc-git-offline-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, pin.file), bytes);
  const offline = async () => assert.fail('the release was fetched although an archive was given');
  assert.ok((await gitBundleFiles({ arch: 'arm64', archive: join(root, pin.file), pin, fetcher: offline })).get(pin.file).equals(bytes));
  await assert.rejects(gitBundleFiles({ arch: 'arm64', archive: null, pin, fetcher: offline, bytes: Buffer.concat([bytes, Buffer.from('x')]) }), /checksum/);
  // The other architecture's archive cannot satisfy this architecture's pin.
  await assert.rejects(gitBundleFiles({ arch: 'x64', archive: null, bytes, fetcher: offline }), /checksum|not pinned/);
});

// The Git corresponding source a release publishes beside itself; packaging may take it from the v0.1.5 release.
test('the Git corresponding source is pinned to its v0.1.5 release asset and unpacked only after the pin matches', async t => {
  const { gitSourceRelease, gitCorrespondingSource } = await import('../scripts/setup/windows-git-bundle.mjs');
  assert.deepEqual({ ...gitSourceRelease }, { name: 'Git corresponding source',
    file: 'miniclaw-0.1.5-6457c45-windows-arm64-git-corresponding-source.tar.gz', bytes: 154469901, algorithm: 'sha256',
    digest: 'b3e5fb5bafbcbb459f77d4a487cb0711cd260319bcb2946c294354e04cfdfea0',
    release: { repository: 'Sep-Fading/miniclaw-windows', tag: 'v0.1.5', asset: 616891793 } });
  const tar = process.platform === 'win32' ? 'tar.exe' : '/usr/bin/tar';
  const root = await mkdtemp(join(tmpdir(), 'mc-git-source-release-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'input/git-corresponding-source'), { recursive: true });
  await writeFile(join(root, 'input/git-corresponding-source/SOURCE-MANIFEST.json'), '{}');
  await promisify(execFile)(tar, ['-czf', join(root, 'source.tar.gz'), '-C', join(root, 'input'), 'git-corresponding-source']);
  const archive = await readFile(join(root, 'source.tar.gz'));
  const pin = { ...gitSourceRelease, bytes: archive.length, digest: createHash('sha256').update(archive).digest('hex') };
  // A configured folder (MINICLAW_GIT_SOURCE) is used as it is. `folder: null` below means unset, whatever this machine's env says.
  assert.equal(await gitCorrespondingSource({ folder: join(root, 'mine'), directory: join(root, 'unused'), tar, pin,
    fetcher: async () => assert.fail('fetched although a folder was given') }), join(root, 'mine'));
  // Other bytes are refused before anything is unpacked.
  await mkdir(join(root, 'bad'));
  await assert.rejects(gitCorrespondingSource({ folder: null, directory: join(root, 'bad'), tar, pin, env: {},
    fetcher: async () => new Response(Buffer.alloc(archive.length)) }), /checksum/);
  assert.deepEqual(await readdir(join(root, 'bad')), []);
  await assert.rejects(gitCorrespondingSource({ folder: null, directory: join(root, 'bad'), tar, pin, env: {},
    fetcher: async () => new Response('', { status: 404 }) }), error => /MINICLAW_GIT_SOURCE/.test(error.message) && /GH_TOKEN/.test(error.message));
  const requests = [];
  await mkdir(join(root, 'work'));
  const folder = await gitCorrespondingSource({ folder: null, directory: join(root, 'work'), tar, pin, env: {},
    fetcher: async url => { requests.push(String(url)); return new Response(archive); } });
  assert.deepEqual(requests, [`https://github.com/Sep-Fading/miniclaw-windows/releases/download/v0.1.5/${pin.file}`]);
  assert.equal(folder, join(root, 'work/git-source/git-corresponding-source'));
  assert.equal(await readFile(join(folder, 'SOURCE-MANIFEST.json'), 'utf8'), '{}');
});

test('bundled Git files are the verified archive plus its licence, patch, notice and offer texts', async t => {
  const { bytes, pin } = bundleFixture();
  const files = await gitBundleFiles({ arch: 'arm64', pin, bytes });
  assert.deepEqual([...files.keys()].sort(), [pin.file, ...licenseNames.map(name => `licenses/${name}`)].sort());
  assert.ok(files.get(pin.file).equals(bytes));
  const root = await mkdtemp(join(tmpdir(), 'mc-git-bundle-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const archive = join(root, pin.file);
  await writeFile(archive, bytes);
  assert.equal((await gitBundleFiles({ arch: 'arm64', pin, archive })).get(pin.file).length, bytes.length);
  // A runtime without its written source offer is not shipped.
  const bare = zip([['git/bin/git.exe', 'git'], ['git/etc/ssl/certs/ca-bundle.crt', 'ca']]);
  await assert.rejects(gitBundleFiles({ arch: 'arm64', bytes: bare,
    pin: { ...pin, bytes: bare.length, digest: createHash('sha256').update(bare).digest('hex') } }), /licenses/);
  // lpac5: a runtime without the third patch (git-lpac-console-handle.patch) is not shipped either.
  const twoPatches = zip([['git/bin/git.exe', 'git'], ['git/etc/ssl/certs/ca-bundle.crt', 'ca'],
    ...licenseNames.filter(name => name !== 'git-lpac-console-handle.patch').map(name => [`git/licenses/${name}`, `${name} text`])]);
  await assert.rejects(gitBundleFiles({ arch: 'arm64', bytes: twoPatches,
    pin: { ...pin, bytes: twoPatches.length, digest: createHash('sha256').update(twoPatches).digest('hex') } }), /git-lpac-console-handle\.patch/);
});

test('setup installs the bundled build under git/ and publishes its record last', async t => {
  const data = await privateData(t, 'mc-git-bundle-install-');
  const { bytes, pin } = bundleFixture();
  const archive = join(data, pin.file); await writeFile(archive, bytes);
  const result = await installWindowsGit({ runtime: data, data, archive, arch: 'arm64', pin, ...nativeOptions });
  assert.equal(result.program, 'git/bin/git.exe');
  assert.equal(result.execPath, 'git/libexec/git-core');
  assert.deepEqual(JSON.parse(await readFile(join(data, 'windows-git.json'), 'utf8')),
    { root: result.root, program: 'git/bin/git.exe', execPath: 'git/libexec/git-core' });
  assert.equal(await readFile(join(result.root, 'git/bin/git.exe'), 'utf8'), 'git one');
  // The official MinGit pin does not accept the bundled build.
  await assert.rejects(installWindowsGit({ runtime: data, data, archive, arch: 'arm64', pin: windowsGitPackage('arm64'), ...nativeOptions }), /checksum/);
});

test('an update switches Git as one recoverable step: rollback restores the earlier runtime, commit removes it', async t => {
  const data = await privateData(t, 'mc-git-bundle-update-');
  const install = options => installWindowsGit({ ...options, ...nativeOptions });
  const first = bundleFixture('one'), second = bundleFixture('two');
  const archive = join(data, 'a.zip'); await writeFile(archive, first.bytes);
  const old = await installWindowsGit({ runtime: data, data, archive, arch: 'arm64', pin: first.pin, ...nativeOptions });
  const before = await readFile(join(data, 'windows-git.json'), 'utf8');
  await writeFile(archive, second.bytes);
  const swap = await replaceWindowsGit({ runtime: data, data, arch: 'arm64', pin: second.pin, archive, install, ...updateBackend });
  assert.equal(JSON.parse(await readFile(join(data, 'windows-git.json'), 'utf8')).root, swap.installed.root);
  assert.equal(await readFile(join(swap.installed.root, 'git/bin/git.exe'), 'utf8'), 'git two');
  await swap.rollback();
  assert.equal(await readFile(join(data, 'windows-git.json'), 'utf8'), before);
  assert.deepEqual(await readdir(join(data, 'windows-git-runtime')), [old.root.split(/[\\/]/).at(-1)]);
  const again = await replaceWindowsGit({ runtime: data, data, arch: 'arm64', pin: second.pin, archive, install, ...updateBackend });
  await again.commit();
  assert.equal(JSON.parse(await readFile(join(data, 'windows-git.json'), 'utf8')).root, again.installed.root);
  assert.deepEqual(await readdir(join(data, 'windows-git-runtime')), [again.installed.root.split(/[\\/]/).at(-1)]);
  assert.deepEqual((await readdir(data)).filter(name => name.startsWith('windows-git.json.')), []);
  // A failed install leaves the live record and runtime untouched.
  await writeFile(archive, 'tampered');
  const live = await readFile(join(data, 'windows-git.json'), 'utf8');
  await assert.rejects(replaceWindowsGit({ runtime: data, data, arch: 'arm64', pin: second.pin, archive, install, ...updateBackend }), /checksum/);
  assert.equal(await readFile(join(data, 'windows-git.json'), 'utf8'), live);
  assert.equal(bundledGitArchive('/runtime', 'arm64').replaceAll('\\', '/'), '/runtime/bin/git/miniclaw-git-2.56.0-lpac5-arm64.zip');
  assert.equal(bundledGitArchive('/runtime', 'x64').replaceAll('\\', '/'), '/runtime/bin/git/miniclaw-git-2.56.0-lpac5-x64.zip');
});

// R3 (v3 re-review): an update interrupted between its record steps must neither lose the live record nor orphan a runtime.
const runtimeNames = async data => (await readdir(join(data, 'windows-git-runtime'))).sort();
const recordRoot = async data => JSON.parse(await readFile(join(data, 'windows-git.json'), 'utf8')).root;
const leftovers = async data => (await readdir(data)).filter(name => name.startsWith('windows-git.json.'));
const leaf = path => path.split(/[\\/]/).at(-1);
// Installs fixture `label` with its own pin, optionally writing the record to another name.
async function installFixture(data, label, record) {
  const { bytes, pin } = bundleFixture(label);
  const archive = join(data, `${label}.zip`); await writeFile(archive, bytes);
  return { pin, archive, installed: await installWindowsGit({ runtime: data, data, archive, arch: 'arm64', pin,
    ...(record ? { record: join(data, record) } : {}), ...nativeOptions }) };
}
const replaceOptions = (data, { pin, archive }) => ({ runtime: data, data, arch: 'arm64', pin, archive,
  install: options => installWindowsGit({ ...options, ...nativeOptions }), ...updateBackend });

test('an update interrupted between the record renames is rolled back on the next update without orphaned runtimes', async t => {
  for (const finish of ['commit', 'rollback']) {
    const data = await privateData(t, `mc-git-crash-${finish}-`);
    // The earlier update protocol renamed the live record away, then the new one in; a crash in between left no record.
    const a = await installFixture(data, 'one');
    const b = await installFixture(data, 'two', 'windows-git.json.next');
    await rename(join(data, 'windows-git.json'), join(data, 'windows-git.json.previous'));
    const c = await installFixture(data, 'three', 'unused.json').then(async value => {
      await rm(join(data, 'unused.json')); await rm(value.installed.root, { recursive: true }); return value;
    });
    const swap = await replaceWindowsGit(replaceOptions(data, c));
    assert.equal(await recordRoot(data), swap.installed.root);
    await swap[finish]();
    const expected = finish === 'commit' ? swap.installed.root : a.installed.root;
    assert.equal(await recordRoot(data), expected, `${finish} ends on the right record`);
    assert.deepEqual(await runtimeNames(data), [leaf(expected)], `${finish} leaves no orphaned runtime (${leaf(b.installed.root)} was half-installed)`);
    assert.deepEqual(await leftovers(data), []);
  }
});

test('recovery keeps the live record and removes every runtime it does not name', async t => {
  // Crash before the switch: the unpublished next record and its runtime go away.
  let data = await privateData(t, 'mc-git-recover-before-');
  const a = await installFixture(data, 'one');
  await installFixture(data, 'two', 'windows-git.json.next');
  await mkdir(join(data, 'windows-git-runtime', 'not-generated'));
  await recoverWindowsGit({ data });
  assert.equal(await recordRoot(data), a.installed.root);
  assert.deepEqual(await runtimeNames(data), [leaf(a.installed.root), 'not-generated'].sort(), 'only generated runtime folders are removed');
  assert.deepEqual(await leftovers(data), []);
  // Crash after the switch, before commit or rollback: the switch is finished.
  data = await privateData(t, 'mc-git-recover-after-');
  await installFixture(data, 'one');
  const second = bundleFixture('two'), archive = join(data, 'two.zip'); await writeFile(archive, second.bytes);
  const swap = await replaceWindowsGit(replaceOptions(data, { pin: second.pin, archive }));
  await recoverWindowsGit({ data });
  assert.equal(await recordRoot(data), swap.installed.root);
  assert.deepEqual(await runtimeNames(data), [leaf(swap.installed.root)]);
  assert.deepEqual(await leftovers(data), []);
  // The earlier protocol's half-switched state: the earlier record is restored and the unpublished runtime removed.
  data = await privateData(t, 'mc-git-recover-half-');
  const earlier = await installFixture(data, 'one');
  await installFixture(data, 'two', 'windows-git.json.next');
  await rename(join(data, 'windows-git.json'), join(data, 'windows-git.json.previous'));
  await recoverWindowsGit({ data });
  assert.equal(await recordRoot(data), earlier.installed.root);
  assert.deepEqual(await runtimeNames(data), [leaf(earlier.installed.root)]);
  assert.deepEqual(await leftovers(data), []);
  // Nothing installed yet: recovery is a no-op.
  data = await privateData(t, 'mc-git-recover-empty-');
  await recoverWindowsGit({ data });
  await assert.rejects(readFile(join(data, 'windows-git.json')), { code: 'ENOENT' });
});

test('the live record is never absent while an update switches or rolls back Git', async t => {
  const data = await privateData(t, 'mc-git-switch-');
  const a = await installFixture(data, 'one');
  const second = bundleFixture('two'), archive = join(data, 'two.zip'); await writeFile(archive, second.bytes);
  // After every rename the record is what a crash at that moment would leave behind.
  const states = [];
  const observed = async (from, to) => { await rename(from, to); states.push(await recordRoot(data).catch(() => null)); };
  const swap = await replaceWindowsGit({ ...replaceOptions(data, { pin: second.pin, archive }), rename: observed });
  await swap.rollback();
  assert.ok(states.length >= 2, 'the switch and the rollback rename the record');
  assert.deepEqual(states.filter(root => root !== a.installed.root && root !== swap.installed.root), [],
    'every intermediate state names the earlier or the new runtime');
  assert.equal(await recordRoot(data), a.installed.root);
});

test('a Windows plan installs the bundled Git without downloading it', async () => {
  const { planSetup, GiB } = await import('../scripts/setup/plan.mjs');
  const { review, stageList } = await import('../scripts/setup/view.mjs');
  for (const arch of ['arm64', 'x64']) {
    const plan = planSetup({ port: 8787, providers: { codex: true, claude: false }, vmEnabled: false },
      { platform: 'win32', arch, cpuCount: 8, memoryBytes: 8 * GiB, diskBytes: 256 * GiB, freeBytes: 150 * GiB });
    assert.deepEqual(plan.packages, []);
    assert.equal(plan.downloadBytes, 0);
    assert.equal(plan.windowsGit.file, windowsGitBundle(arch).file);
    assert.ok(plan.retainedPackageBytes >= windowsGitBundle(arch).expandedBytes);
    assert.deepEqual(stageList(plan), ['build', 'windows-git', 'windows-network']);
    const text = review(plan, { destination: 'C:\\MiniClaw', cache: 'C:\\Cache' });
    assert.match(text, /Git 2\.56\.0.*bundled.*read.only/);
    assert.doesNotMatch(text, /Git 2\.56\.0.*downloads/);
    // Even a plan that lost its Git entry never claims that setup downloads Git (v3 re-review R1).
    const bare = review({ ...plan, windowsGit: null, packages: [{ id: 'windows-git', bytes: 39602073 }] }, { destination: 'C:\\MiniClaw', cache: 'C:\\Cache' });
    assert.match(bare, /Workspace Git.*bundled/);
    assert.doesNotMatch(bare, /download/i);
  }
});
