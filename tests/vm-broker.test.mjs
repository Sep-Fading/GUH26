// Checks pairing without opening network listeners.
import assert from 'node:assert/strict';
// Runs connection and egress boundary cases.
import { test } from 'node:test';
// Uses real local streams for pairing.
import { connect, createServer } from 'node:net';
// Creates and removes private test directories.
import { mkdtemp, rm, writeFile, readFile, symlink, link, chmod } from 'node:fs/promises';
// Locates short temporary Unix socket paths.
import { tmpdir } from 'node:os';
// Resolves owned socket names without interpolation.
import { join } from 'node:path';
// Awaits connection events without timing guesses.
import { once } from 'node:events';
// Exercises the fixed VM pairing service.
import { startBroker, writeRoutes } from '../scripts/vm/broker.mjs';
// Checks public destinations before network dialing.
import { publicAddress, destination, startEgress } from '../scripts/vm/egress.mjs';

test('route updates refuse links and non-private files before truncation', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-routes-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const path = join(dir, 'routes.json');
  const routes = [{ port: 10242, path: '/private/tmp/test.sock' }];
  await writeRoutes(path, routes);
  assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), routes);
  const before = await readFile(path, 'utf8');
  await symlink(path, join(dir, 'alias'));
  await assert.rejects(writeRoutes(join(dir, 'alias'), []));
  await link(path, join(dir, 'hard-link'));
  await assert.rejects(writeRoutes(path, []), /private/i);
  await rm(join(dir, 'hard-link'));
  await chmod(path, 0o644);
  await assert.rejects(writeRoutes(path, []), /private/i);
  assert.equal(await readFile(path, 'utf8'), before);
});

// Sends a CONNECT header and returns the proxy's first response.
async function request(dir, authority) {
  const socket = connect(join(dir, 'egress.sock'));
  await once(socket, 'connect');
  const reply = once(socket, 'data');
  socket.write(`CONNECT ${authority} HTTP/1.1\r\nHost: ${authority}\r\n\r\n`);
  const text = (await reply)[0].toString();
  socket.destroy();
  return text;
}

test('CONNECT pins public DNS and never dials rejected requests', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-egress-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const upstream = createServer(socket => socket.on('error', () => {}));
  upstream.listen(join(dir, 'fixture.sock'));
  await once(upstream, 'listening');
  t.after(() => new Promise(resolve => upstream.close(resolve)));
  let records = [{ address: '1.1.1.1', family: 4 }];
  const calls = [];
  const proxy = await startEgress(dir, {
    resolve: async () => records,
    dial: options => { calls.push(options); return connect(join(dir, 'fixture.sock')); },
  });
  t.after(() => proxy.close());
  assert.match(await request(dir, 'auth.openai.com:443'), /^HTTP\/1.1 200/);
  assert.deepEqual(calls, [{ host: '1.1.1.1', family: 4, port: 443 }]);
  for (const authority of ['localhost:443', 'chatgpt.com:80', '127.0.0.1:443']) {
    assert.match(await request(dir, authority), /^HTTP\/1.1 403/);
  }
  records = [{ address: '127.0.0.1', family: 4 }];
  assert.match(await request(dir, 'auth.openai.com:443'), /^HTTP\/1.1 403/);
  records = [{ address: '1.1.1.1', family: 4 }, { address: '192.168.1.1', family: 4 }];
  assert.match(await request(dir, 'chatgpt.com:443'), /^HTTP\/1.1 403/);
  assert.equal(calls.length, 1);
});

test('VM streams pair and disconnect together', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-pair-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const broker = await startBroker(dir);
  t.after(() => broker.close());
  const worker = connect(join(dir, 'worker.sock'));
  const controller = connect(join(dir, 'controller.sock'));
  t.after(() => { worker.destroy(); controller.destroy(); });
  await Promise.all([once(worker, 'connect'), once(controller, 'connect')]);
  const result = once(worker, 'data');
  controller.write('protocol bytes');
  assert.equal((await result)[0].toString(), 'protocol bytes');
  const closed = once(worker, 'close');
  controller.destroy();
  await closed;
});

test('egress refuses local addresses and unapproved destinations', () => {
  for (const ip of ['127.0.0.1', '10.0.0.1', '172.16.2.3', '192.168.1.1',
    '169.254.1.1', '100.64.0.1', '0.0.0.0', '198.18.0.1', '224.0.0.1',
    '255.255.255.255', '::1', '::ffff:127.0.0.1']) assert.equal(publicAddress(ip), false, ip);
  assert.equal(publicAddress('1.1.1.1'), true);
  assert.equal(publicAddress('1.1.1.1', ['1.1.1.1']), false);
  assert.equal(destination('auth.openai.com:443'), 'auth.openai.com');
  for (const name of ['localhost:443', 'chatgpt.com:80', 'auth.openai.com.evil:443',
    'user@chatgpt.com:443', '127.0.0.1:443', 'https://chatgpt.com', 'chatgpt.com:443/path']) {
    assert.throws(() => destination(name));
  }
});

// Broad website access must not broaden the private-address boundary.
test('public web mode pins DNS and rejects local or malformed targets', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-public-web-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const upstream = createServer(socket => socket.on('error', () => {}));
  upstream.listen(join(dir, 'fixture.sock'));
  await once(upstream, 'listening');
  t.after(() => new Promise(resolve => upstream.close(resolve)));
  const calls = [];
  let records = [{ address: '1.1.1.1', family: 4 }];
  const proxy = await startEgress(dir, { mode: 'public-web',
    resolve: async () => records,
    dial: options => { calls.push(options); return connect(join(dir, 'fixture.sock')); },
  });
  t.after(() => proxy.close());
  assert.match(await request(dir, 'www.wikipedia.org:443'), /^HTTP\/1.1 200/);
  assert.deepEqual(calls, [{ host: '1.1.1.1', family: 4, port: 443 }]);
  for (const authority of ['localhost:443', 'machine.local:443', '127.1:443',
    '2130706433:443', '[::1]:443', 'example.com:22', 'user@example.com:443',
    'example.com:443/path', 'bad..example.com:443']) {
    assert.match(await request(dir, authority), /^HTTP\/1.1 403/, authority);
  }
  for (const address of ['127.0.0.1', '192.168.1.1', '169.254.169.254', '::ffff:127.0.0.1']) {
    records = [{ address, family: 4 }];
    assert.match(await request(dir, 'www.wikipedia.org:443'), /^HTTP\/1.1 403/);
  }
  assert.equal(calls.length, 1, 'Denied destinations must never reach the dialer');
});

// Invalid saved policy cannot silently enable broader egress.
test('egress rejects unknown modes before opening a socket', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-egress-mode-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await assert.rejects(async () => {
    const proxy = await startEgress(dir, { mode: 'anything' });
    await proxy.close();
  }, /mode/i);
});
