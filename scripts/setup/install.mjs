// Claims the installation folder with platform-appropriate private permissions.
import { adoptKept, claimDestination, keptMarker } from './destination.mjs';
// Names the program files an uninstall removes from a kept data folder; a failed reinstall removes them again.
import { programData } from './uninstall-windows.mjs';
// Installs a private Git runtime from the reviewed package.
import { installWindowsGit } from './windows-git.mjs';
// Locates and pins the Git runtime carried inside the release bundle.
import { bundledGitArchive, windowsGitBundle } from './windows-git-bundle.mjs';
import { writePrivateFile } from './private-file.mjs';
// Revalidates the selected Windows helper before saving its path.
import { inspectWindowsComputerHelper } from './windows-computer.mjs';
// Saves the helper choice in the form the launcher reads.
import { windowsHelperSetting } from '../start-host.mjs';
// Installs Microsoft's Visual C++ runtime, which the Windows backend and Git need, with administrator approval.
import { installVcRuntime } from './vc-runtime.mjs';
// Installs Windows network restrictions through administrator approval.
import { installWindowsNetwork } from './windows-network.mjs';
// Writes only newly claimed installation directories.
import { copyFile, lstat, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
// Validates destinations and locates packaged output.
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
// Locates the source builder independently of cwd.
import { fileURLToPath } from 'node:url';
// Downloads and verifies only approved images.
import { downloadImage } from './download.mjs';
// Prepares the opt-in VM for a Mac backend.
import { prepareMacVm } from '../vm/prepare-mac.mjs';
// Rechecks capacity and runs owned builds.
import { checkPort as portCheck, run, volume } from './system.mjs';
// Uses a release's prebuilt files instead of compiling.
import { prebuilt } from './prebuilt.mjs';
// Rechecks the reviewed helper before copying.
import { inspectComputerApp } from '../vm/computer-source.mjs';

// Copies a release's prebuilt Mac bundle, each file checked against the release's manifest.
export async function copyBundle(path, from = prebuilt) {
  const names = Object.keys(JSON.parse(await readFile(join(from.folder, 'manifest.json'), 'utf8')).files)
    .filter(name => name.startsWith('bundle/'));
  if (!names.includes(`bundle/bin/miniclaw-server${process.platform === 'win32' ? '.exe' : ''}`)) throw new Error('This release has no backend.');
  for (const name of names) {
    const target = join(path, name.slice('bundle/'.length));
    await mkdir(dirname(target), { recursive: true, mode: 0o755 });
    await copyFile(await from.file(name), target);
  }
}

// Builds a relocatable Mac bundle without launching the application. A release copies its
// prebuilt bundle instead, so the Mac needs no Rust or Node packages.
async function build(path, signal, onOutput) {
  if (await prebuilt.available()) { onOutput?.('Using the prebuilt backend of this release.\n'); return copyBundle(path); }
  const root = fileURLToPath(new URL('../../', import.meta.url));
  await run('cargo', ['fetch', '--locked'], { cwd: root, signal, onOutput });
  await run(process.execPath, [join(root, 'scripts/package-backend.mjs'), '--host', path],
    { cwd: root, signal, onOutput });
}

// Copies the VM session and everything it imports into `runtime/`, keeping the
// relative layout so the installed launcher resolves the same paths as the checkout.
export async function copyVmRuntime(runtime, scripts = fileURLToPath(new URL('../', import.meta.url))) {
  const pending = ['vm/session.mjs', 'vm/readiness.mjs'];
  const copied = new Set();
  while (pending.length) {
    const path = pending.pop();
    if (copied.has(path)) continue;
    copied.add(path);
    const text = await readFile(join(scripts, path), 'utf8');
    for (const match of text.matchAll(/^(?:import|export)\b[^'"]*?from\s+['"](\.{1,2}\/[^'"]+)['"]/gm)) {
      pending.push(relative(scripts, resolve(scripts, dirname(path), match[1])));
    }
    await mkdir(dirname(join(runtime, path)), { recursive: true, mode: 0o700 });
    await copyFile(join(scripts, path), join(runtime, path));
  }
}

// Checks whether two directories overlap, so rollback cannot erase resumable downloads.
function overlaps(left, right) {
  const path = relative(resolve(left), resolve(right));
  return path === '' || (!isAbsolute(path) && path.split(sep)[0] !== '..');
}

// Installs the Mac runtime and, when chosen, prepares the opt-in VM. VM readiness
// waits for guest setup and sign-in, which the wizard runs afterwards.
export async function install({ plan, destination, cache, codexBin, claudeBin, approved, signal,
  installGit = installWindowsGit,
  installNetwork = installWindowsNetwork,
  installRuntime = installVcRuntime,
  claim = claimDestination,
  // A folder `miniclaw uninstall` left with the user's data (destination.mjs `keptData`), installed into
  // again: it is re-checked instead of claimed, and a failure leaves it as uninstall left it.
  keep = null, adopt = adoptKept,
  secureData = (runtime, data) => run(join(runtime, 'bin/miniclaw-server.exe'),
    ['--secure-data-dir', data], { signal, onOutput }),
  buildHost = build, download = downloadImage, checkPort = portCheck,
  prepareVm = prepareMacVm,
  inspectComputer = inspectComputerApp, inspectWindowsComputer = inspectWindowsComputerHelper,
  freeBytes = async path => (await volume(path)).freeBytes,
  onProgress = () => {}, onStage = () => {}, onOutput = () => {},
}) {
  if (approved !== true) throw new Error('Installation needs explicit approval');
  if (!isAbsolute(destination) || !isAbsolute(cache) || overlaps(destination, cache) || overlaps(cache, destination)) {
    throw new Error('Installation and cache need separate absolute directories');
  }
  // Only enabled providers are wired in, and each needs a real executable.
  const binaries = { codex: plan.providers.codex ? codexBin : null,
    claude: plan.providers.claude ? claudeBin : null };
  for (const [id, name] of [['codex', 'Codex'], ['claude', 'Claude Code']]) {
    if (plan.providers[id] && (typeof binaries[id] !== 'string' || !isAbsolute(binaries[id]))) {
      throw new Error(`${name} is enabled but its executable was not found`);
    }
  }
  signal?.throwIfAborted();
  if (plan.computer) {
    const source = await inspectComputer(plan.computer.path, { signal });
    if (source.bytes !== plan.computer.bytes) {
      throw new Error('Computer Use source changed; review the installation again');
    }
  }
  if (plan.windowsComputerHelper) {
    const source = await inspectWindowsComputer(plan.windowsComputerHelper);
    if (source.bytes !== plan.windowsComputer?.bytes || source.path !== plan.windowsComputerHelper) {
      throw new Error('Windows Computer Use source changed; review the installation again');
    }
  }
  await checkPort(plan.port);
  if (await freeBytes(destination) < plan.peakBytes) throw new Error('Not enough free disk space');
  const dependencies = [...plan.images, ...plan.packages];
  if (dependencies.length && await freeBytes(cache) < plan.retainedImageBytes + plan.packageDownloadBytes) {
    throw new Error('Not enough free disk space in download cache');
  }
  // Windows ignores mode bits: the claim creates a protected folder there and refuses a
  // location another account could replace, before any backend is built into it.
  if (keep) await adopt(destination); else await claim(destination);
  let windowsNetwork = null;
  // The earlier provider choice in a kept data folder, set aside until this installation succeeds.
  const providersPath = join(destination, 'data/providers.json'), earlierProviders = `${providersPath}.before-setup`;
  let providersMoved = false;
  try {
    const downloads = [];
    if (dependencies.length) {
      await mkdir(cache, { recursive: true, mode: 0o700 });
      for (const image of dependencies) {
        onStage('download', image);
        downloads.push(await download(image, cache, { signal,
          onProgress: progress => onProgress(image, progress) }));
      }
    }
    // The backend links vcruntime140.dll and setup runs it below, so Microsoft's runtime comes first.
    if (plan.platform === 'win32' && plan.vcRuntime) {
      // The runtime must match the backend this plan installs (the release architecture).
      if (plan.vcRuntime.arch !== plan.arch) throw new Error('The Microsoft Visual C++ runtime download is for another architecture');
      onStage('vc-runtime');
      const { restart } = await installRuntime({ arch: plan.arch, installer: plan.vcRuntime.installer, sha256: plan.vcRuntime.sha256 });
      if (restart) onOutput('The Microsoft Visual C++ runtime is installed; Windows recommends a restart to complete it.\n');
    }
    onStage('build');
    const runtime = join(destination, 'runtime');
    await buildHost(runtime, signal, onOutput);
    // The backend reads this on first start, so the wizard's choice takes effect.
    if (plan.platform === 'win32') await secureData(runtime, join(destination, 'data'));
    else await mkdir(join(destination, 'data'), { mode: 0o700 });
    if (keep && await lstat(providersPath).then(() => true, () => false)) { await rename(providersPath, earlierProviders); providersMoved = true; }
    await writePrivateFile(runtime, providersPath, `${JSON.stringify(plan.providers)}\n`);
    if (plan.platform === 'win32' && plan.windowsGit) {
      // The bundle copied above carries the pinned archive; installWindowsGit verifies it again.
      onStage('windows-git');
      await installGit({ runtime, data: join(destination, 'data'), arch: plan.arch,
        archive: bundledGitArchive(runtime, plan.arch), pin: windowsGitBundle(plan.arch) });
    }
    let pair = null;
    let masterSetup = null;
    if (plan.master) {
      pair = await prepareVm({ destination, plan, downloads, runtime, signal, onStage, onOutput });
      if (prepareVm === prepareMacVm) await copyVmRuntime(runtime);
      masterSetup = { image: pair.master?.media, status: 'worker-prepared', oauthReady: false,
        computerUse: Boolean(plan.computer) };
    }
    signal?.throwIfAborted();
    const settings = { schemaVersion: 1, port: plan.port, backend: 'host',
      providers: plan.providers, codexBin: binaries.codex, claudeBin: binaries.claude,
      // The consent is saved by the Codex app's package family, so a Codex update keeps it working.
      windowsComputerHelper: windowsHelperSetting(plan.windowsComputerHelper ?? null),
      windowsNetwork,
      vmEnabled: plan.vmEnabled, master: plan.master, limits: plan.limits,
      hostReady: true, vmReady: false,
      status: pair ? 'awaiting-guest-setup' : 'installed',
      downloads, masterSetup, pair, createdAt: new Date().toISOString() };
    if (plan.platform === 'win32') {
      await writePrivateFile(runtime, join(destination, 'data/windows-setup-pending.json'),
        `${JSON.stringify({ ...settings, hostReady: false, status: 'awaiting-network-service' })}\n`);
      onStage('windows-network');
      windowsNetwork = await installNetwork({ runtime, data: join(destination, 'data'), onOutput });
      settings.windowsNetwork = windowsNetwork;
    }
    await writeFile(join(destination, 'settings.json'), `${JSON.stringify(settings, null, 2)}\n`,
      { flag: 'wx', mode: 0o600 });
    if (plan.platform === 'win32') {
      await rm(join(destination, 'data/windows-setup-pending.json'), { force: true });
      await rm(join(destination, 'data/windows-network-pending.json'), { force: true });
    }
    if (keep) {
      await rm(earlierProviders, { force: true });
      await rm(join(destination, 'data', keptMarker), { force: true });
    }
    return settings;
  } catch (error) {
    windowsNetwork ??= error.windowsNetwork;
    if (windowsNetwork) {
      throw Object.assign(new Error(`Network service ${windowsNetwork.serviceName} may be installed; files were retained at ${destination}. Run miniclaw resume "${destination}". ${error.message}`, { cause: error }),
        { retained: destination });
    }
    // The directory was created above; existing installs and the image cache stay intact.
    if (keep) {
      // Back to what uninstall left: only `data`, with the user's state, its earlier choices and the marker.
      for (const name of ['runtime', 'settings.json', 'settings.ready.json']) await rm(join(destination, name), { recursive: true, force: true, maxRetries: 3 });
      for (const name of programData) await rm(join(destination, 'data', name), { recursive: true, force: true, maxRetries: 3 });
      if (providersMoved) { await rm(providersPath, { force: true }); await rename(earlierProviders, providersPath); }
      throw Object.assign(error, { keptData: destination });
    }
    await rm(destination, { recursive: true, force: true });
    throw error;
  }
}
