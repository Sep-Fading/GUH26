#!/usr/bin/env node
// Refreshes an installation's backend, page and VM scripts from this checkout, in place.
// Builds the fresh bundle in a child process.
import { execFile } from 'node:child_process';
// Copies the bundle and swaps files in.
import { chmod, copyFile, cp, lstat, mkdir, mkdtemp, readFile, readdir, rename, rm, rmdir } from 'node:fs/promises';
// Resolves installation, bundle and source paths.
import { dirname, join, resolve } from 'node:path';
// Locates this checkout's scripts folder.
import { fileURLToPath } from 'node:url';
// Awaits the build without a shell.
import { promisify } from 'node:util';
// Remembers where MiniClaw was installed.
import { defaultInstallation } from './default-installation.mjs';
// Copies the VM session and its imports.
import { copyBundle, copyVmRuntime } from './install.mjs';
// Tells a release from a checkout.
import { prebuilt } from './prebuilt.mjs';
// Names the project VM files kept in `runtime/`.
import { supportFiles } from '../vm/dedicated-setup.mjs';
// Updates the separately protected Windows service through administrator approval.
import { backendMismatch, updateWindowsNetwork } from './windows-network.mjs';
// Reads an older installation's architecture from its backend.
import { peArch } from './pe-machine.mjs';
// Keeps executable and corresponding source together.
import { shellNames } from './windows-shell.mjs';
// Switches the private Git runtime to the bundle's pinned build inside the same recovery group.
import { gitBundleFolder, replaceWindowsGit, windowsGitBundle } from './windows-git-bundle.mjs';
// Installs Microsoft's Visual C++ runtime, which the new backend and Git need, before they are switched in.
import { ensureVcRuntime } from './vc-runtime.mjs';

const exec = promisify(execFile);
const checkout = fileURLToPath(new URL('../', import.meta.url));

// Reports whether a path exists without following a link.
const exists = path => lstat(path).then(() => true, () => false);

// Writes one file beside its target and renames it over, so a reader never sees half a file.
// Without a mode the source's own permissions are kept.
async function swapFile(source, target, mode) {
  await copyFile(source, `${target}.new`);
  if (mode !== undefined) await chmod(`${target}.new`, mode);
  try { await rename(`${target}.new`, target); }
  catch (error) {
    await rm(`${target}.new`, { force: true });
    if (process.platform === 'win32' && /\.(exe|dll)$/i.test(target)) {
      throw new Error(`Stop MiniClaw before replacing its Windows executable: ${error.code}`);
    }
    throw error;
  }
}

// Restores earlier native files if any replacement or the dependent `after` step fails;
// retains backups if recovery fails. Backups are removed only once `after` has succeeded.
async function replaceNative(runtime, bundle, names, replace, after = async () => {}) {
  const backup = await mkdtemp(join(runtime, '.native-update-'));
  const changed = [];
  const created = [];
  let retain = false;
  try {
    for (const [index, name] of names.entries()) {
      const target = join(runtime, name), saved = join(backup, String(index));
      const present = await exists(target);
      if (present) await copyFile(target, saved);
      if (!await exists(dirname(target))) created.push(dirname(target));
      await mkdir(dirname(target), { recursive: true });
      await replace(join(bundle, name), target, 0o755);
      changed.push({ target, saved, present });
    }
    await after();
  } catch (error) {
    const failures = [];
    for (const item of changed.reverse()) {
      try {
        if (item.present) await swapFile(item.saved, item.target, 0o755);
        else await rm(item.target, { force: true });
      } catch (failure) { failures.push(failure); }
    }
    // Folders this update introduced go away once empty; anything else in them is kept.
    for (const folder of created.reverse()) await rmdir(folder).catch(() => {});
    if (failures.length) {
      retain = true;
      throw Object.assign(new AggregateError([error, ...failures], `Runtime update failed; recovery files retained at ${backup}`), { recoveryFailed: true });
    }
    throw error;
  } finally { if (!retain) await rm(backup, { recursive: true, force: true }); }
}

// One line of why a step failed: a command's own last line of error output, else the error's message.
function reason(error) {
  if (error?.code === backendMismatch) return "the new release's backend does not match its manifest, so the release folder is damaged or was changed";
  const output = String(error?.stderr ?? '').trim().split(/\r?\n/).filter(Boolean).at(-1);
  return output || String(error?.message ?? error);
}

