// Checks that an installation folder is claimed privately before a backend from it can become
// a LocalSystem service, and that unsafe destinations are refused before approval.
import assert from 'node:assert/strict';
// Registers the destination contract tests.
import { test } from 'node:test';
// Creates disposable installations and runtime fixtures.
import { access, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Pins fixture content the way the release manifest does.
import { createHash } from 'node:crypto';
// Runs the shipped script natively on Windows.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { claimDestination, destinationError, verifyInstallation } from '../scripts/setup/destination.mjs';
import { pinnedBackend } from '../scripts/setup/windows-network.mjs';
import { install } from '../scripts/setup/install.mjs';

const windows = process.platform === 'win32';
const exe = windows ? 'miniclaw-server.exe' : 'miniclaw-server.exe';

test('Windows destinations must be plain local drive paths', async () => {
  const check = value => destinationError(value, { platform: 'win32' });
  for (const value of ['\\\\server\\share\\MiniClaw', '//server/share/MiniClaw', '\\\\?\\C:\\MiniClaw',
    '\\\\.\\C:\\MiniClaw', '\\\\?\\UNC\\server\\share\\MiniClaw', 'C:MiniClaw', 'MiniClaw', '\\MiniClaw',
    'C:\\MiniClaw:stream', 'C:\\Apps\\..\\MiniClaw']) {
    assert.match(await check(value), /local drive|absolute/i, value);
  }
  assert.match(await check('\\\\server\\share\\MiniClaw'), /network/i);
  assert.equal(await check('Q:\\No Such Folder Ω\\MiniClaw Install'), undefined);
  assert.equal(await check('Q:/No Such Folder/MiniClaw'), undefined);
});

test('existing and relative destinations are still refused on every platform', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-destination-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  assert.match(await destinationError(root), /already exists/);
  assert.match(await destinationError('relative/folder'), /absolute/i);
  assert.match(await destinationError(''), /absolute/i);
  assert.equal(await destinationError(join(root, 'new')), undefined);
});

test('a Mac destination is created owner-only, as before', { skip: windows }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-destination-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await claimDestination(join(root, 'install'), { run: () => assert.fail('no Windows script on macOS') });
  assert.equal((await stat(join(root, 'install'))).mode & 0o777, 0o700);
  await assert.rejects(claimDestination(join(root, 'install')), { code: 'EEXIST' });
  await verifyInstallation(join(root, 'install'), { run: () => assert.fail('no Windows script on macOS') });
});

