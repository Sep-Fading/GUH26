// Includes the bundled, pinned private Git runtime on Windows.
import { windowsGitBundle } from './windows-git-bundle.mjs';
// Rejects relative paths in reviewed helper sources.
import { isAbsolute, win32 } from 'node:path';

export const GiB = 1024 ** 3;
export const defaultLimits = { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 };

// Rejects unsupported hardware and invalid capacity measurements.
function checkHost(host) {
  if (!((host.platform === 'darwin' && host.arch === 'arm64')
    || (host.platform === 'win32' && ['x64', 'arm64'].includes(host.arch)))) {
    throw new Error('Setup supports Apple silicon Macs and Windows x64 or ARM64');
  }
  for (const key of ['cpuCount', 'memoryBytes', 'diskBytes', 'freeBytes']) {
    if (!Number.isSafeInteger(host[key]) || host[key] <= 0) throw new Error(`Invalid host ${key}`);
  }
}

// Suggests a Master size for this Mac. The VM is opt-in on every machine: normal chat
// and the Seatbelt tool need no VM, so the default installs none.
export function recommend(host) {
  checkHost(host);
  const memoryGiB = host.memoryBytes / GiB;
  const master = { cpuCount: 2, memoryGiB: memoryGiB >= 24 ? 8 : 4, diskGiB: 64 };
  const masterFits = host.platform === 'darwin' && master.memoryGiB <= memoryGiB * 0.5
    && master.cpuCount <= Math.floor(host.cpuCount * 0.5)
    && (master.diskGiB + 24) * GiB <= Math.min(host.freeBytes, host.diskBytes * 0.25);
  return { port: 8787, backend: 'host', master, masterFits, vmEnabled: false,
    limits: { ...defaultLimits } };
}

// Checks a Master allocation against the smallest supported macOS guest.
function allocation(value) {
  const minimum = { cpuCount: 2, memoryGiB: 4, diskGiB: 64 };
  for (const key of Object.keys(minimum)) {
    if (!Number.isSafeInteger(value?.[key]) || value[key] < minimum[key] || value[key] > 65536) {
      const label = key === 'memoryGiB' ? 'RAM' : key === 'diskGiB' ? 'disk' : 'CPU';
      throw new Error(`Master ${label} requires at least ${minimum[key]} ${key}`);
    }
  }
  return value;
}

