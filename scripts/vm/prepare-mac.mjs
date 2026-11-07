// Creates private pairing material without credentials.
import { randomBytes } from 'node:crypto';
// Creates owned build paths and runtime files.
import { access, chmod, copyFile, mkdir, rm, writeFile } from 'node:fs/promises';
// Resolves build inputs and retained guest paths.
import { join } from 'node:path';
// Locates trusted sources independently of working directory.
import { fileURLToPath } from 'node:url';
// Runs pinned builds without administrator privileges.
import { run } from '../setup/system.mjs';
// Uses a release's prebuilt files instead of compiling.
import { prebuilt } from '../setup/prebuilt.mjs';
// Rechecks downloaded images before extracting payloads.
import { verifyDownload } from '../setup/download.mjs';
// Packages signed Codex and the guest worker.
import { prepareMasterMedia } from './master-media.mjs';
// Packages fresh-install tools for later project VMs.
import { packageDedicatedSupport } from './dedicated-setup.mjs';
// Applies the wizard's approved global percentages.
import { configureResourceLimits } from './resource-admission.mjs';

const source = fileURLToPath(new URL('../../', import.meta.url));

// Prepares the opt-in macOS VM for a backend that runs on the Mac. There is no Linux
// controller and no separate Mac worker: the backend reads `worker.json` and reaches
// the guest worker through the session's private sockets. Boot and sign-in happen later.
export async function prepareMacVm({ destination, plan, downloads, signal,
  onStage = () => {}, onOutput = () => {},
}) {
  if (!plan.master) throw new Error('VM setup needs a Master allocation');
  configureResourceLimits(plan.limits);
  const options = { cwd: source, signal, onOutput };
  const dependencies = [...plan.images, ...plan.packages];
  // Resolves only the approved, checksum-verified download with this identifier.
  async function downloaded(id) {
    const index = dependencies.findIndex(item => item.id === id);
    if (index < 0 || !downloads[index]?.path) throw new Error(`Missing ${id} download`);
    await verifyDownload(downloads[index].path, dependencies[index], signal);
    return downloads[index].path;
  }
  const restoreImage = await downloaded('macos');
  const codexArchive = await downloaded('codex');
  // A release brings the worker, the network helper and the VM supervisor already built.
  const ready = await prebuilt.available();
  const networkArchive = ready ? null : await downloaded('network-go');
  onStage('build-worker');
  const workerBinary = await prebuilt.file('miniclaw-worker') ?? join(source, 'target/release/miniclaw-worker');
  if (!ready) await run('cargo', ['build', '--release', '--locked', '--offline', '--bin', 'miniclaw-worker',
    '--target-dir', join(source, 'target')], options);
  // The same key goes onto the guest's setup disk and into the Mac's private settings.
  const worker = { key: randomBytes(32).toString('hex'), cwd: '/Users/Shared/MiniClaw/Workspace' };
  const workerSettings = join(destination, 'worker.json');
  await writeFile(workerSettings, `${JSON.stringify(worker)}\n`, { flag: 'wx', mode: 0o600 });
  onStage('master-media');
  const { image: media } = await prepareMasterMedia({ output: join(destination, 'master-setup'),
    archive: codexArchive, computerApp: plan.computer?.path,
    worker: { binary: workerBinary, settings: worker },
    signal, onOutput });
  // Quick builds run first, so nothing can fail after the long macOS install.
  onStage('build-network');
  const networkTools = join(destination, 'network-tools');
  await mkdir(networkTools, { mode: 0o700 });
  if (!ready) await run('/usr/bin/tar', ['-xzf', networkArchive, '-C', networkTools], options);
  await mkdir(join(destination, 'runtime'), { recursive: true, mode: 0o700 });
  const network = join(destination, 'runtime', 'miniclaw-network');
  if (ready) { await copyFile(await prebuilt.file('miniclaw-network'), network); await chmod(network, 0o700); }
  else await run('/usr/bin/env', [`GOPATH=${join(networkTools, 'modules')}`,
    `GOCACHE=${join(networkTools, 'cache')}`, 'GOTOOLCHAIN=local',
    join(networkTools, 'go/bin/go'), 'build', '-mod=readonly', '-trimpath', '-o', network, '.'],
  { ...options, cwd: join(source, 'scripts/vm/network-helper') });
  const dedicatedWorker = await packageDedicatedSupport(destination, join(source, 'scripts'), workerBinary);
  onStage('install-macos');
  const masterRoot = join(destination, 'master');
  const app = join(masterRoot, 'MiniClaw Master.app/Contents');
  await mkdir(join(app, 'MacOS'), { recursive: true, mode: 0o700 });
  await writeFile(join(app, 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>master-vm</string>
<key>CFBundleIdentifier</key><string>dev.miniclaw.master</string>
<key>CFBundleName</key><string>MiniClaw Master</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>\n`, { flag: 'wx', mode: 0o600 });
  const binary = join(app, 'MacOS/master-vm');
  const swiftArgs = ['swiftc', '-swift-version', '6', '-warnings-as-errors', '-parse-as-library',
    '-module-cache-path', join(masterRoot, 'compiler-cache')];
  const olderSDK = '/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk';
  try { await access(olderSDK); swiftArgs.push('-sdk', olderSDK); } catch { /* Use selected SDK. */ }
  if (ready) { await copyFile(await prebuilt.file('master-vm'), binary); await chmod(binary, 0o700); }
  else await run('/usr/bin/xcrun', [...swiftArgs,
    ...['experiments/vm-lab.swift', 'experiments/vm-window.swift', 'experiments/vm-headless.swift',
      'vm/host-relay.swift', 'vm/guest-bridge.swift', 'vm/guest-network.swift']
      .map(path => join(source, 'scripts', path)), '-o', binary], options);
  await run('/usr/bin/codesign', ['--sign', '-', '--entitlements',
    join(source, 'scripts/experiments/virtualization.entitlements'), binary], options);
  const guest = join(masterRoot, 'Guest');
  await run(binary, ['init', guest, restoreImage, String(plan.master.cpuCount),
    String(plan.master.memoryGiB), String(plan.master.diskGiB)], options);
  await run(binary, ['install', guest, restoreImage], options);
  await rm(join(masterRoot, 'compiler-cache'), { recursive: true, force: true });
  return { root: destination, port: plan.port, mac: { worker: workerSettings },
    master: { binary, guest, media }, network,
    provisioning: { restoreImage, computerApp: plan.computer?.path, restoreDescriptor: plan.images.find(image => image.id === 'macos'),
      codexArchive, codexDescriptor: plan.packages.find(item => item.id === 'codex'),
      workerBinary: dedicatedWorker,
      resources: { cpuCount: plan.master.cpuCount, memoryGiB: plan.master.memoryGiB,
        diskGiB: plan.master.diskGiB } } };
}
