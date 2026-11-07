// Checks the opt-in VM session that serves a backend running on the Mac.
import assert from 'node:assert/strict';
// Runs each case against isolated fixtures.
import { test } from 'node:test';
// Builds private installed layouts and reads link files.
import { access, chmod, copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
// Keeps socket paths short enough for Unix sockets.
import { join } from 'node:path';
// Runs the installed launcher as a child.
import { spawnSync } from 'node:child_process';
// Starts the session under test with fake native supervisors.
import { startPair } from '../scripts/vm/session.mjs';
import { eventually } from './support/harness.mjs';

// Creates a pair whose native binary only records that it was started.
async function fixture(t) {
  const root = await mkdtemp('/private/tmp/mc-mac-');
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  await mkdir(join(root, 'Master'));
  const log = join(root, 'native-started');
  const native = join(root, 'native');
  await writeFile(native, `#!${process.execPath}
import { appendFileSync } from 'node:fs';
appendFileSync(${JSON.stringify(log)}, JSON.stringify(process.argv.slice(2)) + '\\n');
appendFileSync(${JSON.stringify(log)} + '.pid', String(process.pid));
process.stdout.write('{"status":"vm_running"}\\n');
process.on('SIGTERM', () => { process.stdout.write('{"status":"stopped"}\\n'); process.exit(0); });
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  const worker = join(root, 'worker.json');
  await writeFile(worker, JSON.stringify({ key: 'ab'.repeat(32), cwd: '/Users/Shared/MiniClaw/Workspace' }), { mode: 0o600 });
  return { root, log, pair: { root, port: 19878, mac: { worker },
    master: { binary: native, guest: join(root, 'Master'), media: 'unused' }, provisioning: { resources: {} } } };
}
const started = async log => { try { return (await readFile(log, 'utf8')).trim().split('\n'); } catch { return []; } };

test('a Mac-backend session starts no VM and no Linux controller', async t => {
  const { log, pair } = await fixture(t);
  const session = await startPair(pair, { admit: () => ({ settle() {} }) });
  try {
    assert.deepEqual(await started(log), [], 'nothing boots until a VM task asks');
    const info = await stat(session.linksPath);
    assert.equal(info.mode & 0o077, 0, 'the link file is private');
    const links = JSON.parse(await readFile(session.linksPath, 'utf8'));
    assert.deepEqual(Object.keys(links).sort(), ['controlKey', 'controlSocket', 'dedicated', 'master', 'worker']);
    assert.equal(links.worker, pair.mac.worker);
    for (const path of [links.controlSocket, links.controlKey, links.master,
      join(links.dedicated, 'd00', 'controller.sock'), join(links.dedicated, 'd63', 'controller.sock')]) {
      await access(path);
    }
  } finally { assert.equal(await session.close(), 0); }
  await assert.rejects(access(session.linksPath), 'the link directory is removed on close');
});

test('Master boots only when started through the session', async t => {
  const { log, pair } = await fixture(t);
  const session = await startPair(pair, { admit: () => ({ settle() {} }) });
  try {
    await session.startMaster();
    const lines = await eventually(() => started(log), value => value.length === 1, 'Master start');
    assert.equal(JSON.parse(lines[0])[0], 'run');
  } finally { await session.close(); }
});

test('first-time setup opens the VM in the foreground; normal use keeps it hidden and view-only', async t => {
  for (const [options, background] of [[{ setup: true }, false], [{}, true]]) {
    const { log, pair } = await fixture(t);
    const session = await startPair(pair, { admit: () => ({ settle() {} }), ...options });
    try {
      await session.startMaster();
      const lines = await eventually(() => started(log), value => value.length === 1, 'Master start');
      assert.equal(JSON.parse(lines[0]).includes('--background'), background,
        background ? 'a working VM stays hidden behind its input shield'
          : 'setup needs a window the user can click in');
    } finally { await session.close(); }
  }
});

test('the host launcher passes the link file to the backend and closes the session', async t => {
  const root = await mkdtemp('/private/tmp/mc-mac-launch-');
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'runtime/bin'), { recursive: true });
  await copyFile(new URL('../scripts/start-host.mjs', import.meta.url), join(root, 'runtime/start-host.mjs'));
  await mkdir(join(root, 'runtime/vm'));
  const closed = join(root, 'session-closed');
  await writeFile(join(root, 'runtime/vm/session.mjs'), `
import { writeFileSync } from 'node:fs';
export const reportShutdown = () => {};
export async function startPair(pair, { limits } = {}) {
  if (!pair.mac || limits?.cpuPercent !== 70) throw new Error('Unexpected session input');
  return { linksPath: '/private/tmp/fixture-links.json', done: new Promise(() => {}),
    close: async () => { writeFileSync(${JSON.stringify(closed)}, 'closed'); return 0; } };
}
`);
  await writeFile(join(root, 'runtime/bin/miniclaw-server'), `#!${process.execPath}
console.log(JSON.stringify({ links: process.env.MINICLAW_VM_LINKS ?? null, codex: process.env.CODEX_BIN }));
`, { mode: 0o700 });
  const settings = { schemaVersion: 1, backend: 'host', hostReady: true, port: 19878, codexBin: '/fixture/codex' };
  await writeFile(join(root, 'settings.json'), JSON.stringify(settings));
  const plain = spawnSync(process.execPath, [join(root, 'runtime/start-host.mjs')], { encoding: 'utf8', timeout: 5000 });
  assert.equal(plain.status, 0, plain.stderr);
  assert.deepEqual(JSON.parse(plain.stdout), { links: null, codex: '/fixture/codex' });
  await writeFile(join(root, 'settings.json'), JSON.stringify({ ...settings, pair: { mac: { worker: '/fixture/worker.json' } },
    limits: { cpuPercent: 70, memoryPercent: 60, diskPercent: 30 } }));
  const linked = spawnSync(process.execPath, [join(root, 'runtime/start-host.mjs')], { encoding: 'utf8', timeout: 5000 });
  assert.equal(linked.status, 0, linked.stderr);
  assert.equal(JSON.parse(linked.stdout).links, '/private/tmp/fixture-links.json');
  assert.equal(await readFile(closed, 'utf8'), 'closed');
});

test('a hidden VM lets the user log in, then shields itself once its worker connects', async () => {
  const windowSource = await readFile(new URL('../scripts/experiments/vm-window.swift', import.meta.url), 'utf8');
  const bridge = await readFile(new URL('../scripts/vm/guest-bridge.swift', import.meta.url), 'utf8');
  const lab = await readFile(new URL('../scripts/experiments/vm-lab.swift', import.meta.url), 'utf8');
  // The bridge reports the worker route's first connection, which only happens after login.
  assert.match(bridge, /var onPaired: \(\(\) -> Void\)\?/);
  assert.match(bridge, /route\.port == 10242[\s\S]{0,120}onPaired\?\(\)/);
  // Until then a background window is unshielded and shows itself after a short wait.
  assert.match(windowSource, /func workerConnected\(\)/);
  assert.match(windowSource, /Log in to this VM/);
  assert.match(windowSource, /inputShield\.isHidden = !shielded/);
  // A background run with the network helper keeps its network interface.
  assert.match(lab, /network: args\.count >= 8 \? args\[7\] : nil/);
});

test('shutdown confirms the macOS dialog and waits longer than its own countdown', async () => {
  const headless = await readFile(new URL('../scripts/experiments/vm-headless.swift', import.meta.url), 'utf8');
  const windowSource = await readFile(new URL('../scripts/experiments/vm-window.swift', import.meta.url), 'utf8');
  // requestStop acts as a power button: macOS shows a dialog that waits 60 seconds.
  assert.match(headless, /shutdownGrace: Duration = \.seconds\(90\)/);
  assert.match(windowSource, /func confirmShutdownDialog\(\)/);
  assert.match(windowSource, /keyCode: 36/, 'Return presses the dialog\'s default Shut Down button');
  assert.match(windowSource, /try machine\.requestStop\(\)[\s\S]{0,300}confirmShutdownDialog/);
});

test('the user can take control of a shielded VM from the window, and hiding is announced', async () => {
  const windowSource = await readFile(new URL('../scripts/experiments/vm-window.swift', import.meta.url), 'utf8');
  assert.match(windowSource, /Control this VM/);
  assert.match(windowSource, /@objc private func toggleControl/);
  assert.match(windowSource, /inputShield\.isHidden = !shielded/);
  assert.match(windowSource, /Logged in\. This window hides in a moment/);
});

test('during setup the session reports a VM that died; in normal use a stopped VM is not an error', async t => {
  for (const [options, reported] of [[{ setup: true }, true], [{}, false]]) {
    const { log, pair } = await fixture(t);
    const session = await startPair(pair, { admit: () => ({ settle() {} }), ...options });
    try {
      await session.startMaster();
      const pid = await eventually(() => readFile(`${log}.pid`, 'utf8').catch(() => ''), Boolean, 'VM pid');
      process.kill(Number(pid), 'SIGKILL');
      const outcome = await Promise.race([session.done.then(() => 'reported'),
        new Promise(resolve => setTimeout(() => resolve('silent'), 1500))]);
      assert.equal(outcome, reported ? 'reported' : 'silent');
    } finally { await session.close().catch(() => {}); }
  }
});
