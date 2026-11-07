// Native Windows acceptance: builds the OpenSSL seed library and stages the pinned sandbox runtimes
// beside test programs, through the same packaging code as a release. Kept out of the PowerShell
// acceptance scripts, whose size an elevated command line bounds.
//   node scripts/windows-native-runtimes.mjs seed <rust target>           prints the library's path
//   node scripts/windows-native-runtimes.mjs stage <arch> <seed library> <program>...
//   node scripts/windows-native-runtimes.mjs git <arch> <backend>
// `stage` writes shell/, conpty/, node/ and python/ (MiniClaw's sitecustomize) beside each program. The Node archive is read from
// MINICLAW_NODE_ARCHIVE when set (checked against its pin either way), otherwise downloaded.
// `git` installs the private Git into MINICLAW_DATA_DIR from MINICLAW_TEST_GIT_ARCHIVE, checked
// against the bundled pin when the file has the bundle's name and against the official one otherwise.
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { shellFiles } from './setup/windows-shell.mjs';
import { conptyFiles } from './setup/windows-conpty.mjs';
import { sandboxNodeFiles } from './setup/windows-node-sandbox.mjs';
import { installWindowsGit, windowsGitPackage } from './setup/windows-git.mjs';
import { windowsGitBundle } from './setup/windows-git-bundle.mjs';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [command, ...rest] = process.argv.slice(2);

if (command === 'seed') {
  // Its own workspace and target folder, as scripts/package-backend.mjs builds it for a release.
  const [target] = rest;
  const output = execFileSync('cargo', ['build', '--release', '--locked', '--target', target, '--message-format=json',
    '--manifest-path', join(repository, 'openssl-seed', 'Cargo.toml'), '--target-dir', join(repository, 'target', 'openssl-seed')],
  { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 64 * 1024 * 1024 });
  const libraries = output.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line))
    .filter(record => record.reason === 'compiler-artifact' && record.target?.name === 'miniclaw_openssl_seed')
    .flatMap(record => record.filenames).filter(name => name.toLowerCase().endsWith('.dll'));
  if (libraries.length !== 1) throw new Error('The OpenSSL seed library build did not name exactly one library');
  console.log(libraries[0]);
} else if (command === 'stage') {
  const [arch, seedLibrary, ...programs] = rest;
  if (!programs.length) throw new Error('Name the programs to stage the runtimes beside');
  const shell = await shellFiles({ arch });
  const sets = [['shell', shell], ['conpty', await conptyFiles({ arch })],
    ['node', await sandboxNodeFiles({ arch, seedLibrary, shell: shell.get('busybox.exe') })],
    ['python', new Map([['sitecustomize.py', await readFile(join(repository, 'scripts', 'setup', 'python-sandbox', 'sitecustomize.py'))]])]];
  for (const program of programs) for (const [folder, files] of sets) {
    const directory = join(dirname(program), folder);
    await mkdir(directory, { recursive: true });
    // npm's tree goes below the private Node as node_modules/npm/...
    for (const [name, bytes] of files) {
      await mkdir(dirname(join(directory, name)), { recursive: true });
      await writeFile(join(directory, name), bytes);
    }
  }
  console.log(`Staged shell, ConPTY, the private Node and the Python hook beside ${programs.length} programs`);
} else if (command === 'git') {
  const [arch, backend] = rest;
  const archive = process.env.MINICLAW_TEST_GIT_ARCHIVE, bundled = windowsGitBundle(arch);
  if (!archive) throw new Error('Pinned MINICLAW_TEST_GIT_ARCHIVE is required');
  await installWindowsGit({ runtime: repository, data: process.env.MINICLAW_DATA_DIR, archive, arch, backend,
    pin: archive.replaceAll('\\', '/').endsWith(`/${bundled.file}`) ? bundled : windowsGitPackage(arch) });
} else {
  throw new Error('Usage: windows-native-runtimes.mjs seed <target> | stage <arch> <seed library> <program>... | git <arch> <backend>');
}
