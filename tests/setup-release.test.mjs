import assert from 'node:assert/strict';
import { access, copyFile, mkdir, mkdtemp, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { auditReleaseArchive, auditReleaseTree, checksums, closure, extractNodeRuntime, node, releasePlatform } from '../scripts/package-release.mjs';

const exec = promisify(execFile);
const tar = process.platform === 'win32' ? 'tar.exe' : '/usr/bin/tar';

test('only the packages the scripts need at run time go into a release', async t => {
  const modules = await mkdtemp(join(tmpdir(), 'mc-modules-'));
  t.after(() => rm(modules, { recursive: true, force: true }));
  const pack = async (name, dependencies = {}) => { await mkdir(join(modules, name), { recursive: true });
    await writeFile(join(modules, name, 'package.json'), JSON.stringify({ name, dependencies })); };
  await pack('@clack/prompts', { '@clack/core': '1', picocolors: '1' });
  await pack('@clack/core', { picocolors: '1', sisteransi: '1' });
  await pack('picocolors'); await pack('sisteransi'); await pack('vite', { rollup: '1' }); await pack('rollup');
  assert.deepEqual(await closure(modules, ['@clack/prompts']), ['@clack/core', '@clack/prompts', 'picocolors', 'sisteransi']);
});

test('the manifest records every prebuilt file by its relative path, never itself', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-sums-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  await mkdir(join(folder, 'bundle/bin'), { recursive: true });
  await writeFile(join(folder, 'bundle/bin/miniclaw-server'), 'a');
  await writeFile(join(folder, 'miniclaw-worker'), 'b');
  await writeFile(join(folder, 'manifest.json'), '{}');
  const files = await checksums(folder);
  assert.deepEqual(Object.keys(files), ['bundle/bin/miniclaw-server', 'miniclaw-worker']);
  assert.match(files['miniclaw-worker'], /^[a-f0-9]{64}$/);
});

test('the bundled Node is pinned to an official archive by checksum', () => {
  assert.match(node.url, /^https:\/\/nodejs\.org\/dist\/v24\.\d+\.\d+\/node-v24\.\d+\.\d+-darwin-arm64\.tar\.gz$/);
  assert.match(node.digest, /^[a-f0-9]{64}$/);
  assert.equal(node.algorithm, 'sha256');
});

test('release audit refuses credentials and private files in copied trees', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-release-audit-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'safe.mjs'), 'export const port = 8787;\n');
  await auditReleaseTree(root);
  for (const name of ['.env', '.env.local', '.npmrc', '.netrc', 'credentials.json',
    'settings.json', 'private.pem', 'id_ed25519', 'data.sqlite3']) {
    const path = join(root, name);
    await writeFile(path, 'test-only-secret');
    await assert.rejects(auditReleaseTree(root), /release audit refused/i, name);
    await rm(path);
  }
  const outside = await mkdtemp(join(tmpdir(), 'mc-outside-release-'));
  t.after(() => rm(outside, { recursive: true, force: true }));
  await symlink(outside, join(root, 'outside'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await rm(join(root, 'outside'));
  await writeFile(join(root, '.DS_Store'), 'Finder metadata');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await auditReleaseTree(root, { ignoreMacMetadata: true });
});

test('release audit refuses VM media in copied trees', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-release-media-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const name of ['Restore.ipsw', 'MasterSetup.dmg', 'Disk.img', 'guest.iso',
    'disk.qcow2', 'disk.vmdk', 'disk.vdi', 'disk.vhd', 'disk.vhdx',
    'disk.sparseimage', 'machine.ova']) {
    const path = join(root, name);
    await writeFile(path, 'tiny test image');
    await assert.rejects(auditReleaseTree(root), /release audit refused/i, name);
    await rm(path);
  }
  await mkdir(join(root, 'disk.sparsebundle'));
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await rm(join(root, 'disk.sparsebundle'), { recursive: true });
  const archiveRoot = join(root, 'release');
  await mkdir(archiveRoot);
  await writeFile(join(archiveRoot, 'Restore.ipsw'), 'tiny test image');
  const archive = join(root, 'release.tar.gz');
  await exec(tar, ['-czf', archive, '-C', root, 'release']);
  await assert.rejects(auditReleaseArchive(archive), /release audit refused/i);
});