test('Windows claims and verifies through the fixed protected-directory script', async () => {
  const calls = [];
  const run = async (file, args) => { calls.push([file, args]); };
  await claimDestination('C:\\Apps\\MiniClaw', { platform: 'win32', run, systemRoot: 'C:\\Windows' });
  await verifyInstallation('C:\\Apps\\MiniClaw', { platform: 'win32', run, systemRoot: 'C:\\Windows' });
  for (const [file, args] of calls) {
    assert.equal(file, 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe');
    assert.ok(args.includes('-NonInteractive') && args.includes('-NoProfile'));
    assert.match(args[args.indexOf('-File') + 1], /windows-install-directory\.ps1$/);
    assert.equal(args[args.indexOf('-Path') + 1], 'C:\\Apps\\MiniClaw');
  }
  assert.ok(calls[0][1].includes('-Create'));
  assert.equal(calls[1][1].includes('-Create'), false);
  // A share or device path never reaches PowerShell.
  await assert.rejects(claimDestination('\\\\server\\share\\MiniClaw', { platform: 'win32', run }), /local drive/i);
  await assert.rejects(verifyInstallation('\\\\?\\C:\\MiniClaw', { platform: 'win32', run }), /local drive/i);
  assert.equal(calls.length, 2);
  // The script's own diagnosis reaches the user.
  await assert.rejects(claimDestination('C:\\MiniClaw', { platform: 'win32', systemRoot: 'C:\\Windows',
    run: async () => { throw Object.assign(new Error('Command failed'), { stderr: 'Installation folder ancestor permits replacement by another account\r\n' }); } }),
  /ancestor permits replacement by another account/);
});

// Shared plan and options for installations that never elevate.
async function options(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-claim-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return { plan: { platform: 'win32', providers: { codex: true }, port: 8787, images: [], packages: [], peakBytes: 0 },
    destination: join(root, 'install'), cache: join(root, 'cache'), codexBin: process.execPath, approved: true,
    checkPort: async () => {}, freeBytes: async () => 1e9 };
}

test('installation claims its folder before anything is built into it', async t => {
  const input = await options(t);
  const order = [];
  await assert.rejects(install({ ...input,
    claim: async path => { order.push(['claim', path]); await mkdir(path); },
    buildHost: async () => { order.push(['build']); throw new Error('stop after claim'); } }), /stop after claim/);
  assert.deepEqual(order, [['claim', input.destination], ['build']]);
  // A refused claim builds nothing and leaves nothing behind.
  await assert.rejects(install({ ...input,
    claim: async () => { throw new Error('Installation folder ancestor permits replacement by another account'); },
    buildHost: () => assert.fail('built into an unprotected folder') }), /another account/);
  await assert.rejects(access(input.destination), { code: 'ENOENT' });
});

test('a refused claim never removes a folder this installation did not create', async t => {
  const input = await options(t);
  await assert.rejects(install({ ...input,
    claim: async path => { await mkdir(path); await writeFile(join(path, 'theirs.txt'), 'planted');
      throw Object.assign(new Error('Service destination already exists'), { code: 'EEXIST' }); },
    buildHost: () => assert.fail('built') }), /already exists/);
  assert.equal(await readFile(join(input.destination, 'theirs.txt'), 'utf8'), 'planted');
});

// Builds a runtime whose manifest pins the backend bytes.
async function runtimeFixture(t, { recorded } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'mc-pinned-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const runtime = join(root, 'install', 'runtime');
  await mkdir(join(runtime, 'bin'), { recursive: true });
  await writeFile(join(runtime, 'bin', exe), 'reviewed backend');
  const sha256 = createHash('sha256').update('reviewed backend').digest('hex');
  await writeFile(join(runtime, 'manifest.json'), JSON.stringify({ files: { 'bin/miniclaw-server.exe': recorded ?? sha256 } }));
  return { runtime, sha256, destination: join(root, 'install') };
}

test('the service is pinned to the manifest hash, after the folder permissions are verified', async t => {
  const { runtime, sha256, destination } = await runtimeFixture(t);
  const verified = [];
  const pinned = await pinnedBackend(runtime, { verify: async path => { verified.push(path); } });
  assert.deepEqual(verified, [destination]);
  assert.equal(pinned.sha256, sha256);
  assert.equal(pinned.source, join(runtime, 'bin', exe));
  // Unsafe permissions stop before the backend is read or hashed.
  await rm(join(runtime, 'bin', exe));
  await assert.rejects(pinnedBackend(runtime, { verify: async () => { throw new Error('grants another account access'); } }),
    /another account/);
});

test('a backend that differs from its manifest is never offered for elevation', async t => {
  const { runtime } = await runtimeFixture(t);
  await writeFile(join(runtime, 'bin', exe), 'planted backend');
  await assert.rejects(pinnedBackend(runtime, { verify: async () => {} }), /does not match its release manifest/);
  const missing = await runtimeFixture(t, { recorded: 'not-a-hash' });
  await assert.rejects(pinnedBackend(missing.runtime, { verify: async () => {} }), /manifest/);
});

// The elevated and unelevated scripts are parsed natively; behaviour runs only on Windows.
const script = new URL('../scripts/setup/windows-install-directory.ps1', import.meta.url);

test('the protected-directory script creates atomically and checks owners and ancestors', async () => {
  const text = await readFile(script, 'utf8');
  assert.match(text, /CreateDirectoryW/);
  assert.match(text, /SetAccessRuleProtection\(\$true, \$false\)/);
  for (const sid of ['S-1-5-18', 'S-1-5-32-544']) assert.ok(text.includes(sid), sid);
  assert.match(text, /0x500D0150/, 'ancestors use the reviewed replacement mask of the cache check');
  assert.match(text, /ReparsePoint|0x400/);
  assert.doesNotMatch(text, /Set-Acl|icacls|takeown/i, 'existing objects are never re-permissioned');
});

const powershell = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
const ps = command => promisify(execFile)(powershell, ['-NoProfile', '-NonInteractive', '-Command', command], { windowsHide: true });
const quoted = value => `'${value.replaceAll("'", "''")}'`;

test('native: a claimed folder is protected and owned by the installing user', { skip: !windows }, async t => {
  const root = await mkdtemp(join(tmpdir(), "mc claim Ω O'Brien-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'MiniClaw Install');
  await claimDestination(destination);
  const { stdout } = await ps(`$a = Get-Acl -LiteralPath ${quoted(destination)}; $u = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value; `
    + `$r = @($a.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]) | ForEach-Object { $_.IdentityReference.Value + '=' + [int]$_.FileSystemRights + '/' + $_.IsInherited }); `
    + `[Console]::Out.Write((@{ protected = $a.AreAccessRulesProtected; owner = $a.GetOwner([Security.Principal.SecurityIdentifier]).Value; user = $u; rules = $r } | ConvertTo-Json -Compress))`);
  const acl = JSON.parse(stdout);
  assert.equal(acl.protected, true);
  assert.equal(acl.owner, acl.user);
  assert.deepEqual([...acl.rules].sort(), [`${acl.user}=2032127/False`, 'S-1-5-18=2032127/False', 'S-1-5-32-544=2032127/False'].sort());
  await assert.rejects(claimDestination(destination), /already exists/i);
  // Files and folders created inside inherit only those three entries.
  await mkdir(join(destination, 'runtime', 'bin'), { recursive: true });
  await writeFile(join(destination, 'runtime', 'bin', 'miniclaw-server.exe'), 'fixture');
  await verifyInstallation(destination);
});

test('native: destinations another account could replace or write are refused', { skip: !windows }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-claim-open-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const icacls = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'icacls.exe');
  // A test-owned parent that Authenticated Users may modify: its child could be swapped.
  const open = join(root, 'open parent');
  await mkdir(open);
  await promisify(execFile)(icacls, [open, '/grant', '*S-1-5-11:(M)'], { windowsHide: true });
  await assert.rejects(claimDestination(join(open, 'MiniClaw')), /another account/i);
  await assert.rejects(access(join(open, 'MiniClaw')), { code: 'ENOENT' });
  // An installation folder that inherits write access for other accounts, like C:\\MiniClaw would.
  const shared = join(root, 'shared install');
  await mkdir(join(shared, 'runtime', 'bin'), { recursive: true });
  await writeFile(join(shared, 'runtime', 'bin', 'miniclaw-server.exe'), 'fixture');
  await verifyInstallation(shared);
  await promisify(execFile)(icacls, [shared, '/grant', '*S-1-5-11:(OI)(CI)(M)'], { windowsHide: true });
  await assert.rejects(verifyInstallation(shared), /another account/i);
  // A drive-root style parent, where others may only add folders, is acceptable.
  const root2 = join(root, 'drive root');
  await mkdir(root2);
  await promisify(execFile)(icacls, [root2, '/grant', '*S-1-5-11:(AD)', '/grant', '*S-1-5-11:(OI)(CI)(IO)(M)'], { windowsHide: true });
  await claimDestination(join(root2, 'MiniClaw'));
  await mkdir(join(root2, 'MiniClaw', 'runtime', 'bin'), { recursive: true });
  await verifyInstallation(join(root2, 'MiniClaw'));
  // An installation without its runtime is not verifiable and is refused.
  await claimDestination(join(root2, 'Empty'));
  await assert.rejects(verifyInstallation(join(root2, 'Empty')), /cannot find/i);
});
