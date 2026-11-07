// Checks `miniclaw uninstall` on Windows: one administrator approval removes the network service
// (after it removed its Winsock entries), then the program files go and the data folder stays.
import assert from 'node:assert/strict';
// Runs each case in a disposable installation folder.
import { test } from 'node:test';
// Builds and inspects the fixture installations.
import { access, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
// Locates temporary storage outside real installations.
import { tmpdir } from 'node:os';
// Resolves fixture paths.
import { join } from 'node:path';
// Runs the CLI without a terminal.
import { spawnSync } from 'node:child_process';
// The module under test.
import { uninstallWindows } from '../scripts/setup/uninstall-windows.mjs';
// The elevated removal request and its fixed script.
import { networkRemovalCommand } from '../scripts/setup/windows-network.mjs';
// Forgetting the remembered installation.
import { forgetInstallation, rememberInstallation, defaultInstallation } from '../scripts/setup/default-installation.mjs';

const id = 'c'.repeat(32), sid = 'S-1-5-21-111-222-333-1001';
const exists = path => access(path).then(() => true, () => false);

// Writes an installed layout: program files, settings, a protected data folder with user state.
async function installed(t, { network = true, pending = false } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'mc-uninstall-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'MiniClaw Install');
  await mkdir(join(destination, 'runtime', 'bin'), { recursive: true });
  await writeFile(join(destination, 'runtime', 'bin', 'miniclaw-server.exe'), 'MZ');
  await writeFile(join(destination, 'runtime', 'manifest.json'), '{"files":{}}');
  await mkdir(join(destination, 'data', 'windows-git-runtime', 'x', 'git'), { recursive: true });
  await writeFile(join(destination, 'data', 'windows-git.json'), '{}');
  await writeFile(join(destination, 'data', 'miniclaw.db'), 'conversations');
  await mkdir(join(destination, 'data', 'chat-workspace'));
  if (network) await writeFile(join(destination, 'data', 'windows-network.json'),
    JSON.stringify({ service_name: `MiniClawNetwork-${id}`, pipe_name: `MiniClawNetwork-${id}`, owner_sid: sid, secret_hex: 'a'.repeat(64) }));
  if (pending) {
    await writeFile(join(destination, 'data', 'windows-network-pending.json'), JSON.stringify({ id, ownerSid: sid, sha256: 'b'.repeat(64) }));
    await writeFile(join(destination, 'data', 'windows-setup-pending.json'), '{}');
  } else await writeFile(join(destination, 'settings.json'), JSON.stringify({ schemaVersion: 1, backend: 'host', hostReady: true }));
  return destination;
}

// Collects every injected step so a test can see what ran, in order.
function recorder({ answers = [true, true], running = false, fail } = {}) {
  const calls = [];
  return { calls, options: { platform: 'win32', onOutput: text => calls.push(['output', text]),
    confirm: async message => { calls.push(['confirm', message]); return answers.shift(); },
    running: async path => { calls.push(['running', path]); return running; },
    removeService: async request => { calls.push(['elevate', request]); if (fail) throw new Error(fail); },
    forget: async path => { calls.push(['forget', path]); },
    userSid: async () => { calls.push(['user']); return sid; } } };
}

test('uninstall removes the service with one approval, then the program files, and keeps the data', async t => {
  const destination = await installed(t);
  const { calls, options } = recorder();
  const result = await uninstallWindows(destination, options);
  const elevations = calls.filter(([kind]) => kind === 'elevate');
  assert.equal(elevations.length, 1, 'exactly one administrator approval');
  assert.deepEqual(elevations[0][1], { id, ownerSid: sid });
  // The running check comes before anything is asked or changed.
  assert.deepEqual(calls[0], ['running', join(destination, 'runtime', 'bin', 'miniclaw-server.exe')]);
  assert.ok(calls.findIndex(([kind]) => kind === 'confirm') < calls.findIndex(([kind]) => kind === 'elevate'));
  assert.equal(await exists(join(destination, 'runtime')), false);
  assert.equal(await exists(join(destination, 'settings.json')), false);
  for (const name of ['windows-network.json', 'windows-git.json', 'windows-git-runtime']) {
    assert.equal(await exists(join(destination, 'data', name)), false, name);
  }
  assert.equal(await readFile(join(destination, 'data', 'miniclaw.db'), 'utf8'), 'conversations');
  assert.ok(await exists(join(destination, 'data', 'chat-workspace')));
  assert.deepEqual(calls.find(([kind]) => kind === 'forget'), ['forget', destination]);
  assert.equal(result.service, `MiniClawNetwork-${id}`);
  assert.equal(result.dataKept, join(destination, 'data'));
  const said = calls.filter(([kind]) => kind === 'output').map(([, text]) => text).join('');
  assert.match(said, /administrator approval/);
  assert.match(said, /kept/);
});

test('--delete-data asks a second time and removes the whole installation folder', async t => {
  const destination = await installed(t);
  const { calls, options } = recorder({ answers: [true, true] });
  const result = await uninstallWindows(destination, { ...options, deleteData: true });
  assert.equal(calls.filter(([kind]) => kind === 'confirm').length, 2);
  assert.match(calls.filter(([kind]) => kind === 'confirm')[1][1], /cannot be undone/);
  assert.equal(await exists(destination), false);
  assert.equal(result.dataKept, null);
});

test('a declined question, a running backend or a refused approval change nothing', async t => {
  for (const [label, setup, pattern] of [
    ['declined', { answers: [false] }, /Nothing was changed/],
    ['data declined', { answers: [true, false] }, /Nothing was changed/],
    ['running', { running: true }, /is running.*Stop it.*Nothing was changed/s],
    ['refused', { fail: 'The operation was canceled by the user.' }, /canceled by the user.*Nothing else was removed/s],
  ]) {
    const destination = await installed(t);
    const before = (await readdir(destination)).sort();
    const { calls, options } = recorder(setup);
    await assert.rejects(uninstallWindows(destination, { ...options, deleteData: label === 'data declined' }), pattern, label);
    assert.deepEqual((await readdir(destination)).sort(), before, label);
    assert.ok(await exists(join(destination, 'runtime', 'bin', 'miniclaw-server.exe')), label);
    assert.ok(await exists(join(destination, 'data', 'windows-network.json')), label);
    if (label !== 'refused') assert.equal(calls.filter(([kind]) => kind === 'elevate').length, 0, label);
    assert.equal(calls.filter(([kind]) => kind === 'forget').length, 0, label);
  }
});

test('an interrupted setup removes the service it may have installed; no service means no elevation', async t => {
  const pending = await installed(t, { network: false, pending: true });
  const first = recorder();
  await uninstallWindows(pending, first.options);
  assert.deepEqual(first.calls.find(([kind]) => kind === 'elevate')[1], { id, ownerSid: sid });
  assert.equal(await exists(join(pending, 'data', 'windows-setup-pending.json')), false);
  assert.equal(await exists(join(pending, 'data', 'windows-network-pending.json')), false);
  const plain = await installed(t, { network: false });
  const second = recorder();
  await uninstallWindows(plain, second.options);
  assert.equal(second.calls.filter(([kind]) => kind === 'elevate').length, 0);
  assert.equal(await exists(join(plain, 'runtime')), false);
});

test('only a MiniClaw installation is touched, and a forged service identity is refused', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-uninstall-other-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'notes.txt'), 'mine');
  const { calls, options } = recorder();
  await assert.rejects(uninstallWindows(root, options), /No MiniClaw installation at .*Nothing was changed/s);
  await assert.rejects(uninstallWindows(join(root, 'missing'), options), /No MiniClaw installation/);
  assert.equal(await readFile(join(root, 'notes.txt'), 'utf8'), 'mine');
  const forged = await installed(t);
  await writeFile(join(forged, 'data', 'windows-network.json'), JSON.stringify({ service_name: 'Spooler', owner_sid: sid }));
  await assert.rejects(uninstallWindows(forged, options), /Invalid installed network service identity/);
  assert.ok(await exists(join(forged, 'runtime')));
  assert.equal(calls.filter(([kind]) => kind === 'elevate').length, 0);
  await assert.rejects(uninstallWindows(forged, { ...options, platform: 'darwin' }), /Windows/);
});

