// Verifies the relocated service's paths, startup checks and shutdown.
import assert from 'node:assert/strict';
// Runs isolated backend service acceptance cases.
import { before, test } from 'node:test';
// Starts relocated binaries without shell expansion.
import { spawn } from 'node:child_process';
// Observes startup without polling process state.
import { once } from 'node:events';
// Creates package fixtures outside the checkout.
import { access, copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
// Resolves package assets and private data.
import { join } from 'node:path';
// Reuses disposable peers and bounded process cleanup.
import {
  command, eventually, freePort, json, peerFixture, requireApplication, stopChild,
} from './support/harness.mjs';

before(requireApplication);

// Starts a relocated service with disposable data and a watched Codex substitute.
async function service(t, args = [], missingAssets = false) {
  const peer = await peerFixture();
  let child;
  t.after(async () => {
    if (child) await stopChild(child);
    await peer.cleanup();
  });
  const bundle = join(peer.dir, 'bundle with spaces');
  await mkdir(bundle);
  // Windows starts only a named .exe, and the backend stops cleanly when the launcher closes its
  // stdin (what start-host.mjs does there); a terminated process would not exit with 0.
  const windows = process.platform === 'win32';
  const executable = join(bundle, `miniclaw-server${windows ? '.exe' : ''}`);
  await copyFile(command[0], executable);
  const assets = join(bundle, 'web');
  await mkdir(assets);
  if (!missingAssets) await writeFile(join(assets, 'index.html'), '<h1>Packaged UI</h1>');
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  child = spawn(executable, [...command.slice(1), ...args], {
    cwd: peer.cwd,
    env: { ...process.env, MINICLAW_PORT: String(port), MINICLAW_DATA_DIR: peer.dir,
      MINICLAW_CHAT_DIR: peer.cwd, MINICLAW_WEB_DIR: assets,
      CODEX_HOME: peer.codexHome, CODEX_BIN: peer.binary,
      ...(windows ? { MINICLAW_LAUNCHER_STDIN: '1' } : {}) },
    stdio: [windows ? 'pipe' : 'ignore', 'pipe', 'pipe'],
  });
  let logs = '';
  for (const stream of [child.stdout, child.stderr]) {
    stream.on('data', chunk => { logs = (logs + chunk).slice(-8000); });
  }
  await once(child, 'spawn');
  return { child, peer, url, logs: () => logs,
    // Waits for backend health, failing immediately on an early process exit.
    async ready() {
      return eventually(async () => {
        assert.equal(child.exitCode, null, logs);
        try { return await (await fetch(`${url}/api/health`, {
          signal: AbortSignal.timeout(500),
        })).json(); }
        catch { return null; }
      }, Boolean, 'relocated backend health');
    },
  };
}

test('the relocated service serves its packaged UI', async t => {
  const app = await service(t);
  await app.ready();
  await app.peer.ready();
  assert.equal(await (await fetch(app.url)).text(), '<h1>Packaged UI</h1>');
});

test('missing package assets fail before startup', async t => {
  const app = await service(t, [], true);
  const code = await eventually(() => app.child.exitCode,
    value => value !== null, 'invalid package rejection');
  assert.notEqual(code, 0);
  assert.match(app.logs(), /MINICLAW_WEB_DIR/);
  await assert.rejects(access(app.peer.socket), { code: 'ENOENT' });
  await assert.rejects(access(join(app.peer.dir, 'agents.sqlite3')), { code: 'ENOENT' });
});

test('the retired controller mode and stray arguments start nothing', async t => {
  for (const args of [['--controller'], ['--computer-helper']]) {
    const app = await service(t, args);
    const code = await eventually(() => app.child.exitCode,
      value => value !== null, 'argument rejection');
    assert.notEqual(code, 0);
    await assert.rejects(access(app.peer.socket), { code: 'ENOENT' });
    await assert.rejects(access(join(app.peer.dir, 'agents.sqlite3')), { code: 'ENOENT' });
  }
});

test('shutdown preserves private storage', async t => {
  const app = await service(t);
  await app.ready();
  const marker = join(app.peer.dir, 'keep.txt');
  await writeFile(marker, 'persistent service data');
  await stopChild(app.child);
  assert.equal(app.child.exitCode, 0, app.logs());
  assert.equal(await readFile(marker, 'utf8'), 'persistent service data');
  await access(join(app.peer.dir, 'agents.sqlite3'));
});