// Words a failed native update. Undone completely, the installation is exactly as before, so the user is
// told that, why it failed and what to try; reinstalling is the advice only when the undo itself failed.
export function updateFailure(error, version) {
  if (error?.recoveryFailed) {
    const parts = (error.errors ?? [error]).map(reason).join('; ');
    return Object.assign(new Error(`The update failed and could not be fully undone: ${parts}. ${error.message}.\n`
      + 'Reinstall MiniClaw: run miniclaw uninstall (your data is kept), then miniclaw setup from the new release.', { cause: error }));
  }
  const why = reason(error);
  const hint = error?.code === backendMismatch
    ? 'Extract the downloaded release archive again into a new folder and run miniclaw.cmd update from there.'
    : 'Fix the cause above if you can, then run miniclaw update again. If it fails the same way, extract the release archive again into a new folder and update from there.';
  return Object.assign(new Error(`The update did not finish, and every change was undone: this installation is unchanged`
    + `${version ? ` (MiniClaw ${version})` : ''} and starts as before.\nWhat failed: ${why}.\nWhat to try: ${hint}`, { cause: error }),
  { rolledBack: true });
}

// Names an installed Windows runtime's architecture: its manifest's, or else its backend's PE header.
async function installedArch(runtime, binary) {
  const manifest = JSON.parse(await readFile(join(runtime, 'manifest.json'), 'utf8'));
  if (manifest.arch !== undefined) return manifest.arch;
  try { return peArch(await readFile(join(runtime, binary))); } catch { return null; }
}

