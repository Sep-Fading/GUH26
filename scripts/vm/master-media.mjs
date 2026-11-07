// Creates private media without touching guests.
import { chmod, copyFile, lstat, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
// Hashes locally built worker installation payloads.
import { createHash } from 'node:crypto';
// Resolves packaged sources and output locations.
import { dirname, join, resolve } from 'node:path';
// Finds bundled sources independently of cwd.
import { fileURLToPath } from 'node:url';
// Verifies archives before creating setup media.
import { verifyDownload } from '../setup/download.mjs';
// Runs native media creation without shells.
import { run } from '../setup/system.mjs';
// Uses a release's prebuilt files instead of compiling.
import { prebuilt } from '../setup/prebuilt.mjs';
// Shares the reviewed guest package descriptor.
import { masterCodex } from './codex-package.mjs';
// Verifies source and copied helper signatures consistently.
import { verifyComputerApp } from './computer-source.mjs';

// Builds read-only guest setup media without installing software or starting a VM.
export async function prepareMasterMedia({ archive, output, worker, computerApp,
  signal, onOutput = () => {} }) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') {
    throw new Error('Master setup requires an Apple silicon Mac');
  }
  output = resolve(output);
  await verifyDownload(archive, masterCodex, signal);
  await mkdir(output, { mode: 0o700 });
  try {
    const payload = join(output, 'Payload');
    // The setup disk must be traversable by the selected Standard guest account.
    await mkdir(payload, { mode: 0o755 });
    await chmod(payload, 0o755);
    await copyFile(archive, join(payload, 'codex.tar.gz'));
    await chmod(join(payload, 'codex.tar.gz'), 0o644);
    await verifyDownload(join(payload, 'codex.tar.gz'), masterCodex, signal);
    await writeFile(join(payload, 'archive.sha256'), `${masterCodex.digest}  codex.tar.gz\n`,
      { flag: 'wx', mode: 0o644 });
    await chmod(join(payload, 'archive.sha256'), 0o644);
    const source = dirname(fileURLToPath(import.meta.url));
    await copyFile(join(source, 'install-master.command'), join(payload, 'Install Codex.command'));
    await chmod(join(payload, 'Install Codex.command'), 0o755);
    if (computerApp) {
      const staging = join(output, 'computer');
      await mkdir(staging, { mode: 0o700 });
      const destination = join(staging, 'Codex Computer Use.app');
      try {
        if (!(await lstat(computerApp)).isDirectory()) throw new Error('Expected application directory');
        await verifyComputerApp(computerApp, { signal });
        await run('/usr/bin/ditto', [computerApp, destination], { signal });
        await verifyComputerApp(destination, { signal });
        // The archive keeps HFS media metadata outside the signed app bundle.
        await run('/usr/bin/tar', ['-czf', join(payload, 'computer-use.tar.gz'),
          '-C', staging, 'Codex Computer Use.app'], { signal });
        await rm(staging, { recursive: true });
      } catch {
        throw new Error('Computer Use signature verification failed');
      }
    }
    if (worker) {
      if (!/^[a-f0-9]{64}$/.test(worker.settings?.key ?? '')
        || worker.settings.cwd !== '/Users/Shared/MiniClaw/Workspace') {
        throw new Error('Invalid worker pairing settings');
      }
      const info = await lstat(worker.binary);
      if (!info.isFile() || info.size < 8 || info.size > 100 * 1024 ** 2) throw new Error('Invalid worker binary');
      const bytes = await readFile(worker.binary);
      if (bytes.readUInt32LE(0) !== 0xfeedfacf || bytes.readUInt32LE(4) !== 0x100000c) {
        throw new Error('Worker must be an Apple silicon Mach-O binary');
      }
      const settings = Buffer.from(`${JSON.stringify(worker.settings)}\n`);
      await writeFile(join(payload, 'miniclaw-worker'), bytes, { flag: 'wx', mode: 0o755 });
      await writeFile(join(payload, 'worker.json'), settings, { flag: 'wx', mode: 0o644 });
      await chmod(join(payload, 'worker.json'), 0o644);
      const desktop = join(payload, 'miniclaw-desktop-stream');
      const swiftArgs = ['swiftc', '-swift-version', '6', '-warnings-as-errors',
        '-parse-as-library', '-module-cache-path', join(output, 'swift-module-cache')];
      const olderSDK = '/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk';
      try { await lstat(olderSDK); swiftArgs.push('-sdk', olderSDK); } catch { /* Use selected SDK. */ }
      const builtHelper = await prebuilt.file('miniclaw-desktop-stream');
      if (builtHelper) { await copyFile(builtHelper, desktop); await chmod(desktop, 0o755); }
      else await run('/usr/bin/xcrun', [...swiftArgs, join(source, 'desktop-stream.swift'),
        '-o', desktop], { signal, onOutput });
      await run('/usr/bin/codesign', ['--sign', '-', desktop], { signal, onOutput });
      const desktopBytes = await readFile(desktop);
      // Hashes exact payload bytes before building read-only installation media.
      const hash = value => createHash('sha256').update(value).digest('hex');
      await writeFile(join(payload, 'worker.sha256'),
        `${hash(bytes)}  miniclaw-worker\n${hash(settings)}  worker.json\n`
          + `${hash(desktopBytes)}  miniclaw-desktop-stream\n`,
        { flag: 'wx', mode: 0o644 });
      await chmod(join(payload, 'worker.sha256'), 0o644);
    }
    const builtProbe = await prebuilt.file('socket-probe');
    if (builtProbe) { await copyFile(builtProbe, join(payload, 'socket-probe')); await chmod(join(payload, 'socket-probe'), 0o755); }
    else await run('/usr/bin/xcrun', ['clang', '-Wall', '-Wextra', '-Werror',
      join(source, 'master-probe.c'), '-o', join(payload, 'socket-probe')],
    { signal, onOutput });
    await run('/usr/bin/codesign', ['--sign', '-', join(payload, 'socket-probe')],
      { signal, onOutput });
    const image = join(output, 'MasterSetup.dmg');
    await run('/usr/bin/hdiutil', ['makehybrid', '-hfs', '-hfs-volume-name',
      'MiniClawSetup', '-o', image, payload], { signal, onOutput });
    await chmod(image, 0o600);
    await rm(payload, { recursive: true });
    return { image };
  } catch (error) {
    // No guest is started here, so only these newly created media files are removed.
    await rm(output, { recursive: true, force: true });
    throw error;
  }
}