test('release audit refuses secret literals and populated env examples', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-release-audit-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'tool.mjs');
  await writeFile(path, 'const apiKey = "sk-proj-' + 'abcdefghijklmnopqrstuvwxyz123456";\n');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await writeFile(path, '-----BEGIN PRIVATE KEY-----\n');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await writeFile(path, 'SERVICE_API_KEY=opaque-testing-value\n');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
  await rm(path);
  const example = join(root, 'credentials.env.example');
  await writeFile(example, 'API_KEY=\n');
  await auditReleaseTree(root);
  await writeFile(example, 'API_KEY=test-only-secret\n');
  await assert.rejects(auditReleaseTree(root), /release audit refused/i);
});

test('release retains only the pinned Node executable and license', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-node-release-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, 'node-dist');
  await mkdir(join(source, 'bin'), { recursive: true });
  await mkdir(join(source, 'lib/node_modules/npm'), { recursive: true });
  await writeFile(join(source, 'bin/node'), 'node-binary');
  await writeFile(join(source, 'LICENSE'), 'node-license');
  await writeFile(join(source, 'lib/node_modules/npm/credentials.json'), 'test-only-secret');
  const archive = join(root, 'node.tar.gz');
  await exec(tar, ['-czf', archive, '-C', root, 'node-dist']);
  const output = join(root, 'release-node');
  await extractNodeRuntime(archive, output, join(root, 'work'), { platform: 'darwin', tar });
  assert.deepEqual((await readdir(output)).sort(), ['LICENSE', 'bin']);
  assert.deepEqual(await readdir(join(output, 'bin')), ['node']);
  await assert.rejects(access(join(output, 'lib')), { code: 'ENOENT' });
});

test('archive audit sees entries hidden by macOS tar listing', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-archive-audit-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, 'release');
  await mkdir(source);
  await writeFile(join(source, 'safe.txt'), 'safe');
  await writeFile(join(source, '._safe.txt'), 'metadata');
  const archive = join(root, 'release.tar.gz');
  await exec(tar, ['-czf', archive, '-C', root, 'release']);
  await assert.rejects(auditReleaseArchive(archive), /release audit refused/i);
});

// Windows releases pin a native runtime and expose a launcher that needs no system Node.
test('Windows release assets select native archives and bundled launchers', async () => {
  const { releasePlatform } = await import('../scripts/package-release.mjs');
  for (const arch of ['x64', 'arm64']) {
    const selected = releasePlatform('win32', arch);
    assert.equal(selected.vm, false);
    assert.ok(selected.node.file.endsWith(`win-${arch}.zip`));
    assert.match(selected.node.digest, /^[a-f0-9]{64}$/);
    assert.ok(selected.node.bytes > 1e6);
    assert.equal(selected.launcher, 'miniclaw.cmd');
    assert.ok(selected.launcherText.includes('"%~dp0node\\bin\\node.exe"'));
    assert.ok(selected.launcherText.includes('DisableDelayedExpansion'));
    assert.equal(selected.tar, 'tar.exe');
  }
  assert.throws(() => releasePlatform('linux', 'x64'), /unsupported/i);
  assert.throws(() => releasePlatform('win32', 'ia32'), /unsupported/i);
});

// Uses a real ZIP so Windows runtime extraction is checked independently of its manifest.
test('Windows Node ZIP extracts only executable and license', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-node-zip-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, 'node-dist');
  await mkdir(source);
  await writeFile(join(source, 'node.exe'), 'fixture executable');
  await writeFile(join(source, 'LICENSE'), 'fixture license');
  await writeFile(join(source, 'npm.cmd'), 'excluded npm');
  const archive = join(root, 'node.zip');
  await exec(tar, ['--format', 'zip', '-cf', archive, '-C', root, 'node-dist']);
  const output = join(root, 'node');
  await extractNodeRuntime(archive, output, join(root, 'work'), { platform: 'win32', tar });
  assert.deepEqual((await readdir(output)).sort(), ['LICENSE', 'bin']);
  assert.deepEqual(await readdir(join(output, 'bin')), ['node.exe']);
});

// Runs the portable launcher from a path cmd.exe commonly mishandles.
test('Windows release launcher preserves paths arguments and exit status',
  { skip: process.platform !== 'win32' }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'mc-launcher-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const folder = join(root, "Space Ω O'Brien & (test) ! %PATH%");
    await mkdir(join(folder, 'node/bin'), { recursive: true });
    await mkdir(join(folder, 'scripts'));
    await copyFile(process.execPath, join(folder, 'node/bin/node.exe'));
    await writeFile(join(folder, 'miniclaw.cmd'), releasePlatform('win32', 'x64').launcherText);
    await writeFile(join(folder, 'scripts/cli.mjs'),
      'console.log(JSON.stringify(process.argv.slice(2))); process.exitCode = 23;');
    // This final argument is cmd syntax, not a CRT argv value to escape again.
    const error = await exec('cmd.exe', ['/d', '/s', '/c', 'miniclaw.cmd "two words" Ω'],
      { cwd: folder, encoding: 'utf8', windowsVerbatimArguments: true }).then(() => null, error => error);
    assert.equal(error?.code, 23);
    assert.deepEqual(JSON.parse(error.stdout.trim()), ['two words', 'Ω']);
  });