// Replaces the installed backend, page, launcher and VM scripts with a freshly built bundle.
// Native replacements keep temporary recovery copies until their group succeeds. The worker
// and VM setup disk are left to `miniclaw update-vm`, which needs the VM stopped.
export async function refreshRuntime({ destination, bundle, scripts = checkout,
  platform = process.platform, updateNetwork = updateWindowsNetwork, updateGit = replaceWindowsGit, replace = swapFile,
  ensureRuntime = ensureVcRuntime }) {
  const runtime = join(destination, 'runtime');
  const binary = `bin/miniclaw-server${platform === 'win32' ? '.exe' : ''}`;
  if (!await exists(join(runtime, 'manifest.json')) || !await exists(join(runtime, binary))) {
    throw new Error(`${destination} is not a MiniClaw installation`);
  }
  const terminal = platform === 'win32' ? ['bin/conpty/conpty.dll', 'bin/conpty/OpenConsole.exe', 'bin/conpty/LICENSE.txt', ...shellNames.map(name => `bin/shell/${name}`)] : [];
  for (const name of [binary, ...terminal, 'web/index.html', 'start-host.mjs', 'manifest.json']) {
    if (!await exists(join(bundle, name))) throw new Error(`The new build is incomplete: ${name} is missing`);
  }
  // The bundle's own manifest names its architecture, which selects the pinned Git archive. It must
  // be this installation's architecture, for a release and for a checkout build alike.
  let git = [], arch = null, others = [];
  if (platform === 'win32') {
    const bundleManifest = JSON.parse(await readFile(join(bundle, 'manifest.json'), 'utf8'));
    arch = bundleManifest.arch;
    const archive = `${gitBundleFolder}/${windowsGitBundle(arch).file}`;
    if (!await exists(join(bundle, archive))) throw new Error(`The new build is incomplete: ${archive} (Git) is missing`);
    const licenses = await readdir(join(bundle, gitBundleFolder, 'licenses')).catch(() => []);
    git = [archive, ...licenses.sort().map(name => `${gitBundleFolder}/licenses/${name}`)];
    // Every other native file the new bundle records (the private Node with its pinned seed library and npm's
    // tree, Python's hook) is replaced in the same recovery group: the new backend checks them by hash.
    const handled = new Set([binary, ...terminal, ...git]);
    others = Object.keys(bundleManifest.files ?? {}).filter(name => name.startsWith('bin/') && !handled.has(name)).sort();
    for (const name of others) if (!await exists(join(bundle, name))) throw new Error(`The new build is incomplete: ${name} is missing`);
    const installed = await installedArch(runtime, binary);
    if (installed !== arch) {
      throw new Error(`This installation's architecture is ${installed ?? 'unknown'}, and the new build is ${arch ?? 'unknown'}; nothing was changed.`);
    }
    // Nothing is replaced unless Microsoft's runtime is present (or was just installed with approval).
    await ensureRuntime({ arch, onOutput: text => process.stdout.write(text) });
  }
  // Refuse a running backend first, then replace its terminal, shell and Git dependencies as one
  // recoverable group. The private Git record switches to a runtime installed from the new archive,
  // then the protected service is pinned to the new backend; a failure in either restores the earlier
  // backend, terminal, shell and Git so they never run beside an old service.
  let gitSwitch = null;
  const installedVersion = await readFile(join(runtime, 'manifest.json'), 'utf8').then(text => JSON.parse(text).version, () => undefined);
  await replaceNative(runtime, bundle, [binary, ...terminal, ...git, ...others], replace, async () => {
    if (platform !== 'win32') return;
    const data = join(destination, 'data');
    try {
      gitSwitch = await updateGit({ runtime, data, arch });
      // The installed manifest still names the old backend here; the service pins the new one.
      await updateNetwork({ runtime, data, manifest: join(bundle, 'manifest.json'), onOutput: text => process.stdout.write(text) });
    } catch (error) {
      if (!gitSwitch) throw error;
      try { await gitSwitch.rollback(); } catch (failure) {
        throw Object.assign(new AggregateError([error, failure], `Git runtime rollback failed: ${failure.message}`), { recoveryFailed: true });
      }
      throw error;
    }
  }).catch(error => { throw platform === 'win32' ? updateFailure(error, installedVersion) : error; });
  // Only now is the earlier Git runtime no longer needed.
  await gitSwitch?.commit();
  // The page's asset names change with every build, so the folder is replaced whole.
  await rm(join(runtime, 'web.new'), { recursive: true, force: true });
  await cp(join(bundle, 'web'), join(runtime, 'web.new'), { recursive: true });
  await rm(join(runtime, 'web'), { recursive: true, force: true });
  await rename(join(runtime, 'web.new'), join(runtime, 'web'));
  await swapFile(join(bundle, 'start-host.mjs'), join(runtime, 'start-host.mjs'), 0o755);

  const vm = await exists(join(runtime, 'vm'));
  if (vm) {
    await copyVmRuntime(runtime, scripts);
    for (const [directory, names] of supportFiles) {
      const from = join(scripts, directory === 'runtime' ? 'vm' : 'setup');
      await mkdir(join(destination, directory), { recursive: true, mode: 0o700 });
      for (const name of names) await swapFile(join(from, name), join(destination, directory, name));
    }
  }
  // The manifest goes last, so its version only changes once everything else is in place.
  await swapFile(join(bundle, 'manifest.json'), join(runtime, 'manifest.json'), 0o644);
  return { vm };
}

// Builds a bundle inside the installation's volume, swaps it in and removes the build folder.
async function main() {
  const [path] = process.argv.slice(2);
  if (path?.startsWith('-')) throw new Error('Usage: miniclaw update [INSTALLATION]');
  const destination = resolve(path ?? await defaultInstallation());
  const bundle = join(destination, `.update-${process.pid}`);
  const release = await prebuilt.available();
  // Said first: without a path the remembered installation is the one updated.
  if (process.platform === 'win32') console.log(`Updating the MiniClaw installation at ${destination}${path ? '' : ' (the remembered one; name another with: miniclaw update PATH)'}.`);
  console.log(release ? 'Installing the backend and the page of this release...'
    : 'Building the backend and the page. This takes a few minutes...');
  try {
    // A release copies its checked, prebuilt bundle; a checkout builds one.
    if (release) await copyBundle(bundle);
    else await exec(process.execPath, [join(checkout, 'package-backend.mjs'), bundle],
      { cwd: dirname(checkout), maxBuffer: 1 << 26 });
    const { vm } = await refreshRuntime({ destination, bundle });
    console.log(`Updated ${destination}.`);
    console.log('Restart miniclaw to load the new backend.');
    if (vm) console.log('If the worker or the guest installer changed, also run: miniclaw update-vm');
  } finally {
    await rm(bundle, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.stderr || error.message); process.exitCode = 1; });
}
