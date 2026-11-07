// Checks how setup explains what it can and cannot resume.
import assert from 'node:assert/strict';
// Runs each receipt case in isolation.
import { test } from 'node:test';
// Creates disposable installation folders.
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
// Opens a receipt to another account on Windows, where mode bits are not permissions.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
// Locates temporary storage outside real installations.
import { tmpdir } from 'node:os';
// Resolves fixture paths.
import { join } from 'node:path';
// Loads the receipt reader under test.
import { readReceipt } from '../scripts/setup/receipt.mjs';
// Loads the Windows check that explains a resume before any terminal is needed.
import { pendingWindowsSetup } from '../scripts/setup/resume-windows.mjs';

// Writes one receipt with the given mode and returns its installation folder.
async function installation(t, settings, mode = 0o600) {
  const root = await mkdtemp(join(tmpdir(), 'mc-receipt-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  if (settings) await writeFile(join(root, 'settings.json'),
    JSON.stringify(typeof settings === 'function' ? settings(root) : settings), { mode });
  // Windows ignores the mode, so a readable receipt gets a real rule for Authenticated Users.
  if (settings && process.platform === 'win32' && mode & 0o077) {
    await promisify(execFile)(join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'icacls.exe'),
      [join(root, 'settings.json'), '/grant', '*S-1-5-11:(R)'], { windowsHide: true });
  }
  return root;
}
const waiting = root => ({ schemaVersion: 1, backend: 'host', port: 8787, status: 'awaiting-guest-setup',
  pair: { root, port: 8787, mac: { worker: join(root, 'worker.json') } } });

test('a missing installation says why and what to do', async t => {
  const root = await installation(t, null);
  await assert.rejects(readReceipt(join(root, 'never-installed')), error => {
    assert.match(error.message, /No installation at/);
    assert.match(error.message, /interrupted before it finished is removed/);
    assert.match(error.message, /Run it again with: miniclaw setup "/);
    assert.doesNotMatch(error.message, /ENOENT|lstat/);
    return true;
  });
});

test('a finished or Mac-only installation has nothing to resume', async t => {
  const macOnly = await installation(t, { schemaVersion: 1, backend: 'host', port: 8787, status: 'installed', pair: null });
  await assert.rejects(readReceipt(macOnly), /has no VM.*Nothing to resume.*miniclaw start/s);
  const done = await installation(t, root => ({ ...waiting(root), status: 'installed', vmReady: true }));
  await assert.rejects(readReceipt(done), /already finished.*miniclaw start/s);
});

test('a waiting VM installation is returned, and an unsafe receipt is refused', async t => {
  const root = await installation(t, waiting);
  assert.equal((await readReceipt(root)).pair.mac.worker, join(root, 'worker.json'));
  const open = await installation(t, waiting, 0o644);
  await assert.rejects(readReceipt(open), /private/);
  const moved = await installation(t, root => ({ ...waiting(root), pair: { ...waiting(root).pair, root: '/elsewhere' } }));
  await assert.rejects(readReceipt(moved), /does not belong to this folder/);
});

// On Windows `miniclaw resume` recovers a pending network-service setup. Before it asks for a
// terminal or elevation it says why there is nothing to resume, in the same words as the Mac.
test('a Windows resume explains a missing or finished installation first', async t => {
  const missing = await installation(t, null);
  await assert.rejects(pendingWindowsSetup(join(missing, 'never-installed')),
    /No installation at .*Run it again with: miniclaw setup "/s);
  const finished = await installation(t, { schemaVersion: 1, backend: 'host', port: 8787, status: 'installed',
    hostReady: true, pair: null });
  // Changed 2026-10-05 (lead): a finished Windows installation is a no-op for resume, worded for Windows.
  await assert.rejects(pendingWindowsSetup(finished), /already installed at .*Nothing to resume/s);
  const open = await installation(t, { schemaVersion: 1, backend: 'host', port: 8787, status: 'installed',
    hostReady: true, pair: null }, 0o644);
  await assert.rejects(pendingWindowsSetup(open), /private/);
  const pending = await installation(t, null);
  await mkdir(join(pending, 'data'));
  await writeFile(join(pending, 'data', 'windows-setup-pending.json'), '{}\n', { mode: 0o600 });
  await pendingWindowsSetup(pending);
});

// `miniclaw resume` on a finished Windows installation is a no-op: it says so and how to start, and the
// wizard exits 0 for it (VM2 release run, 2026-10-05: it exited 1 with "This installation has no VM").
test('a finished Windows installation has nothing to resume, said as a no-op', async t => {
  const root = await installation(t, { schemaVersion: 1, backend: 'host', port: 8787, status: 'installed', hostReady: true, pair: null });
  await assert.rejects(pendingWindowsSetup(root), error => {
    assert.equal(error.nothingToResume, true);
    assert.match(error.message, /MiniClaw is already installed at .*Nothing to resume\..*\.\\miniclaw\.cmd/s);
    assert.doesNotMatch(error.message, /VM/);
    return true;
  });
  // A missing installation is still an error.
  await assert.rejects(pendingWindowsSetup(join(root, 'missing')), error => !error.nothingToResume && /No installation at/.test(error.message));
});
