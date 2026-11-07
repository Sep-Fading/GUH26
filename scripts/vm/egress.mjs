// Parses bounded proxy requests without TLS interception.
import { createServer } from 'node:http';
// Dials checked addresses and classifies IPv4.
import { BlockList, connect, isIPv4 } from 'node:net';
// Resolves approved names before dialing addresses.
import { lookup } from 'node:dns/promises';
// Excludes addresses assigned to this host.
import { networkInterfaces } from 'node:os';
// Locates the fixed private egress socket.
import { join } from 'node:path';
// Shares private directory and listener checks.
import { listen, privateDirectory } from './broker.mjs';

const hosts = new Set(['auth.openai.com', 'chatgpt.com', 'api.openai.com']);
const denied = new BlockList();
for (const [address, prefix] of [['0.0.0.0', 8], ['10.0.0.0', 8], ['100.64.0.0', 10],
  ['127.0.0.0', 8], ['169.254.0.0', 16], ['172.16.0.0', 12], ['192.168.0.0', 16],
  ['192.0.0.0', 24], ['192.0.2.0', 24], ['192.88.99.0', 24], ['198.18.0.0', 15],
  ['198.51.100.0', 24], ['203.0.113.0', 24], ['224.0.0.0', 4], ['240.0.0.0', 4]]) {
  denied.addSubnet(address, prefix, 'ipv4');
}

// Accepts only public IPv4 addresses not assigned to the host.
export function publicAddress(address, local = Object.values(networkInterfaces()).flat()
  .filter(Boolean).map(info => info.address)) {
  return isIPv4(address) && !denied.check(address, 'ipv4') && !local.includes(address);
}

// Accepts HTTPS names under the selected policy; local names and literals fail.
export function destination(authority, mode = 'openai') {
  const match = /^([a-z0-9.-]+):443$/.exec(authority ?? '');
  const host = match?.[1];
  const labels = host?.split('.') ?? [];
  const publicName = host?.length <= 253 && labels.length >= 2
    && labels.every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))
    && /^[a-z]{2,63}$/.test(labels.at(-1))
    && !['localhost', 'local', 'internal', 'home', 'lan', 'invalid', 'test'].includes(labels.at(-1));
  if (!host || !(mode === 'openai' ? hosts.has(host) : mode === 'public-web' && publicName)) {
    throw new Error('Egress destination is not permitted');
  }
  return host;
}

// Bounds DNS completion without accepting late results after a failed request.
async function resolvePublic(host, resolve = lookup) {
  let timer;
  try {
    const records = await Promise.race([resolve(host, { family: 4, all: true }),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('DNS timeout')), 5000); })]);
    if (!records.length || records.some(record => !publicAddress(record.address))) {
      throw new Error('Egress address is not permitted');
    }
    return records[0].address;
  } finally { clearTimeout(timer); }
}

// Serves CONNECT over a private Unix socket, retaining end-to-end vendor TLS.
export async function startEgress(dir, { mode = 'openai', resolve = lookup, dial = connect } = {}) {
  if (!['openai', 'public-web'].includes(mode)) throw new Error('Unknown egress mode');
  await privateDirectory(dir);
  const sockets = new Set();
  const server = createServer({ maxHeaderSize: 8192, headersTimeout: 5000,
    requestTimeout: 5000 }, (_, response) => { response.writeHead(403); response.end(); });
  server.maxConnections = 32;
  server.on('connection', socket => {
    sockets.add(socket);
    socket.setTimeout(5000, () => socket.destroy());
    socket.on('error', () => {});
    socket.on('close', () => sockets.delete(socket));
  });
  server.on('connect', async (request, client, head) => {
    client.pause();
    let upstream;
    try {
      const address = await resolvePublic(destination(request.url, mode), resolve);
      if (client.destroyed) return;
      upstream = dial({ host: address, family: 4, port: 443 });
      sockets.add(upstream);
      upstream.on('error', () => client.destroy());
      upstream.on('close', () => { sockets.delete(upstream); client.destroy(); });
      client.on('close', () => upstream.destroy());
      upstream.setTimeout(120_000, () => upstream.destroy());
      client.setTimeout(120_000);
      upstream.once('connect', () => {
        if (client.destroyed) { upstream.destroy(); return; }
        client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
        if (head.length) upstream.write(head);
        client.pipe(upstream).pipe(client);
      });
    } catch {
      upstream?.destroy();
      client.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
    }
  });
  await listen(server, join(dir, 'egress.sock'));
  return {
    // Closes owned proxy connections and its private listener.
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
    },
  };
}
