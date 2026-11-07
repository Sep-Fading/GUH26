// Inspects helper files without following symlinks.
import { lstat, readdir } from 'node:fs/promises';
// Requires explicit paths and resolves app contents.
import { isAbsolute, join } from 'node:path';
// Verifies vendor signatures using native tooling.
import { run } from '../setup/system.mjs';

export const computerRequirement = 'anchor apple generic and certificate leaf[subject.OU] = "2DC432GLL2" '
  + 'and identifier "com.openai.sky.CUAService"';

// Accepts only the vendor-signed helper, without modifying its signature.
export async function verifyComputerApp(path, { signal } = {}) {
  try {
    await run('/usr/bin/codesign', ['--verify', '--deep', '--strict',
      `-R=${computerRequirement}`, path], { signal });
  } catch (error) {
    signal?.throwIfAborted();
    throw new Error('Computer Use signature verification failed', { cause: error });
  }
}

// Returns verified source size for planning; credentials outside the app are excluded.
// Path problems name the step-by-step guide, which the wizard prints before offering to change the choice.
export async function inspectComputerApp(path, { signal, verify = verifyComputerApp } = {}) {
  const pathError = message => Object.assign(new Error(`${message}\nExpected Codex Computer Use.app, usually in ~/.codex/computer-use. `
    + 'See: miniclaw guide computer-use'), { guide: 'computer-use' });
  if (typeof path !== 'string' || !isAbsolute(path)) throw pathError('Choose an absolute application path.');
  const info = await lstat(path).catch(() => { throw pathError(`Nothing at ${path}.`); });
  if (!info.isDirectory()) throw pathError('Expected an application directory, not a link');
  await verify(path, { signal });
  const pending = [path];
  let bytes = 0, entries = 0;
  while (pending.length) {
    signal?.throwIfAborted();
    const current = pending.pop();
    const info = await lstat(current);
    if (++entries > 20_000) throw new Error('Computer Use application contains too many files');
    if (info.isDirectory()) {
      for (const name of await readdir(current)) pending.push(join(current, name));
    } else if (info.isFile() || info.isSymbolicLink()) {
      bytes += info.size;
    } else throw new Error('Computer Use application contains an unsupported file');
    if (bytes > 1024 ** 3) throw new Error('Computer Use application exceeds 1 GiB');
  }
  if (bytes === 0) throw new Error('Computer Use application is empty');
  return { path, bytes };
}
