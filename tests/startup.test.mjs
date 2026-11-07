import assert from 'node:assert/strict';
import { before, test } from 'node:test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import {
  command, eventually, freePort, json, peerFixture,
  requireApplication, stopChild,
} from './support/harness.mjs';

before(requireApplication);

// Backends and fixture folders started by each test. node:test runs `t.after` hooks in the order
// they were added and stops at the first that fails, so a later backend that still had an earlier
// fixture open as its data folder was never stopped: on Windows the open files made that folder's
// removal fail (EBUSY) and the running backend kept the test process alive (VM2, 2026-10-04).
// Every backend of a test is therefore stopped before any of its folders is removed.
const started = new WeakMap();
// Registers one backend and its fixture for the test's single teardown.
function own(t, child, peer) {
  if (!started.has(t)) {
    const owned = { children: [], peers: [] };
    started.set(t, owned);
    t.after(async () => {
      await Promise.all(owned.children.map(stopChild));
      for (const fixture of owned.peers) await fixture.cleanup();
    });
  }
  started.get(t).children.push(child);
  started.get(t).peers.push(peer);
}

async function start(t, port, env = {}) {
  const peer = await peerFixture();
  const child = spawn(command[0], command.slice(1), {
    env: {
      ...process.env,
      MINICLAW_PORT: String(port),
      MINICLAW_DATA_DIR: peer.dir,
      MINICLAW_CHAT_DIR: peer.cwd,
      CODEX_HOME: peer.codexHome,
      CODEX_BIN: peer.binary,
      // As in launchApp: never the user's own Claude Code or its sign-in.
      CLAUDE_BIN: '',
      CLAUDE_CONFIG_DIR: `${peer.dir}/claude-home`,
      ...env,
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  own(t, child, peer);
  let error;
  let stderr = '';
  child.on('error', value => { error = value; });
  child.stderr.on('data', chunk => {
    stderr = (stderr + chunk).slice(-4096);
  });
  await once(child, 'spawn');
  return {
    child,
    peer,
    stderr: () => stderr,
    check() { if (error) throw error; },
  };
}

test('startup serves health', async t => {
  const port = await freePort();
  const app = await start(t, port);
  const response = await eventually(async () => {
    app.check();
    assert.equal(app.child.exitCode, null, app.stderr());
    try {
      return await fetch(`http://127.0.0.1:${port}/api/health`, {
        signal: AbortSignal.timeout(500),
      });
    } catch { return null; }
  }, Boolean, 'listening backend');
  assert.equal((await json(response)).backend, 'ok');
});

test('one instance per data directory', async t => {
  const firstPort = await freePort();
  const first = await start(t, firstPort);
  await first.peer.ready();
  const secondPort = await freePort();
  const second = await start(t, secondPort, {
    MINICLAW_DATA_DIR: first.peer.dir,
  });
  const code = await eventually(() => second.child.exitCode,
    value => value !== null, 'duplicate instance rejection');
  assert.notEqual(code, 0);
  assert.match(second.stderr(), /already running/i);
  await assert.rejects(fetch(`http://127.0.0.1:${secondPort}/api/health`));
  assert.equal((await json(await fetch(
    `http://127.0.0.1:${firstPort}/api/health`,
  ))).backend, 'ok');

  await stopChild(first.child);
  const next = await start(t, secondPort, {
    MINICLAW_DATA_DIR: first.peer.dir,
  });
  await next.peer.ready();
  assert.equal(next.child.exitCode, null, next.stderr());
});

test('only the app origin is accepted', async t => {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  const app = await start(t, port, {
    MINICLAW_DEV_ORIGIN: 'https://untrusted.example',
  });
  await app.peer.ready();
  for (const origin of [
    'https://untrusted.example', 'http://127.0.0.1:5173', 'null',
  ]) {
    const response = await fetch(`${url}/api/account`, {
      headers: { Origin: origin, 'Sec-Fetch-Site': 'same-site' },
    });
    await json(response, 403);
    assert.equal(response.headers.get('access-control-allow-origin'), null);
  }
  assert.equal((await json(await fetch(`${url}/api/account`, {
    headers: { Origin: url },
  }))).status, 'signed_in');
});

test('startup rejects invalid ports', async t => {
  for (const port of ['invalid', '-1', '65536', '']) {
    const app = await start(t, port);
    const code = await eventually(() => {
      app.check();
      return app.child.exitCode;
    }, value => value !== null, 'startup failure');
    assert.notEqual(code, 0);
    assert.match(app.stderr(), /MINICLAW_PORT/);
  }
});

test('startup rejects an occupied port', async t => {
  const listener = createServer();
  t.after(() => new Promise(resolve => listener.close(resolve)));
  listener.listen(0, '127.0.0.1');
  await once(listener, 'listening');
  const app = await start(t, listener.address().port);
  const code = await eventually(() => {
    app.check();
    return app.child.exitCode;
  }, value => value !== null, 'bind failure');
  assert.notEqual(code, 0);
  assert.match(app.stderr(), /listen|bind|address/i);
});

// MiniClaw's own diagnostic events (tracing) go to stderr at the level MINICLAW_LOG names.
// Unset, Windows prints warnings and errors and macOS prints none, as before.
test('diagnostic events reach stderr at the chosen level', async t => {
  const port = await freePort();
  const app = await start(t, port, { MINICLAW_LOG: 'info' });
  await app.peer.ready();
  await eventually(() => app.stderr(), text => /Backend listening/.test(text), 'info event on stderr');
  assert.doesNotMatch(app.stderr(), /\x1b\[/, 'plain text without colour codes');
  const quiet = await start(t, await freePort(), { MINICLAW_LOG: undefined });
  await quiet.peer.ready();
  assert.doesNotMatch(quiet.stderr(), /Backend listening/);
});

test('startup rejects an unknown log level', async t => {
  const app = await start(t, await freePort(), { MINICLAW_LOG: 'miniclaw_server=trace,hyper=trace' });
  const code = await eventually(() => {
    app.check();
    return app.child.exitCode;
  }, value => value !== null, 'startup failure');
  assert.notEqual(code, 0);
  assert.match(app.stderr(), /MINICLAW_LOG must be one of off, error, warn, info, debug, trace/);
});
