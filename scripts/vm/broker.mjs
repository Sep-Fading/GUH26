// Pairs local byte streams without TCP.
import { createServer } from 'node:net';
// Validates private paths and socket permissions.
import { chmod, lstat, open } from 'node:fs/promises';
// Opens route files without following links.
import { constants } from 'node:fs';
// Resolves fixed endpoints beneath private storage.
import { join } from 'node:path';

// Updates only an owned private route file.
export async function writeRoutes(path, routes) {
  const file = await open(path, constants.O_WRONLY | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.uid !== process.getuid() || info.nlink !== 1 || info.mode & 0o077) {
      throw new Error('Routes must use a private, unlinked, user-owned file');
    }
    await file.truncate(0);
    await file.writeFile(`${JSON.stringify(routes)}\n`);
    await file.sync();
  } finally { await file.close(); }
}

// Requires an owned private directory before creating any relay endpoint.
export async function privateDirectory(dir) {
  const info = await lstat(dir);
  if (!info.isDirectory() || info.uid !== process.getuid() || (info.mode & 0o077)) {
    throw new Error('Relay directory must be private and owned by this user');
  }
}

// Claims a new Unix socket without unlinking another process's endpoint.
export async function listen(server, path) {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(path, () => { server.off('error', reject); resolve(); });
  });
  await chmod(path, 0o600);
}

// Pairs one controller with one worker, closing both when either disconnects.
export async function startBroker(dir) {
  await privateDirectory(dir);
  const peers = [null, null];
  const sockets = new Set();
  const servers = [0, 1].map(index => createServer(socket => {
    socket.on('error', () => {});
    if (peers[index]) { socket.destroy(); return; }
    peers[index] = socket;
    sockets.add(socket);
    socket.pause();
    socket.setTimeout(10_000, () => socket.destroy());
    socket.on('close', () => {
      sockets.delete(socket);
      if (peers[index] !== socket) return;
      peers[index] = null;
      peers[1 - index]?.destroy();
    });
    socket.on('end', () => socket.destroy());
    if (peers.every(Boolean)) {
      const [controller, worker] = peers;
      for (const peer of peers) peer.setTimeout(0);
      controller.pipe(worker).pipe(controller);
    }
  }));
  try {
    await listen(servers[0], join(dir, 'controller.sock'));
    await listen(servers[1], join(dir, 'worker.sock'));
  } catch (error) {
    for (const server of servers) server.close();
    throw error;
  }
  return {
    // Stops admission and closes paired or waiting connections before returning.
    async close() {
      for (const socket of sockets) socket.destroy();
      await Promise.all(servers.map(server => new Promise(resolve => server.close(resolve))));
    },
  };
}
