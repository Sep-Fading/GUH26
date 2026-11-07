// Reads actual host hardware and identity.
import { arch, cpus, platform, totalmem } from 'node:os';
// Reads filesystem capacity and executable access.
import { access, stat, statfs } from 'node:fs/promises';
// Checks executable permissions without running programs.
import { constants } from 'node:fs';
// Probes only the selected loopback port.
import { createServer } from 'node:net';
// Resolves parent directories and executable candidates.
import { dirname, join, resolve } from 'node:path';
// Runs bounded builds without shell interpolation.
import { spawn } from 'node:child_process';

// Reads the target volume even when the final installation directory is new.
export async function volume(path) {
  let current = resolve(path);
  while (true) {
    try {
      const info = await statfs(current, { bigint: true });
      return { diskBytes: Number(info.blocks * info.bsize), freeBytes: Number(info.bavail * info.bsize) };
    } catch (error) {
      if (error.code !== 'ENOENT' || dirname(current) === current) throw error;
      current = dirname(current);
    }
  }
}

// Returns hardware totals and space on the proposed installation volume.
export async function inspectHost(path) {
  return { platform: platform(), arch: arch(), cpuCount: cpus().length,
    memoryBytes: totalmem(), ...await volume(path) };
}

// Refuses busy ports without choosing a replacement or leaving a listener behind.
export async function checkPort(port) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('Invalid port');
  const server = createServer(socket => socket.destroy());
  await new Promise((resolve, reject) => {
    server.once('error', error => reject(new Error(`Port ${port} is unavailable: ${error.code}`)));
    server.listen({ port, host: '127.0.0.1', exclusive: true }, () => server.close(resolve));
  });
}

// Locates an existing executable without installing software or reading credentials.
export async function executable(name, candidates = [], { platform = process.platform, env = process.env } = {}) {
  const windows = platform === 'win32';
  const pathValue = Object.entries(env).find(([key]) => key.toUpperCase() === 'PATH')?.[1] ?? '';
  const suffixes = windows && !/\.(exe|com|cmd|bat)$/i.test(name) ? ['.exe', '.com'] : [''];
  const paths = [...candidates, ...pathValue.split(windows ? ';' : ':').filter(Boolean)
    .flatMap(dir => suffixes.map(suffix => join(dir, name + suffix)))];
  for (const path of paths) {
    if (windows && !/\.(exe|com)$/i.test(path)) continue;
    try {
      if (!(await stat(path)).isFile()) continue;
      await access(path, windows ? constants.F_OK : constants.X_OK);
      return resolve(path);
    } catch { /* Continue through explicit candidates and PATH. */ }
  }
  return null;
}

// Runs an owned process group; cancellation terminates its build children too.
export async function run(binary, args, { cwd, signal, onOutput = () => {} } = {}) {
  signal?.throwIfAborted();
  const child = spawn(binary, args, { cwd, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let tail = '';
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => {
    tail = (tail + chunk).slice(-8192);
    onOutput(chunk.toString());
  });
  const stop = () => {
    if (!child.pid) return;
    try {
      if (process.platform === 'win32') {
        const killer = spawn(join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'taskkill.exe'), ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true });
        killer.on('error', () => child.kill());
        killer.on('close', code => { if (code) child.kill(); });
      } else process.kill(-child.pid, 'SIGTERM');
    }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
  };
  signal?.addEventListener('abort', stop, { once: true });
  try {
    await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', code => {
        if (code === 0) return resolve();
        // A build the user stopped is a cancellation, not a failure with a log to read.
        reject(new Error(signal?.aborted ? 'Setup cancelled' : `Command failed (${code}): ${tail}`));
      });
    });
    signal?.throwIfAborted();
  } finally { signal?.removeEventListener('abort', stop); }
}
