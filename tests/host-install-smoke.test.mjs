// Verifies a real installed Host release.
import assert from 'node:assert/strict';
// Runs explicit artifact-level installation acceptance tests.
import { test } from 'node:test';
// Hashes served assets against the manifest.
import { createHash } from 'node:crypto';
// Starts the installed launcher outside checkout.
import { spawn, execFile } from 'node:child_process';
// Awaits native ACL and service checks.
import { promisify } from 'node:util';
// Waits for the installed process startup.
import { once } from 'node:events';
// Relocates packages and verifies retained state.
import { cp, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
// Locates disposable directories for installation tests.
import { tmpdir } from 'node:os';
// Resolves artifact, installation and state paths.
import { join, resolve, parse, win32 } from 'node:path';
// Imports installation code from relocated portable releases.
import { pathToFileURL } from 'node:url';
// Uses the real installation and planning workflow.
import { install } from '../scripts/setup/install.mjs';
// Exercises the production service and runtime updater.
import { refreshRuntime } from '../scripts/setup/update-runtime.mjs';
// Builds a host-only plan without images.
import { GiB, planSetup } from '../scripts/setup/plan.mjs';
// Provides isolated Codex responses and child cleanup.
import { eventually, freePort, peerFixture, stopChild } from './support/harness.mjs';

test('installed Host release starts and retains data outside checkout', async t => {
  assert.ok(process.env.MINICLAW_HOST_PACKAGE, 'Set MINICLAW_HOST_PACKAGE to a built Host bundle');
  const portable = process.env.MINICLAW_PORTABLE_PACKAGE;
  const installHost = portable ? (await import(pathToFileURL(join(portable, 'scripts/setup/install.mjs')))).install : install;
  const updateHost = portable ? (await import(pathToFileURL(join(portable, 'scripts/setup/update-runtime.mjs')))).refreshRuntime : refreshRuntime;
  const root = await mkdtemp(join(tmpdir(), 'mc-installed-'));
  const peer = await peerFixture();
  let child;
  let installedService;
  let acceptanceCompleted = false;
  const windows = process.platform === 'win32';
  t.after(async () => {
    if (child) await stopChild(child);
    await peer.cleanup();
    if (windows && !acceptanceCompleted) {
      if (installedService) await removeWindowsTestService(installedService, true);
      t.diagnostic(`Preserved failed Windows installation for recovery: ${root}`);
      return;
    }
    if (installedService) await removeWindowsTestService(installedService);
    await rm(root, { recursive: true, force: true });
  });
  const port = await freePort();
  const plan = planSetup({ port, providers: { codex: true, claude: false }, vmEnabled: false, master: null },
    { platform: process.platform, arch: process.arch, cpuCount: 8,
      memoryBytes: 8 * GiB, diskBytes: 256 * GiB, freeBytes: 150 * GiB });
  const destination = join(root, 'installed app');
  const settings = await installHost({ plan, destination, cache: join(root, 'cache'), codexBin: peer.binary, approved: true,
    ...(portable ? {} : { buildHost: path => cp(resolve(process.env.MINICLAW_HOST_PACKAGE), path, { recursive: true }) }),
    ...(windows ? {} : { download: () => assert.fail('Mac Host-only install downloaded an image') }),
  });
  installedService = settings.windowsNetwork?.serviceName;
  let manifest = JSON.parse(await readFile(join(destination, 'runtime/manifest.json'), 'utf8'));
  assert.equal(manifest.role, 'host');
  for (const [path, hash] of Object.entries(manifest.files)) {
    assert.equal(createHash('sha256').update(await readFile(join(destination, 'runtime', path))).digest('hex'), hash);
  }
  if (windows) {
    assert.match(installedService, /^MiniClawNetwork-[a-f0-9]{32}$/);
    await assertPrivateWindowsState(join(destination, 'data'));
  } else {
    assert.equal((await stat(destination)).mode & 0o777, 0o700);
    assert.equal((await stat(join(destination, 'settings.json'))).mode & 0o777, 0o600);
  }
  const base = `http://127.0.0.1:${port}`;
  for (let boot = 0; boot < 3; boot++) {
    if (boot === 2) {
      // `miniclaw update` builds its bundle inside the installation, where the service pin trusts its manifest.
      const bundle = join(destination, '.update-smoke');
      await cp(resolve(process.env.MINICLAW_HOST_PACKAGE), bundle, { recursive: true });
      const proof = Buffer.from('installed update retained state');
      await writeFile(join(bundle, 'web/update-proof.txt'), proof);
      manifest = { ...manifest, version: `${manifest.version}-update-smoke`,
        files: { ...manifest.files, 'web/update-proof.txt': createHash('sha256').update(proof).digest('hex') } };
      await writeFile(join(bundle, 'manifest.json'), JSON.stringify(manifest));
      await updateHost({ destination, bundle });
      await rm(bundle, { recursive: true, force: true });
      assert.equal(JSON.parse(await readFile(join(destination, 'runtime/manifest.json'), 'utf8')).version, manifest.version);
      if (windows) await assertPrivateWindowsState(join(destination, 'data'));
    }
    child = spawn(process.execPath, [join(destination, 'runtime/start-host.mjs')], {
      cwd: parse(destination).root, env: { ...process.env, CODEX_HOME: peer.codexHome },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let logs = '';
    for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { logs = (logs + chunk).slice(-8192); });
    await once(child, 'spawn');
    await peer.ready();
    const health = await eventually(async () => {
      assert.equal(child.exitCode, null, logs);
      try { return await (await fetch(`${base}/api/health`)).json(); }
      catch { return null; }
    }, Boolean, 'installed Host readiness');
    // Health names the operating system, so the page can word itself for it (PROTO-01).
    assert.deepEqual(health, { backend: 'ok', platform: windows ? 'windows' : 'macos', claude: 'unavailable', codex: 'connected' });
    for (const [path, hash] of Object.entries(manifest.files)) {
      if (!path.startsWith('web/')) continue;
      const response = await fetch(`${base}/${path.slice(4)}`);
      assert.equal(response.status, 200);
      assert.equal(createHash('sha256').update(Buffer.from(await response.arrayBuffer())).digest('hex'), hash);
    }
    assert.equal((await fetch(`${base}/api/health`, { headers: { Origin: 'https://untrusted.invalid' } })).status, 403);
    const marker = join(destination, 'data/keep');
    if (!boot) await writeFile(marker, 'retained');
    else assert.equal(await readFile(marker, 'utf8'), 'retained');
    await stopChild(child);
    if (windows) {
      // Terminating the Node launcher closes its private pipe; the backend must drain and stop.
      await eventually(async () => {
        try { await fetch(`${base}/api/health`, { signal: AbortSignal.timeout(500) }); return false; }
        catch { return true; }
      }, Boolean, 'installed backend shutdown after launcher exit');
    } else assert.equal(child.exitCode, 0, logs);
    // The fake peer exits without unlinking its control socket; it is not application state.
    if (!windows) await rm(peer.socket, { force: true });
    t.diagnostic(`Installed Host boot ${boot + 1}: health, assets and shutdown passed`);
  }
  acceptanceCompleted = true;
});

// Checks the real installer boundary for private data and service client credentials.
async function assertPrivateWindowsState(data) {
  const script = `$ErrorActionPreference='Stop'; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;
    foreach ($path in @($env:MC_INSTALLED_DATA, (Join-Path $env:MC_INSTALLED_DATA 'providers.json'),
      (Join-Path $env:MC_INSTALLED_DATA 'windows-network.json'),
      (Join-Path $env:MC_INSTALLED_DATA 'windows-git.json'))) {
      $acl=Get-Acl -LiteralPath $path;
      if (!$acl.AreAccessRulesProtected -or !$acl.Access.Count) { throw 'Installed state has an inherited or empty DACL' };
      if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw 'Installed state owner differs' };
      foreach ($entry in $acl.Access) {
        if ($entry.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $sid) {
          throw 'Installed state grants another identity'
        }
      }
    }`;
  await promisify(execFile)(windowsShell(), ['-NoProfile', '-NonInteractive', '-Command', script],
    { env: { ...process.env, MC_INSTALLED_DATA: data }, timeout: 15000 });
}

// Locates the system PowerShell used by the production installer.
function windowsShell() {
  return win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
}

// Reuses the native test cleanup only after this install returned its exact service identity.
async function removeWindowsTestService(name, preserve = false) {
  assert.match(name, /^MiniClawNetwork-[a-f0-9]{32}$/);
  const body = await readFile(new URL('../scripts/windows-ci-stop-network.ps1', import.meta.url), 'utf8');
  const inner = `try { & {\n${body}\n} -Id '${name.slice('MiniClawNetwork-'.length)}'${preserve ? ' -PreserveInstallation' : ''}; exit 0 } catch { Write-Error $_; exit 1 }`;
  const encoded = Buffer.from(inner, 'utf16le').toString('base64');
  const shell = windowsShell();
  const command = `$p=Start-Process -FilePath '${shell.replaceAll("'", "''")}' -Verb RunAs -Wait -PassThru `
    + `-ArgumentList '-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encoded}'; exit $p.ExitCode`;
  await promisify(execFile)(shell, ['-NoProfile', '-NonInteractive', '-Command', command], { timeout: 180000 });
}
