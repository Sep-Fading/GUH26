import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { access, copyFile, mkdtemp, mkdir, writeFile, readFile, rm, realpath } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { connect, createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { defaults } from './fixtures.mjs';
import { providerFixturePaths, nativeArtifacts, peMachine } from './provider-fixture-paths.mjs';

const peerURL = new URL('./fake-codex.mjs', import.meta.url).href;
const root = fileURLToPath(new URL('../../', import.meta.url));
const artifacts = nativeArtifacts(root, process.env.MINICLAW_TEST_TARGET);
export const command = JSON.parse(process.env.MINICLAW_TEST_COMMAND ?? JSON.stringify([artifacts.backend]));

export async function requireApplication() {
  assert.ok(Array.isArray(command) && command.length && command.every(x => typeof x === 'string'), 'MINICLAW_TEST_COMMAND must be a JSON argv array');
  try { await access(command[0], constants.X_OK); }
  catch { throw new Error(`Application not implemented/built: ${command[0]}. These are acceptance tests, not skipped tests. Build the future Rust server or set MINICLAW_TEST_COMMAND to an absolute executable argv array.`); }
  if (process.platform === 'win32' && artifacts.machine) {
    for (const path of [command[0], artifacts.provider]) {
      assert.equal(peMachine(await readFile(path)), artifacts.machine, `Wrong architecture: ${path}`);
    }
  }
}

/** @template T @param {() => T | Promise<T>} read @param {(value: T) => boolean} predicate */
export async function eventually(read, predicate, label = 'condition', timeout = 4000) {
  const end = Date.now() + timeout;
  let value;
  while (Date.now() < end) {
    value = await read();
    if (predicate(value)) return value;
    await new Promise(r => setTimeout(r, 20));
  }
  assert.fail(`Timed out waiting for ${label}. Last value: ${JSON.stringify(value)}`);
}

/** @returns {Promise<{requests: any[], responses: any[], held: any[], initialized: boolean, acknowledged: boolean, codexHome: string}>} */
export function control(path, payload) {
  return new Promise((resolve, reject) => {
    const socket = connect(path);
    let data = '';
    socket.setTimeout(3000, () => socket.destroy(new Error('Test peer control timed out')));
    socket.on('error', reject);
    socket.on('connect', () => socket.write(`${JSON.stringify(payload)}\n`));
    socket.on('data', chunk => { data += chunk; });
    socket.on('end', () => {
      try {
        const response = JSON.parse(data);
        if (response.error) reject(new Error(response.error));
        else resolve(response.result);
      } catch (error) { reject(error); }
    });
  });
}

// Removes a test folder, retrying while Windows still holds a handle inside it, for at most
// `limit` ms in total. Node's own `maxRetries` restarts its backoff at every nested folder, so an
// open file deep inside kept one removal retrying for many minutes (VM2, 2026-10-04).
export async function removeTree(path, limit = 30000) {
  const end = Date.now() + limit;
  for (;;) {
    try { return await rm(path, { recursive: true, force: true }); } catch (error) {
      if (!['EBUSY', 'EPERM', 'ENOTEMPTY', 'EMFILE', 'ENFILE'].includes(error.code) || Date.now() >= end) {
        throw new Error(`Could not remove test folder ${path} within ${limit} ms: ${error.code} ${error.message}`, { cause: error });
      }
      await new Promise(resolve => setTimeout(resolve, 250));
    }
  }
}

export async function peerFixture(overrides = {}) {
  const container = await mkdtemp(join(tmpdir(), 'mc-'));
  const dir = process.platform === 'win32' ? join(container, 'private') : container;
  if (process.platform === 'win32') {
    try {
      await promisify(execFile)(command[0], [...command.slice(1), '--secure-data-dir', dir],
        { timeout: 15000, windowsHide: true });
    } catch (error) {
      await rm(container, { recursive: true, force: true });
      throw error;
    }
  }
  await mkdir(join(dir, 'chat'));
  // The backend's own spelling of its chat folder: the final path, on Windows in its plain drive form.
  const cwd = await realpath(join(dir, 'chat'));
  const codexHome = join(dir, 'codex-home');
  await mkdir(codexHome);
  const { socket, binary, script } = providerFixturePaths(dir, randomUUID());
  const scenario = join(dir, 'scenario.json');
  await writeFile(scenario, JSON.stringify({ handlers: { ...defaults(cwd), ...overrides } }));
  await writeFile(script, `#!/usr/bin/env node\nprocess.env.MC_SCENARIO=${JSON.stringify(scenario)};\nprocess.env.MC_CONTROL=${JSON.stringify(socket)};\nawait import(${JSON.stringify(peerURL)});\n`, { mode: 0o700 });
  if (process.platform === 'win32') {
    await copyFile(artifacts.provider, binary);
    await writeFile(join(dir, 'codex-fixture.node-path'), process.execPath);
  }
  // A stand-in for the user's Documents folder, outside MiniClaw's own (protected) data folder.
  const documents = await mkdtemp(join(tmpdir(), 'mc-documents-'));
  return { dir, cwd, codexHome, socket, binary, script, documents,
    call: payload => control(socket, payload),
    async ready() {
      await eventually(async () => { try { await control(socket, { op: 'inspect' }); return true; } catch { return false; } }, Boolean, 'fake Codex control endpoint');
    },
    async cleanup() {
      // Also reap the test peer if a broken application failed to stop its child.
      try { await control(socket, { op: 'shutdown' }); } catch { /* The peer may already have exited. */ }
      await removeTree(container);
      await removeTree(documents);
    },
  };
}

export async function freePort() {
  const listener = createServer();
  listener.listen(0, '127.0.0.1');
  await once(listener, 'listening');
  const port = listener.address().port;
  await new Promise((resolve, reject) => listener.close(error => error ? reject(error) : resolve()));
  return port;
}

export async function stopChild(child) {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit');
  if (process.platform === 'win32' && child.stdin) child.stdin.end();
  else child.kill('SIGTERM');
  const timer = setTimeout(() => child.kill('SIGKILL'), 1500);
  try { await exited; } finally { clearTimeout(timer); }
}

// `options.env` adds backend variables; the real Claude CLI stays disabled unless a test selects one.
// `options.codex === false` starts the app with no Codex runtime at all.
// `options.real === true` uses the installed Codex and its existing sign-in instead of the fake
// peer. Only opt-in experiments do that; it makes real model calls.
export async function launchApp(overrides = {}, options = {}) {
  await requireApplication();
  const peer = await peerFixture(overrides);
  if (process.platform === 'win32' && options.git) {
    try {
      const archive = process.env.MINICLAW_TEST_GIT_ARCHIVE;
      if (!archive) throw new Error('Set MINICLAW_TEST_GIT_ARCHIVE to the pinned Git ZIP (bundled MiniClaw build or official MinGit) before Windows workspace acceptance');
      const { installWindowsGit, windowsGitPackage } = await import('../../scripts/setup/windows-git.mjs');
      const { windowsGitBundle } = await import('../../scripts/setup/windows-git-bundle.mjs');
      // The bundled MiniClaw build is selected by its file name; any other archive must match the official MinGit pin.
      const bundled = windowsGitBundle(process.arch);
      await installWindowsGit({ runtime: root, backend: command[0], data: peer.dir, archive, arch: process.arch,
        pin: archive.replaceAll('\\', '/').endsWith(`/${bundled.file}`) ? bundled : windowsGitPackage(process.arch) });
    } catch (error) { await peer.cleanup(); throw error; }
  }
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  let logs = '';
  let child;
  // A test's own handles on the backend's database close before its folder is removed: Windows
  // cannot delete an open file, and the retried removal then runs for many minutes (VM2, 2026-10-04).
  const databases = [];
  const close = async () => { for (const db of databases.splice(0)) db.close(); await stopChild(child); await peer.cleanup(); };
  async function boot() {
    // Tests never look into the user's own Codex or Claude folders, where real skills and sign-ins live.
    const env = options.real ? { ...process.env }
      : { ...process.env, CODEX_BIN: peer.binary, CODEX_HOME: peer.codexHome, CLAUDE_CONFIG_DIR: `${peer.dir}/claude-home` };
    const overrides = { ...options.env };
    if (process.platform === 'win32' && !options.real && overrides.CLAUDE_BIN?.endsWith('.mjs')) {
      const binary = join(peer.dir, 'claude-fixture.exe');
      const target = pathToFileURL(overrides.CLAUDE_BIN).href;
      const fixtureEnv = Object.fromEntries(Object.entries(overrides).filter(([name]) => name.startsWith('MC_')));
      await copyFile(artifacts.provider, binary);
      await writeFile(join(peer.dir, 'claude-fixture.node-path'), process.execPath);
      await writeFile(join(peer.dir, 'claude-fixture.mjs'),
        `Object.assign(process.env, ${JSON.stringify(fixtureEnv)}); await import(${JSON.stringify(target)});\n`);
      overrides.CLAUDE_BIN = binary;
    }
    child = spawn(command[0], command.slice(1), { cwd: root,
      env: { ...env, MINICLAW_PORT: String(port),
        MINICLAW_DATA_DIR: peer.dir, MINICLAW_CHAT_DIR: peer.cwd,
        // A conversation's own folder goes under this Documents folder, never the user's real one.
        MINICLAW_DOCUMENTS_DIR: peer.documents,
        ...(process.platform === 'win32' ? { MINICLAW_LAUNCHER_STDIN: '1' } : {}),
        // Secrets of MCP servers go to a private file here, never to the real Keychain.
        MINICLAW_TEST_SECRETS: `${peer.dir}/secrets.json`,
        CLAUDE_BIN: '', ...overrides,
        ...(options.codex === false ? { CODEX_BIN: '' } : {}) },
      stdio: [process.platform === 'win32' ? 'pipe' : 'ignore', 'pipe', 'pipe'] });
    
    let launchError;
    child.on('error', error => { launchError = error; });
    for (const stream of [child.stdout, child.stderr]) {
      stream.on('data', b => { logs = (logs + b).slice(-65536); });
    }
    try {
      await eventually(async () => {
        if (launchError) throw launchError;
        if (child.exitCode !== null) throw new Error(`Application exited before listening: ${logs}`);
        try { return (await fetch(`${url}/api/health`, { signal: AbortSignal.timeout(500) })).status; }
        catch { return 0; }
      }, status => status === 200, 'backend health endpoint', 8000);
      if (options.codex !== false && !options.real) await peer.ready();
    } catch (error) { await close(); throw error; }
  }
  await boot();
  return {
    url, peer, close, logs: () => logs,
    /** The backend's process id, for a test that ends it the hard way. */
    pid: () => child.pid,
    /** Opens the backend's metadata database for the test; `close` closes it first. */
    database() { const db = new DatabaseSync(join(peer.dir, 'agents.sqlite3')); databases.push(db); return db; },
    async restart() {
      await stopChild(child);
      if (process.platform !== 'win32') await rm(peer.socket, { force: true });
      await boot();
    },
    async request(path, options = {}) {
      const headers = { Origin: url, 'Sec-Fetch-Site': 'same-origin', ...options.headers };
      if (options.body !== undefined) {
        headers['Content-Type'] ??= 'application/json';
        headers['X-MiniClaw-Request'] ??= '1';
      }
      return fetch(`${url}${path}`, { ...options, headers, signal: options.signal ?? AbortSignal.timeout(5000),
        body: options.body === undefined || typeof options.body === 'string' || options.body instanceof Uint8Array
          ? options.body : JSON.stringify(options.body) });
    },
    inspect: () => peer.call({ op: 'inspect' }),
    set: (method, handler) => peer.call({ op: 'set', method, handler }),
    emit: (...frames) => peer.call({ op: 'emit', frames }),
  };
}

export async function json(response, status = 200) {
  assert.equal(response.status, status, `Unexpected HTTP status: ${await response.clone().text()}`);
  assert.match(response.headers.get('content-type') ?? '', /application\/json/);
  return response.json();
}

export async function events(app, path, headers = {}) {
  const abort = new AbortController();
  const response = await app.request(path, { headers, signal: abort.signal });
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-type') ?? '', /text\/event-stream/);
  const seen = [];
  let buffer = '';
  const decoder = new TextDecoder();
  const reader = response.body.getReader();
  let failure;
  let ended = false;
  const reading = (async () => {
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer = (buffer + decoder.decode(value, { stream: true })).replace(/\r\n/g, '\n');
        let split;
        while ((split = buffer.indexOf('\n\n')) !== -1) {
          const block = buffer.slice(0, split);
          buffer = buffer.slice(split + 2);
          const fields = block.split('\n');
          const data = fields.filter(l => l.startsWith('data:')).map(l => l.slice(5).trimStart()).join('\n');
          if (!data) continue;
          seen.push({ id: fields.find(l => l.startsWith('id:'))?.slice(3).trim(),
            event: fields.find(l => l.startsWith('event:'))?.slice(6).trim() ?? 'message', data: JSON.parse(data) });
        }
      }
    } catch (error) { if (!abort.signal.aborted) failure = error; }
    finally { ended = true; reader.releaseLock(); }
  })();
  return { seen,
    next: (name) => eventually(() => { if (failure) throw failure; return seen.find(e => e.event === name); }, Boolean, `SSE ${name}`),
    waitClosed: () => eventually(() => ended, Boolean, 'SSE closure'),
    close: async () => { abort.abort(); await reading; },
  };
}
