// Checks that a folder left by `miniclaw uninstall` without --delete-data can be installed into again,
// keeping its conversations, and that no other existing folder is ever adopted.
import assert from 'node:assert/strict';
// Registers the kept-data cases.
import { test } from 'node:test';
// Builds disposable kept folders.
import { access, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Runs the shipped permission script natively on Windows.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { claimDestination, destinationError, keptData, keptMarker } from '../scripts/setup/destination.mjs';
import { uninstallWindows } from '../scripts/setup/uninstall-windows.mjs';
import { install } from '../scripts/setup/install.mjs';
import { failureNotice } from '../scripts/setup/view.mjs';

const windows = process.platform === 'win32';
const sid = 'S-1-5-21-111-222-333-1001', id = 'c'.repeat(32);
const exists = path => access(path).then(() => true, () => false);
// The permission script's answer for a private kept folder: the current user's SID.
const verified = (user = sid) => async () => ({ stdout: user, stderr: '' });

// A folder as uninstall leaves it: only `data`, with the user's state and uninstall's marker.
async function keptFolder(t, { marker = true, extra = false, destinationInMarker, ownerSid = sid } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'mc-kept-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'MiniClaw Install');
  await mkdir(join(destination, 'data', 'chat-workspace'), { recursive: true });
  await writeFile(join(destination, 'data', 'miniclaw.db'), 'conversations');
  await writeFile(join(destination, 'data', 'providers.json'), '{"codex":true,"claude":true}\n');
  if (marker) await writeFile(join(destination, 'data', keptMarker), JSON.stringify({ kind: 'miniclaw-kept-data', version: 1,
    destination: destinationInMarker ?? destination, ownerSid, time: '2026-10-05T20:00:00.000Z' }));
  if (extra) await writeFile(join(destination, 'notes.txt'), 'mine');
  return destination;
}

test('uninstall without --delete-data marks the folder it leaves, and --delete-data leaves nothing to mark', async t => {
  for (const deleteData of [false, true]) {
    const root = await mkdtemp(join(tmpdir(), 'mc-kept-uninstall-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const destination = join(root, 'MiniClaw Install');
    await mkdir(join(destination, 'runtime', 'bin'), { recursive: true });
    await writeFile(join(destination, 'runtime', 'bin', 'miniclaw-server.exe'), 'MZ');
    await writeFile(join(destination, 'runtime', 'manifest.json'), '{}');
    await writeFile(join(destination, 'settings.json'), '{}');
    await mkdir(join(destination, 'data'));
    await writeFile(join(destination, 'data', 'miniclaw.db'), 'conversations');
    await writeFile(join(destination, 'data', 'windows-network.json'), JSON.stringify({ service_name: `MiniClawNetwork-${id}`, owner_sid: sid }));
    await uninstallWindows(destination, { platform: 'win32', deleteData, onOutput: () => {}, confirm: async () => true,
      running: async () => false, removeService: async () => {}, forget: async () => {}, userSid: async () => sid });
    if (deleteData) { assert.equal(await exists(destination), false); continue; }
    assert.deepEqual(await readdir(destination), ['data']);
    const marker = JSON.parse(await readFile(join(destination, 'data', keptMarker), 'utf8'));
    assert.equal(marker.kind, 'miniclaw-kept-data');
    assert.equal(marker.destination, destination);
    assert.equal(marker.ownerSid, sid);
    assert.equal(await readFile(join(destination, 'data', 'miniclaw.db'), 'utf8'), 'conversations');
  }
});

test('a kept folder is recognised only with its marker, only data in it, this user and private permissions', async t => {
  const good = await keptFolder(t);
  // The permission script (destination.mjs runs it in its -Kept mode) is asked about this very folder.
  let asked;
  const found = await keptData(good, { platform: 'win32', verify: async folder => { asked = folder; return { stdout: sid }; } });
  assert.deepEqual(found, { destination: good, data: join(good, 'data') });
  assert.equal(asked, good);
  for (const [label, folder, run] of [
    ['no marker', await keptFolder(t, { marker: false }), verified()],
    ['something beside data', await keptFolder(t, { extra: true }), verified()],
    ['marker of another folder', await keptFolder(t, { destinationInMarker: 'C:\\Elsewhere\\MiniClaw' }), verified()],
    ['marker of another user', await keptFolder(t, { ownerSid: 'S-1-5-21-111-222-333-1002' }), verified()],
    ['permissions refused', await keptFolder(t), async () => { throw Object.assign(new Error('Command failed'), { stderr: 'Kept folder grants another account write access\r\n' }); }],
  ]) assert.equal(await keptData(folder, { platform: 'win32', verify: run }), null, label);
  // A marker that is not JSON, and a link in place of the data folder, are not recognised either.
  const garbage = await keptFolder(t);
  await writeFile(join(garbage, 'data', keptMarker), 'not json');
  assert.equal(await keptData(garbage, { platform: 'win32', verify: verified() }), null);
  assert.equal(await keptData(join(good, 'missing'), { platform: 'win32', verify: verified() }), null);
  // The Mac has no uninstall; nothing is ever adopted there.
  assert.equal(await keptData(good, { platform: 'darwin', verify: () => assert.fail('no script') }), null);
});

test('setup accepts a recognised kept folder and still refuses every other existing one', async t => {
  const folder = 'C:\\Users\\S\\AppData\\Local\\MiniClaw Install';
  const existing = async () => ({});
  let asked;
  assert.equal(await destinationError(folder, { platform: 'win32', stat: existing, kept: async path => { asked = path; return { destination: path }; } }), undefined);
  assert.equal(asked, folder);
  assert.match(await destinationError(folder, { platform: 'win32', stat: existing, kept: async () => null }), /already exists/);
  // The Mac never asks.
  const good = await keptFolder(t);
  assert.match(await destinationError(good, { platform: 'darwin', kept: () => assert.fail('asked') }), /already exists/);
});

// The plan and injected steps of a Windows installation that elevates nothing.
function windowsInstall(destination, overrides = {}) {
  const calls = [];
  return { calls, options: { plan: { platform: 'win32', arch: 'arm64', providers: { codex: true, claude: false }, port: 8787, images: [], packages: [], peakBytes: 0 },
    destination, cache: join(destination, '..', 'cache'), codexBin: process.execPath, approved: true, keep: { destination, data: join(destination, 'data') },
    checkPort: async () => {}, freeBytes: async () => 1e9,
    claim: () => assert.fail('a kept folder is not claimed as new'),
    adopt: async path => { calls.push(['adopt', path]); },
    buildHost: async runtime => { calls.push(['build']); await mkdir(join(runtime, 'bin'), { recursive: true }); await writeFile(join(runtime, 'bin', 'miniclaw-server.exe'), 'MZ'); },
    secureData: async (runtime, data) => { calls.push(['secure', data]); },
    installNetwork: async () => { calls.push(['network']); return { serviceName: `MiniClawNetwork-${id}` }; },
    ...overrides } };
}

// The stand-in backend cannot write Windows private files; on Windows a real reinstall into a kept folder is checked
// with the installed release instead (work/windows-vm/polish-kept-data-*-vm1-*.log).
test('installing into a kept folder keeps the conversations, records the new choices and drops the marker',
  { skip: windows && 'stand-in backend: covered natively by the release reinstall check' }, async t => {
  const destination = await keptFolder(t);
  const { calls, options } = windowsInstall(destination);
  await install(options);
  assert.deepEqual(calls.map(([kind]) => kind), ['adopt', 'build', 'secure', 'network']);
  assert.equal(calls[0][1], destination);
  assert.equal(await readFile(join(destination, 'data', 'miniclaw.db'), 'utf8'), 'conversations');
  assert.ok(await exists(join(destination, 'data', 'chat-workspace')));
  assert.deepEqual(JSON.parse(await readFile(join(destination, 'data', 'providers.json'), 'utf8')), { codex: true, claude: false });
  assert.equal(await exists(join(destination, 'data', keptMarker)), false);
  assert.deepEqual((await readdir(join(destination, 'data'))).filter(name => /kept|before/.test(name)), []);
  assert.equal(JSON.parse(await readFile(join(destination, 'settings.json'), 'utf8')).status, 'installed');
});

test('a failed install into a kept folder leaves it exactly as uninstall left it', async t => {
  for (const failAt of ['build', 'secure']) {
    const destination = await keptFolder(t);
    const before = await readFile(join(destination, 'data', keptMarker), 'utf8');
    const { options } = windowsInstall(destination, failAt === 'build'
      ? { buildHost: async runtime => { await mkdir(runtime); throw new Error('build broke'); } }
      : { secureData: async () => { throw new Error('data folder is not private'); } });
    const error = await install(options).then(() => assert.fail('must fail'), error => error);
    assert.deepEqual(await readdir(destination), ['data'], failAt);
    assert.equal(await readFile(join(destination, 'data', keptMarker), 'utf8'), before, failAt);
    assert.equal(await readFile(join(destination, 'data', 'miniclaw.db'), 'utf8'), 'conversations', failAt);
    assert.equal(await readFile(join(destination, 'data', 'providers.json'), 'utf8'), '{"codex":true,"claude":true}\n', failAt);
    assert.equal(error.keptData, destination, failAt);
    assert.match(failureNotice(error), /data .*kept|kept.*data/i);
    assert.doesNotMatch(failureNotice(error), /folder was removed/);
  }
  // A refused re-check of the folder changes nothing in it.
  const destination = await keptFolder(t);
  const { options } = windowsInstall(destination, { adopt: async () => { throw new Error('Kept folder grants another account write access'); },
    buildHost: () => assert.fail('built into an unverified folder') });
  await assert.rejects(install(options), /another account/);
  assert.equal(await readFile(join(destination, 'data', 'miniclaw.db'), 'utf8'), 'conversations');
  assert.ok(await exists(join(destination, 'data', keptMarker)));
});

test('the permission script checks a kept folder without changing it', async () => {
  const text = await readFile(new URL('../scripts/setup/windows-install-directory.ps1', import.meta.url), 'utf8');
  assert.match(text, /\[switch\]\$Kept/);
  const kept = text.slice(text.indexOf('if ($Kept)'));
  assert.match(kept, /AreAccessRulesProtected/);
  assert.match(kept, /GetOwner/);
  assert.match(kept, /Assert-Private/);
  assert.match(kept, /kept-after-uninstall\.json/);
  assert.doesNotMatch(text, /Set-Acl|icacls|takeown/i);
});

const powershell = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');

test('native: a folder left by uninstall is recognised; one with other content or open permissions is not', { skip: !windows }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc kept Ω-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const { stdout } = await promisify(execFile)(powershell, ['-NoProfile', '-NonInteractive', '-Command',
    '[Console]::Out.Write([Security.Principal.WindowsIdentity]::GetCurrent().User.Value)'], { windowsHide: true });
  const make = async name => {
    const destination = join(root, name);
    await claimDestination(destination);
    await mkdir(join(destination, 'data'));
    await writeFile(join(destination, 'data', 'miniclaw.db'), 'conversations');
    await writeFile(join(destination, 'data', keptMarker), JSON.stringify({ kind: 'miniclaw-kept-data', version: 1, destination, ownerSid: stdout }));
    return destination;
  };
  const good = await make('MiniClaw Install');
  assert.deepEqual(await keptData(good), { destination: good, data: join(good, 'data') });
  assert.equal(await destinationError(good), undefined);
  const extra = await make('with extra');
  await writeFile(join(extra, 'settings.json'), '{}');
  assert.equal(await keptData(extra), null);
  assert.match(await destinationError(extra), /already exists/);
  const open = await make('open');
  await promisify(execFile)(join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'icacls.exe'), [join(open, 'data'), '/grant', '*S-1-5-11:(OI)(CI)(M)'], { windowsHide: true });
  assert.equal(await keptData(open), null);
  // A plain folder the user made is never adopted.
  const plain = join(root, 'plain');
  await mkdir(join(plain, 'data'), { recursive: true });
  assert.equal(await keptData(plain), null);
});