// The bundled Node and the release manifest follow the backend bundle, not the packaging Node.
test('a release takes its architecture from the backend bundle', async () => {
  const { releaseArch } = await import('../scripts/package-release.mjs');
  assert.equal(releaseArch({ platform: 'win32', arch: 'arm64' }, 'win32', 'x64'), 'arm64');
  assert.equal(releaseArch({ platform: 'win32', arch: 'x64' }, 'win32', 'arm64'), 'x64');
  assert.equal(releasePlatform('win32', releaseArch({ platform: 'win32', arch: 'arm64' }, 'win32', 'x64')).node.file, 'node-v24.21.0-win-arm64.zip');
  assert.equal(releaseArch({ platform: 'darwin', arch: 'arm64' }, 'darwin', 'arm64'), 'arm64');
  assert.throws(() => releaseArch({ platform: 'darwin', arch: 'x64' }, 'darwin', 'arm64'), /architecture/i);
  assert.throws(() => releaseArch({ platform: 'darwin', arch: 'arm64' }, 'win32', 'arm64'), /platform/i);
  assert.throws(() => releaseArch({ platform: 'win32', arch: 'ia32' }, 'win32', 'x64'), /architecture/i);
});

// Windows releases carry MiniClaw's Git build; its complete corresponding source is published beside
// the release archive (GPLv2 3(a)) and the release names who honours the written offer (3(b)).
test('a Windows release publishes the verified Git corresponding source as a sibling archive', async t => {
  const { createHash } = await import('node:crypto');
  const { readFile } = await import('node:fs/promises');
  const { gitSourceArtifact, gitSourceOffer } = await import('../scripts/package-release.mjs');
  const { windowsGitBundle } = await import('../scripts/setup/windows-git-bundle.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-git-source-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const folder = join(root, 'git-corresponding-source');
  await mkdir(join(folder, 'dependencies'), { recursive: true });
  const files = { 'git-v2.56.0.windows.1.tar.gz': 'original', 'git-lpac-path.patch': 'patch', 'dependencies/zlib.tar.gz': 'zlib' };
  for (const [name, text] of Object.entries(files)) await writeFile(join(folder, name), text);
  const sha = text => createHash('sha256').update(text).digest('hex');
  const pin = { ...windowsGitBundle('arm64'), bytes: 42, digest: sha('runtime') };
  const manifest = { runtime: { arm64: { file: pin.file, bytes: pin.bytes, sha256: pin.digest, runtimePrefix: 'git' } },
    files: Object.entries(files).map(([file, text]) => ({ file, bytes: text.length, sha256: sha(text) })) };
  await writeFile(join(folder, 'SOURCE-MANIFEST.json'), JSON.stringify(manifest));
  const output = join(root, 'windows-portable');
  await assert.rejects(gitSourceArtifact({ arch: 'arm64', output, tar, pin, folder: null }), /MINICLAW_GIT_SOURCE/);
  // Source for another build, a changed file or an unlisted file is never published.
  await assert.rejects(gitSourceArtifact({ arch: 'arm64', output, tar, folder, pin: { ...pin, digest: sha('other') } }), /runtime/);
  await writeFile(join(folder, 'dependencies/zlib.tar.gz'), 'changed');
  await assert.rejects(gitSourceArtifact({ arch: 'arm64', output, tar, folder, pin }), /zlib/);
  await writeFile(join(folder, 'dependencies/zlib.tar.gz'), 'zlib');
  await writeFile(join(folder, 'extra.txt'), 'unlisted');
  await assert.rejects(gitSourceArtifact({ arch: 'arm64', output, tar, folder, pin }), /extra\.txt/);
  await rm(join(folder, 'extra.txt'));
  await assert.rejects(access(`${output}-git-corresponding-source.tar.gz`), { code: 'ENOENT' });
  const artifact = await gitSourceArtifact({ arch: 'arm64', output, tar, folder, pin });
  assert.equal(artifact.archive, `${output}-git-corresponding-source.tar.gz`);
  assert.equal(artifact.sha256, sha(await readFile(artifact.archive)));
  const { stdout } = await promisify(execFile)(tar, ['-tzf', artifact.archive]);
  for (const name of [...Object.keys(files), 'SOURCE-MANIFEST.json']) assert.match(stdout, new RegExp(`git-corresponding-source/${name.replace('.', '\\.')}`));
  await assert.rejects(gitSourceArtifact({ arch: 'arm64', output, tar, folder, pin }), /exists/);
  // The distributor named by the user ("Sep") is the default; MINICLAW_SOURCE_OFFER_CONTACT overrides it.
  const saved = process.env.MINICLAW_SOURCE_OFFER_CONTACT;
  t.after(() => { if (saved === undefined) delete process.env.MINICLAW_SOURCE_OFFER_CONTACT; else process.env.MINICLAW_SOURCE_OFFER_CONTACT = saved; });
  delete process.env.MINICLAW_SOURCE_OFFER_CONTACT;
  for (const contact of [undefined, '', '   ']) assert.match(gitSourceOffer({ contact, artifact, pin }), /^Distributor contact: Sep$/m);
  process.env.MINICLAW_SOURCE_OFFER_CONTACT = 'Other Distributor, other@example.invalid';
  assert.match(gitSourceOffer({ artifact, pin }), /^Distributor contact: Other Distributor, other@example\.invalid$/m);
  const offer = gitSourceOffer({ contact: 'Example Publisher, source@example.invalid', artifact, pin, date: new Date('2026-10-04T00:00:00Z') });
  assert.match(offer, /three years/);
  assert.match(offer, /source@example\.invalid/);
  assert.match(offer, /windows-portable-git-corresponding-source\.tar\.gz/);
  assert.match(offer, new RegExp(artifact.sha256));
  assert.match(offer, new RegExp(pin.digest));
  assert.match(offer, /2026-10-04/);
});

// Windows x64 ships marked as unverified in the sandbox (user decision 2026-10-04); ARM64 and Mac text are unchanged.
test('only the Windows x64 READ ME says it is not verified natively in the sandbox', async () => {
  const { releaseReadme } = await import('../scripts/package-release.mjs');
  const notice = 'Windows x64: builds and passes host checks; not verified natively in the sandbox.';
  const x64 = releaseReadme({ vm: false, version: '9.9.9', arch: 'x64' });
  assert.match(x64, /^MiniClaw 9\.9\.9 for Windows x64$/m);
  assert.ok(x64.includes(notice), x64);
  assert.match(x64, /Windows 11 ARM64/);
  const arm64 = releaseReadme({ vm: false, version: '9.9.9', arch: 'arm64' });
  assert.ok(!/not verified/.test(arm64), arm64);
  // The release folder's launcher starts, updates and removes the installation, so it must be kept.
  assert.match(arm64, /^Uninstall: \.\\miniclaw\.cmd uninstall \(keeps your data; add --delete-data to remove it\)$/m);
  assert.match(arm64, /^Keep this folder: its miniclaw\.cmd starts, updates and uninstalls MiniClaw\.$/m);
  assert.equal(arm64.replace('Windows arm64', 'Windows x64'), x64.replace(/\nWindows x64: builds and passes[\s\S]*?\n\n/, '\n'));
  const mac = releaseReadme({ vm: true, version: '9.9.9', arch: 'arm64' });
  assert.equal(mac, [
    'MiniClaw 9.9.9 for Apple silicon Macs', '',
    'Install:   open Terminal, drag the file "miniclaw" into it, type  setup  and press Return.',
    'Start:     ./miniclaw',
    'Update:    unpack a newer release, then run its  ./miniclaw update  and, for a VM,  ./miniclaw update-vm',
    '', 'This release is not signed with an Apple Developer ID. The first time, macOS may say it cannot',
    'check the app. Open System Settings, Privacy & Security, and choose "Open Anyway", or run:',
    '  xattr -dr com.apple.quarantine <this folder>',
    '', 'Nothing else needs to be installed. MiniClaw uses your existing Codex and Claude Code sign-ins.',
    'If Claude Code is missing, setup downloads and installs it from Anthropic (downloads.claude.ai) by itself.', ''].join('\n'));
});

// The Windows docs carry the same x64 status as the x64 READ ME.
test('the Windows docs mark x64 as built and host-checked but not verified natively in the sandbox', async () => {
  const { readFile } = await import('node:fs/promises');
  const status = 'builds and passes host checks; not verified natively in the sandbox';
  for (const doc of ['README.md', 'x64-readiness.md']) {
    const text = await readFile(new URL(`../docs/windows/${doc}`, import.meta.url), 'utf8');
    assert.ok(text.includes(`Windows x64: ${status}`), `${doc} states the x64 status`);
  }
});

// The private Node carries npm's tree from the official archive, pinned by digest. Its empty `.npmrc` and
// its documentation's example tokens are not credentials: a Windows release audits everything else as
// before and accepts that subtree only when it is exactly the pinned tree (VM2 release build, 2026-10-05).
test('a Windows release audit accepts the pinned npm tree and nothing that differs from it', async t => {
  const { auditWindowsRelease } = await import('../scripts/package-release.mjs');
  const { npmTreeDigest } = await import('../scripts/setup/windows-node-sandbox.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-npm-audit-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const npm = join(root, 'prebuilt/bundle/bin/node/node_modules/npm');
  await mkdir(join(npm, 'docs'), { recursive: true });
  const files = new Map([['.npmrc', Buffer.alloc(0)], ['docs/config.md', Buffer.from('NPM_TOKEN=abcdef0123456789\n')]]);
  for (const [path, bytes] of files) await writeFile(join(npm, path), bytes);
  const pin = { prefix: 'node_modules/npm/', files: files.size, digest: npmTreeDigest(files) };
  await assert.rejects(auditReleaseTree(root), /\.npmrc/);
  await auditWindowsRelease(root, { npm: pin });
  // Anything else is still audited, also beside the npm tree.
  await writeFile(join(root, 'prebuilt/bundle/bin/node/.npmrc'), '');
  await assert.rejects(auditWindowsRelease(root, { npm: pin }), /private or unsupported path/);
  await rm(join(root, 'prebuilt/bundle/bin/node/.npmrc'));
  // A changed or added file inside the npm tree is not exempt.
  await writeFile(join(npm, 'docs/config.md'), 'NPM_TOKEN=changed0123456789\n');
  await assert.rejects(auditWindowsRelease(root, { npm: pin }), /differs from its pin/);
  await writeFile(join(npm, 'docs/config.md'), 'NPM_TOKEN=abcdef0123456789\n');
  await writeFile(join(npm, 'extra.env'), 'X=1');
  await assert.rejects(auditWindowsRelease(root, { npm: pin }), /differs from its pin/);
});

// A clean clone builds without local Git files: with MINICLAW_GIT_SOURCE unset, the source comes from the pinned
// v0.1.5 release asset and is verified file by file exactly like a local folder.
test('a release build without MINICLAW_GIT_SOURCE publishes the source taken from the pinned release asset', async t => {
  const { createHash } = await import('node:crypto');
  const { readFile } = await import('node:fs/promises');
  const { gitSourceArtifact } = await import('../scripts/package-release.mjs');
  const { windowsGitBundle, gitSourceRelease, gitCorrespondingSource } = await import('../scripts/setup/windows-git-bundle.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-git-source-fetch-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const input = join(root, 'input/git-corresponding-source');
  await mkdir(input, { recursive: true });
  const sha = text => createHash('sha256').update(text).digest('hex');
  const pin = { ...windowsGitBundle('x64'), bytes: 42, digest: sha('runtime') };
  await writeFile(join(input, 'git-lpac-path.patch'), 'patch');
  await writeFile(join(input, 'SOURCE-MANIFEST.json'), JSON.stringify({ runtime: { x64: { file: pin.file, bytes: pin.bytes, sha256: pin.digest } },
    files: [{ file: 'git-lpac-path.patch', bytes: 5, sha256: sha('patch') }] }));
  await exec(tar, ['-czf', join(root, 'release-source.tar.gz'), '-C', join(root, 'input'), 'git-corresponding-source']);
  const bytes = await readFile(join(root, 'release-source.tar.gz'));
  const work = join(root, 'work');
  await mkdir(work);
  const folder = await gitCorrespondingSource({ folder: null, directory: work, tar, env: {},
    pin: { ...gitSourceRelease, bytes: bytes.length, digest: sha(bytes) }, fetcher: async () => new Response(bytes) });
  const output = join(root, 'miniclaw-0.1.6-windows-x64');
  const artifact = await gitSourceArtifact({ arch: 'x64', output, tar, folder, pin });
  const { stdout } = await exec(tar, ['-tzf', artifact.archive]);
  assert.match(stdout, /git-corresponding-source\/git-lpac-path\.patch/);
  assert.equal(artifact.sha256, sha(await readFile(artifact.archive)));
});
