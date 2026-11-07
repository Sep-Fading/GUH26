// Checks actual TLS and negative responses.
import assert from 'node:assert/strict';
// Waits for socket lifecycle events safely.
import { once } from 'node:events';
// Creates and removes private experiment directories.
import { mkdtemp, rm } from 'node:fs/promises';
// Uses Node's parser for CONNECT requests.
import { request } from 'node:http';
// Locates the disposable private socket directory.
import { tmpdir } from 'node:os';
// Resolves only experiment-owned socket file paths.
import { join } from 'node:path';
// Verifies the public server's TLS certificate.
import { connect } from 'node:tls';
// Requires explicit opt-in for external connections.
import { test } from 'node:test';
// Exercises production destination checks and tunneling.
import { startEgress } from '../../scripts/vm/egress.mjs';

// Proves public HTTPS without adding a guest NIC or a host TCP listener.
test('public HTTPS retains certificate verification and blocks local targets', {
  skip: process.env.MINICLAW_PUBLIC_EGRESS !== '1', timeout: 20_000,
}, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-live-egress-'));
  const proxy = await startEgress(dir, { mode: 'public-web' });
  t.after(async () => { await proxy.close(); await rm(dir, { recursive: true, force: true }); });
  // Opens one tunnel through the private proxy and returns its checked response.
  async function tunnel(authority) {
    const req = request({ socketPath: join(dir, 'egress.sock'), method: 'CONNECT',
      path: authority, agent: false });
    t.after(() => req.destroy());
    req.setTimeout(10_000, () => req.destroy(new Error('CONNECT timed out')));
    const connected = once(req, 'connect');
    req.end();
    const [response, socket, head] = await connected;
    t.after(() => socket.destroy());
    assert.equal(head.length, 0);
    return { response, socket };
  }
  for (const target of ['127.0.0.1:443', 'machine.local:443', '169.254.169.254:443']) {
    const { response, socket } = await tunnel(target);
    assert.equal(response.statusCode, 403);
    socket.destroy();
  }
  const { response, socket } = await tunnel('example.com:443');
  assert.equal(response.statusCode, 200);
  const tls = connect({ socket, servername: 'example.com', rejectUnauthorized: true });
  t.after(() => tls.destroy());
  tls.setTimeout(10_000, () => tls.destroy(new Error('TLS timed out')));
  await once(tls, 'secureConnect');
  assert.equal(tls.authorized, true);
  const result = once(tls, 'data');
  tls.write('HEAD / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n');
  const [chunk] = await result;
  assert.match(chunk.toString(), /^HTTP\/1\.[01] (200|301|302) /);
});
