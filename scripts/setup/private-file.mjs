// Creates protected native setup state without exposing payloads in command arguments.
import { execFile } from 'node:child_process';
import { join } from 'node:path';
import { writeFile, readFile } from 'node:fs/promises';

export async function writePrivateFile(runtime, path, contents, { backend = join(runtime, 'bin', 'miniclaw-server.exe') } = {}) {
  if (Buffer.byteLength(contents) > 65536) throw new Error('Private setup payload exceeds limit');
  if (process.platform !== 'win32') {
    try { await writeFile(path, contents, { flag: 'wx', mode: 0o600 }); }
    catch (error) { if (error.code !== 'EEXIST' || await readFile(path, 'utf8') !== contents) throw error; }
    return;
  }
  await new Promise((resolve, reject) => {
    const child = execFile(backend, ['--write-private-file', path],
      { windowsHide: true, timeout: 15000, maxBuffer: 4096 }, error => {
        if (error) reject(new Error('Could not create protected setup state')); else resolve();
      });
    child.stdin.on('error', () => {});
    child.stdin.end(contents);
  });
}