test('the elevated removal is one fixed script with literal arguments under the command-line limit', async () => {
  const scriptBody = await readFile(new URL('../scripts/setup/windows-network-remove.ps1', import.meta.url), 'utf8');
  const command = networkRemovalCommand({ id, ownerSid: sid, scriptBody });
  const outer = command.args.at(-1);
  assert.match(outer, /Start-Process .*-Verb RunAs -Wait -PassThru/);
  assert.ok(outer.length <= 30000, `${outer.length} characters`);
  const inner = Buffer.from(outer.match(/-EncodedCommand ([A-Za-z0-9+/=]+)/)[1], 'base64').toString('utf16le');
  assert.ok(inner.includes(`-Id '${id}' -OwnerSid '${sid}'`), inner.slice(-200));
  // The service removes its own Winsock entries before it is deleted, and the result is checked.
  assert.match(scriptBody, /Start\('winsock-acl-remove'\)/);
  assert.match(scriptBody, /\.removed/);
  assert.match(scriptBody, /WinSock\*/);
  assert.ok(scriptBody.indexOf("Start('winsock-acl-remove')") < scriptBody.indexOf('sc.exe" delete'));
  for (const change of [{ id: '../x' }, { ownerSid: 'S-1-5-18' }, { scriptBody: '' }]) {
    assert.throws(() => networkRemovalCommand({ id, ownerSid: sid, scriptBody, ...change }));
  }
});

test('the remembered installation is forgotten only when it is the removed one', async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-uninstall-home-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const env = { platform: process.platform, env: { LOCALAPPDATA: join(home, 'Local') } };
  const one = join(home, 'one'), two = join(home, 'two');
  await rememberInstallation(one, home, env);
  await forgetInstallation(two, home, env);
  assert.equal(await defaultInstallation(home, env), one);
  await forgetInstallation(one, home, env);
  assert.notEqual(await defaultInstallation(home, env), one);
});

test('miniclaw uninstall is listed and needs a terminal before it changes anything', async t => {
  const cli = new URL('../scripts/cli.mjs', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
  const help = spawnSync(process.execPath, [cli, '--help'], { encoding: 'utf8', timeout: 5000 });
  assert.match(help.stdout, /miniclaw uninstall \[PATH\]/);
  assert.match(help.stdout, /--delete-data/);
  const destination = await installed(t);
  const result = spawnSync(process.execPath, [cli, 'uninstall', destination], { encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 1);
  assert.match(result.stderr, process.platform === 'win32' ? /Interactive terminal required/ : /Windows/);
  assert.ok(await exists(join(destination, 'runtime')));
});
