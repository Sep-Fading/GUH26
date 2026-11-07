// Hashes packaged files for integrity checks.
import { createHash } from 'node:crypto';
// Reads inputs and writes owned packages.
import { chmod, lstat, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
// Resolves package destinations and asset paths.
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';

// Supplies the pinned Windows terminal runtime.
import { conptyFiles } from './setup/windows-conpty.mjs';
// Supplies the shell with its corresponding source.
import { shellFiles as windowsShellFiles, shellNames } from './setup/windows-shell.mjs';
// Reads the backend's architecture from its own header.
import { peArch } from './setup/pe-machine.mjs';
// Supplies MiniClaw's pinned Git build and its licence texts.
import { gitBundleFiles, gitBundleFolder, windowsGitBundle } from './setup/windows-git-bundle.mjs';
// Supplies the private Node that sandboxed Node commands run with.
import { sandboxNodeFiles, sandboxNodeNames, sandboxNpm } from './setup/windows-node-sandbox.mjs';

// Reads regular files without accepting symbolic links as package inputs.
async function regular(path) {
  const info = await lstat(path);
  if (!info.isFile()) throw new Error(`Expected a regular file: ${path}`);
  return readFile(path);
}

// Collects frontend files, rejecting links and special filesystem entries.
async function webFiles(root, prefix = '') {
  if (!(await lstat(root)).isDirectory()) {
    throw new Error(`Expected a regular directory: ${root}`);
  }
  const files = new Map();
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const source = join(root, entry.name);
    const path = prefix + entry.name;
    if (entry.isDirectory()) {
      for (const item of await webFiles(source, `${path}/`)) files.set(...item);
    } else {
      files.set(path, await regular(source));
    }
  }
  return files;
}

// Creates a new native host bundle; leaves existing destinations untouched.
// A Windows bundle's architecture is the built executable's, since Cargo follows the Rust
// host target and not the Node process that runs this script.
// Windows bundles also carry the Git runtime ZIP from MINICLAW_GIT_ARCHIVE or the pinned release asset (docs/windows/git-runtime.md).
// Windows bundles also carry the private Node of bin/node, whose seed library `seedLibrary` names,
// and the sitecustomize for sandboxed Python in bin/python.
export async function assemble({ binary, assets, output, version, platform = process.platform, arch: requested, terminalFiles = conptyFiles, shellFiles = windowsShellFiles, gitFiles = gitBundleFiles,
  nodeFiles = sandboxNodeFiles, seedLibrary }) {
  const destination = resolve(output);
  const within = relative(resolve(assets), destination);
  if (!within || (!within.startsWith('..') && !isAbsolute(within))) {
    throw new Error('Package output must be outside frontend assets');
  }
  const executable = await regular(binary);
  const arch = platform === 'win32' ? peArch(executable) : requested ?? process.arch;
  if (requested && requested !== arch) {
    throw new Error(`The backend executable architecture is ${arch}, not the requested ${requested}`);
  }
  const files = new Map([[`bin/miniclaw-server${platform === 'win32' ? '.exe' : ''}`, executable]]);
  if (platform === 'win32') {
    const shell = await shellFiles({ arch });
    for (const name of shellNames) {
      const contents = shell.get(name);
      if (!contents?.length) throw new Error(`Missing shell runtime or source: ${name}`);
      files.set(`bin/shell/${name}`, contents);
    }
    const terminal = await terminalFiles({ arch });
    files.set('bin/conpty/LICENSE.txt', await regular(new URL('./setup/licenses/conpty-LICENSE.txt', import.meta.url)));
    for (const name of ['conpty.dll', 'OpenConsole.exe']) {
      const contents = terminal.get(name);
      if (!contents?.length) throw new Error(`Missing ConPTY runtime: ${name}`);
      files.set(`bin/conpty/${name}`, contents);
    }
    // Never ships without Git or with another architecture's build: the pinned archive must be present.
    const git = await gitFiles({ arch });
    if (!git.get(windowsGitBundle(arch).file)?.length) throw new Error(`Missing ${arch} Git runtime ${windowsGitBundle(arch).file}`);
    for (const [name, contents] of git) files.set(`${gitBundleFolder}/${name}`, contents);
    const node = await nodeFiles({ arch, seedLibrary, shell: shell.get('busybox.exe') });
    for (const name of sandboxNodeNames) {
      if (!node.get(name)?.length) throw new Error(`Missing private Node file: ${name}`);
    }
    // The private Node's files and npm's tree below them (`node_modules/npm/...`), nothing else.
    for (const [name, contents] of node) {
      if (!sandboxNodeNames.includes(name) && !name.startsWith(sandboxNpm.prefix)) throw new Error(`Unexpected private Node file: ${name}`);
      files.set(`bin/node/${name}`, contents);
    }
    // MiniClaw's sitecustomize, which lets asyncio run in the sandbox; the backend checks it byte for byte.
    files.set('bin/python/sitecustomize.py', await regular(new URL('./setup/python-sandbox/sitecustomize.py', import.meta.url)));
  }
  const web = await webFiles(assets);
  if (!web.has('index.html')) throw new Error('Frontend assets need index.html');
  for (const [path, contents] of web) files.set(`web/${path}`, contents);
  files.set('start-host.mjs', await regular(new URL('./start-host.mjs', import.meta.url)));
  const manifest = { schemaVersion: 1, version, role: 'host',
    platform, arch, files: {} };

  // Only remove a failed destination after this invocation created it.
  await mkdir(destination, { mode: 0o755 });
  try {
    for (const [path, contents] of files) {
      const target = join(destination, path);
      await mkdir(dirname(target), { recursive: true });
      await writeFile(target, contents, { flag: 'wx' });
      await chmod(target, path.startsWith('web/') ? 0o644 : 0o755);
      manifest.files[path] = createHash('sha256').update(contents).digest('hex');
    }
    await writeFile(join(destination, 'manifest.json'),
      `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx', mode: 0o644 });
    return manifest;
  } catch (error) {
    await rm(destination, { recursive: true, force: true });
    throw error;
  }
}