// Returns a validated installation plan without downloads, writes or VM startup.
export function planSetup(input, host, catalog = []) {
  checkHost(host);
  if (!Number.isInteger(input.port) || input.port < 1024 || input.port > 65535) {
    throw new Error('Choose a port between 1024 and 65535');
  }
  if ((input.backend ?? 'host') !== 'host' || input.linux) {
    throw new Error('The backend always runs on the Mac');
  }
  if (!input.providers || ![input.providers.codex, input.providers.claude]
    .every(value => typeof value === 'boolean') || !(input.providers.codex || input.providers.claude)) {
    throw new Error('Enable at least one provider');
  }
  if (host.platform === 'win32' && (input.vmEnabled || input.master || input.computer || input.computerApp)) {
    throw new Error('Windows does not support macOS VM setup');
  }
  if (typeof input.vmEnabled !== 'boolean') throw new Error('Choose whether to add the VM');
  if (!input.vmEnabled && input.master) throw new Error('The VM is off; remove its allocation');
  if (input.vmEnabled && !input.master) throw new Error('Choose Master resources for the VM');
  if (input.windowsComputerHelper && (host.platform !== 'win32'
    || !win32.isAbsolute(input.windowsComputerHelper)
    || input.windowsComputer?.path !== input.windowsComputerHelper
    || !Number.isSafeInteger(input.windowsComputer?.bytes) || input.windowsComputer.bytes <= 0)) {
    throw new Error('Windows Computer Use source must be verified before review');
  }
  const computer = input.computer ?? null;
  if (input.computerApp && computer?.path !== input.computerApp) {
    throw new Error('Computer Use source must be verified before review');
  }
  if (computer && !input.master) throw new Error('Computer Use setup requires the VM');
  if (computer && (typeof computer.path !== 'string' || !isAbsolute(computer.path)
    || !Number.isSafeInteger(computer.bytes) || computer.bytes <= 0 || computer.bytes > GiB)) {
    throw new Error('Invalid verified Computer Use source');
  }
  const limits = input.limits ?? defaultLimits;
  for (const key of Object.keys(defaultLimits)) {
    if (!Number.isInteger(limits[key]) || limits[key] < 1 || limits[key] > 100) {
      throw new Error('Resource limits must be percentages from 1 to 100');
    }
  }
  const guests = [];
  if (input.master) guests.push({ id: 'macos', ...allocation(input.master) });
  const images = guests.map(guest => {
    const image = catalog.find(item => item.id === guest.id);
    if (!image || !Number.isSafeInteger(image.bytes) || image.bytes <= 0
      || !Number.isSafeInteger(image.expandedBytes) || image.expandedBytes < 0) {
      throw new Error(`Missing verified image sizes for ${guest.id}`);
    }
    if (guest.diskGiB * GiB <= image.expandedBytes) throw new Error(`${guest.id} disk cannot hold its image`);
    return image;
  });
  const cpuCount = guests.reduce((sum, guest) => sum + guest.cpuCount, 0);
  const memoryBytes = guests.reduce((sum, guest) => sum + guest.memoryGiB * GiB, 0);
  const vmDiskBytes = guests.reduce((sum, guest) => sum + guest.diskGiB * GiB, 0);
  // The guest needs Codex; its filtered network helper is built from pinned Go.
  // A release carries the network helper already built, so it needs no Go toolchain.
  // Windows Git travels inside the release (bin/git); it is installed from there, never downloaded.
  const windowsGit = host.platform === 'win32' ? windowsGitBundle(host.arch) : null;
  const packages = host.platform === 'win32' ? []
    : input.master ? (input.prebuilt ? [masterCodex] : [masterCodex, networkGo]) : [];
  const packageDownloadBytes = packages.reduce((sum, item) => sum + item.bytes, 0);
  const imageDownloadBytes = images.reduce((sum, image) => sum + image.bytes, 0);
  const downloadBytes = imageDownloadBytes + packageDownloadBytes;
  // Budget the copy, archive, media and temporary packing overhead.
  const retainedComputerBytes = computer ? 4 * computer.bytes + 16 * 1024 ** 2 : 0;
  const retainedPackageBytes = (packages.length ? 2 * packageDownloadBytes + 16 * 1024 ** 2 : 0)
    + retainedComputerBytes + packages.reduce((sum, item) => sum + (item.expandedBytes ?? 0), 0)
    + (windowsGit ? windowsGit.bytes + windowsGit.expandedBytes : 0);
  const retainedImageBytes = images.reduce((sum, image) => sum + image.bytes + image.expandedBytes, 0);
  // Reserve full sparse-disk capacity plus images; leave room for build/extraction work.
  const peakBytes = vmDiskBytes + retainedImageBytes + retainedPackageBytes + 2 * GiB;
  if (cpuCount > Math.floor(host.cpuCount * limits.cpuPercent / 100)) throw new Error('VM CPU budget exceeded');
  if (memoryBytes > host.memoryBytes * limits.memoryPercent / 100) {
    throw new Error(`VM RAM budget exceeded: ${(memoryBytes / GiB).toFixed(2)} GiB selected; `
      + `${(host.memoryBytes * limits.memoryPercent / 100 / GiB).toFixed(2)} GiB allowed`);
  }
  if (vmDiskBytes + retainedImageBytes + retainedPackageBytes > host.diskBytes * limits.diskPercent / 100) {
    throw new Error(`VM disk budget exceeded, including retained images: `
      + `${((vmDiskBytes + retainedImageBytes + retainedPackageBytes) / GiB).toFixed(2)} GiB selected; `
      + `${(host.diskBytes * limits.diskPercent / 100 / GiB).toFixed(2)} GiB allowed`);
  }
  if (peakBytes > host.freeBytes) throw new Error('Not enough free disk space for installation');
  return { ...input, platform: host.platform, arch: host.arch, backend: 'host', linux: null, computer, retainedComputerBytes, limits: { ...limits }, images, guests, cpuCount, memoryBytes,
    vmDiskBytes, downloadBytes, imageDownloadBytes, packages, packageDownloadBytes, windowsGit,
    retainedImageBytes, retainedPackageBytes, peakBytes,
    url: `http://127.0.0.1:${input.port}`, status: guests.length ? 'vm-preparation' : 'host' };
}
// Includes Codex only for selected workspaces.
import { masterCodex } from '../vm/codex-package.mjs';
// Budgets the paired guest network build.
import { networkGo } from '../vm/network-toolchain.mjs';
