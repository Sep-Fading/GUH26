// The Computer Use helper path the computer suites configure.
import { accessSync, copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { command } from './harness.mjs';

// Fake Codex answers for Computer Use in these suites, so the helper never runs; the backend only
// checks that one is configured. The Mac accepts any executable, so the suites name /usr/bin/true.
// Windows accepts only an absolute path to a file named like the installed helper, so there the
// suites copy the stand-in built by `cargo build --example computer_helper_standin` under that
// name, into a folder of their own that is removed when the suite's process ends.
export function standInHelper() {
  if (process.platform !== 'win32') return '/usr/bin/true';
  const built = join(dirname(command[0]), 'examples', 'computer_helper_standin.exe');
  try { accessSync(built); }
  catch { throw new Error(`The Computer Use stand-in is not built: ${built}. Run cargo build --example computer_helper_standin.`); }
  const folder = mkdtempSync(join(tmpdir(), 'mc-helper-'));
  const helper = join(folder, process.arch === 'arm64' ? 'codex-computer-use-arm64.exe' : 'codex-computer-use.exe');
  copyFileSync(built, helper);
  process.on('exit', () => rmSync(folder, { recursive: true, force: true }));
  return helper;
}
