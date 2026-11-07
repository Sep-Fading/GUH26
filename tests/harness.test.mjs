import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { peerFixture, stopChild, eventually, events, removeTree } from './support/harness.mjs';
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Acceptance fixtures must satisfy the same private-state boundary as installed data.
test('Windows fixture has a protected current-user-only DACL', { skip: process.platform !== 'win32' }, async t => {
  const fixture = await peerFixture();
  t.after(() => fixture.cleanup());
  const script = '$acl=Get-Acl -LiteralPath $env:MC_FIXTURE_DIR; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; '
    + 'if (!$acl.AreAccessRulesProtected) { throw "Fixture inherits its Temp ACL" }; '
    + 'if (!$acl.Access.Count) { throw "Fixture has no explicit access rule" }; '
    + 'if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw "Fixture has a different owner" }; '
    + 'foreach ($entry in $acl.Access) { if ($entry.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw "Fixture grants another identity" } }';
  await promisify(execFile)('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script],
    { env: { ...process.env, MC_FIXTURE_DIR: fixture.dir }, timeout: 15000 });
});

// A folder Windows cannot remove yet (a process still works inside it) stops the removal at its
// time limit with the folder's name, instead of retrying for minutes and hanging the suite.
test('folder removal gives up at its time limit', { skip: process.platform !== 'win32' && 'only Windows keeps a folder in use from being removed' }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-remove-'));
  const deeper = join(dir, 'nested', 'deeper');
  await mkdir(deeper, { recursive: true });
  await writeFile(join(deeper, 'file.txt'), 'kept');
  // A process's current folder is open without delete sharing until it exits.
  const holder = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { cwd: deeper, stdio: 'ignore' });
  t.after(async () => { await stopChild(holder); await removeTree(dir); });
  await once(holder, 'spawn');
  const started = Date.now();
  await assert.rejects(removeTree(dir, 1500), /Could not remove test folder .*mc-remove-.* within 1500 ms/);
  assert.ok(Date.now() - started < 6000, `bounded: ${Date.now() - started} ms`);
  await stopChild(holder);
  await removeTree(dir);
});

async function peerFor(t, overrides = {}) {
  const fixture = await peerFixture(overrides);
  const child = spawn(process.execPath, [fixture.script, 'app-server', '--listen', 'stdio://'], {
    env: { ...process.env, CODEX_HOME: fixture.codexHome }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  const frames = [];
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => frames.push(JSON.parse(line)));
  t.after(async () => { await stopChild(child); lines.close(); await fixture.cleanup(); });
  await fixture.ready();
  const send = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  const received = id => eventually(() => frames.find(x => x.id === id), Boolean, `RPC reply ${id}`);
  const initialize = async () => {
    send({ id: 0, method: 'initialize', params: { clientInfo: { name: 'harness', version: '1' } } });
    await received(0);
    send({ method: 'initialized' });
  };
  return { ...fixture, child, frames, send, received, initialize };
}

test('fake peer rejects ordinary calls before the handshake', async t => {
  const p = await peerFor(t);
  p.send({ id: 'before-init', method: 'account/read' });
  assert.equal((await p.received('before-init')).error.message, 'Not initialized');
  await p.initialize();
  p.send({ id: 1, method: 'account/read' });
  assert.equal((await p.received(1)).result.account.email, 'reader@example.test');
  assert.equal((await p.call({ op: 'inspect' })).codexHome, p.codexHome);
});

test('fake peer rejects a duplicate initialization', async t => {
  const p = await peerFor(t); await p.initialize();
  p.send({ id: 2, method: 'initialize' });
  assert.equal((await p.received(2)).error.message, 'Already initialized');
});

test('fake peer emits a notification before an acknowledgement when scripted', async t => {
  const p = await peerFor(t, { 'turn/start': { before: [{ method: 'fixture/event', params: { text: 'Early' } }], result: { accepted: true } } });
  await p.initialize();
  p.send({ id: 'turn', method: 'turn/start' });
  await p.received('turn');
  assert.ok(p.frames.findIndex(x => x.method === 'fixture/event') < p.frames.findIndex(x => x.id === 'turn'));
});

test('fake peer holds and releases replies by ID in reverse order', async t => {
  const p = await peerFor(t, { 'fixture/read': { hold: true } }); await p.initialize();
  p.send({ id: 'a', method: 'fixture/read' }); p.send({ id: 2, method: 'fixture/read' });
  await eventually(() => p.call({ op: 'inspect' }), x => x.held.length === 2);
  await p.call({ op: 'release', id: 2, reply: { result: 'second' } });
  await p.call({ op: 'release', id: 'a', reply: { result: 'first' } });
  await p.received('a');
  assert.deepEqual(p.frames.filter(x => x.id !== 0).map(x => x.result), ['second', 'first']);
});

test('fake peer records client responses to server-initiated requests', async t => {
  const p = await peerFor(t); await p.initialize();
  await p.call({ op: 'emit', frames: [{ id: 'from-server', method: 'fixture/request' }] });
  await p.received('from-server');
  p.send({ id: 'from-server', error: { code: -32601, message: 'Unsupported' } });
  const seen = await eventually(() => p.call({ op: 'inspect' }), x => x.responses.length === 1);
  assert.equal(seen.responses[0].error.code, -32601);
});

test('unscripted upstream methods fail visibly rather than return a convenient success', async t => {
  const p = await peerFor(t); await p.initialize();
  p.send({ id: 3, method: 'not/in/the/fixture' });
  assert.equal((await p.received(3)).error.code, -32601);
});

test('SSE reader handles split CRLF, UTF-8, comments, and multiple data lines', async () => {
  const encoder = new TextEncoder();
  const bytes = encoder.encode(': keepalive\r\n\r\nid: epoch:1\r\nevent: message.delta\r\ndata: {"delta":\r\ndata: "caf\u00e9"}\r\n\r\n');
  const body = new ReadableStream({ start(controller) {
    for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
    controller.close();
  } });
  const stream = await events({ request: async () => new Response(body, { headers: { 'Content-Type': 'text/event-stream' } }) }, '/fixture');
  try {
    assert.deepEqual(await stream.next('message.delta'), { id: 'epoch:1', event: 'message.delta', data: { delta: 'caf\u00e9' } });
    await stream.waitClosed();
    assert.equal(stream.seen.length, 1);
  } finally { await stream.close(); }
});

// Workspace tests use the same verified embedded package as production setup.
test('Windows Git fixture survives backend restart', { skip: process.platform !== 'win32' }, async t => {
  const { launchApp } = await import('./support/harness.mjs');
  const { readFile } = await import('node:fs/promises');
  const { join, relative } = await import('node:path');
  const app = await launchApp({}, { git: true });
  t.after(() => app.close());
  const configPath = join(app.peer.dir, 'windows-git.json');
  const before = JSON.parse(await readFile(configPath, 'utf8'));
  assert.match(relative(app.peer.dir, before.root), /^windows-git-runtime\\[a-f0-9-]+$/);
  assert.ok((await readFile(join(before.root, before.program))).length > 0);
  await app.restart();
  assert.deepEqual(JSON.parse(await readFile(configPath, 'utf8')), before);
});
