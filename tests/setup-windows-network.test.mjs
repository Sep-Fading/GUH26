// Checks privilege-bound installation commands without elevating the test runner.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { networkServiceCommand } from '../scripts/setup/windows-network.mjs';
// Supplies actual native private-file helpers to Windows orchestration fixtures.
import { copyFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { nativeArtifacts } from './support/provider-fixture-paths.mjs';

// Keep elevation mocked while exercising real private storage on Windows.
const hostFixture = {
  buildHost: async runtime => {
    await mkdir(runtime);
    if (process.platform === 'win32') {
      await mkdir(join(runtime, 'bin'));
      const { backend } = nativeArtifacts(fileURLToPath(new URL('../', import.meta.url)), process.env.MINICLAW_TEST_TARGET);
      await copyFile(backend, join(runtime, 'bin/miniclaw-server.exe'));
    }
  },
  secureData: async (runtime, data) => {
    if (process.platform === 'win32') {
      await promisify(execFile)(join(runtime, 'bin/miniclaw-server.exe'), ['--secure-data-dir', data]);
    } else await mkdir(data);
  },
};

// A hostile source filename remains literal inside the fixed elevated script call.
test('network installer quotes paths and pins the copied binary', () => {
  const source = "C:\\Build Ω\\O'Brien $(`whoami`)\\miniclaw-server.exe";
  const command = networkServiceCommand({ source, sha256: 'a'.repeat(64),
    ownerSid: 'S-1-5-21-123-456-789-1001', id: 'b'.repeat(32),
    scriptBody: "param($Source, $ExpectedSha256, $OwnerSid, $Id)\nWrite-Output 'reviewed-script'" });
  assert.equal(command.args[0], '-NoLogo');
  const outer = command.args.at(-1);
  assert.match(outer, /-Verb RunAs/);
  const encoded = outer.match(/-EncodedCommand ([A-Za-z0-9+/=]+)/)[1];
  const inner = Buffer.from(encoded, 'base64').toString('utf16le');
  assert.ok(inner.includes("Write-Output 'reviewed-script'"));
  assert.ok(inner.includes("'C:\\Build Ω\\O''Brien $(`whoami`)\\miniclaw-server.exe'"));
  assert.ok(inner.includes(`-ExpectedSha256 '${'a'.repeat(64)}'`));
  assert.ok(inner.includes("-OwnerSid 'S-1-5-21-123-456-789-1001'"));
});

// Malformed identities and executable paths cannot become an elevated command.
test('network installer refuses malformed privileged inputs', () => {
  const valid = { source: 'C:\\build\\miniclaw-server.exe', sha256: 'a'.repeat(64),
    ownerSid: 'S-1-5-21-123-456-789-1001', id: 'b'.repeat(32), scriptBody: 'param()' };
  for (const change of [{ source: '\\\\server\\share\\server.exe' }, { source: 'server.exe' },
    { sha256: 'x' }, { ownerSid: 'S-1-5-18' }, { id: '../escape' }, { scriptBody: '' }, { scriptBody: 'x'.repeat(30000) }]) {
    assert.throws(() => networkServiceCommand({ ...valid, ...change }));
  }
});

// Windows completion depends on the network service, and failed elevation rolls back files.
test('Windows installation requires a ready network service', async t => {
  const { mkdtemp, mkdir, rm, access } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { install } = await import('../scripts/setup/install.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-network-install-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const options = { plan: { platform: 'win32', providers: { codex: true }, port: 8787,
    images: [], packages: [], peakBytes: 0 }, destination: join(root, 'install'),
    cache: join(root, 'cache'), codexBin: process.execPath, approved: true,
    checkPort: async () => {}, freeBytes: async () => 1e9,
    ...hostFixture };
  let called = false;
  await assert.rejects(install({ ...options, installNetwork: async () => {
    called = true; throw new Error('Elevation declined');
  } }), /Elevation declined/);
  assert.ok(called);
  await assert.rejects(access(options.destination), { code: 'ENOENT' });
  const result = await install({ ...options, installNetwork: async ({ runtime, data }) => {
    assert.equal(runtime, join(options.destination, 'runtime'));
    assert.equal(data, join(options.destination, 'data'));
    return { serviceName: 'MiniClawNetwork-test' };
  } });
  assert.deepEqual(result.windowsNetwork, { serviceName: 'MiniClawNetwork-test' });
  assert.equal(result.hostReady, true);
});

// Shipped elevated payloads must fit CreateProcess limits, including literal invocation arguments.
test('real network installation and update payloads fit Windows command limits', async () => {
  const { readFile } = await import('node:fs/promises');
  for (const name of ['windows-network-install.ps1', 'windows-network-update.ps1',
    '../windows-native-acceptance.ps1', '../windows-native-standard-user.ps1', '../windows-ci-stop-network.ps1', '../windows-ci-remove-profile.ps1']) {
    const scriptBody = await readFile(new URL(`../scripts/setup/${name}`, import.meta.url), 'utf8');
    const command = networkServiceCommand({ source: 'C:\\Users\\Owner Ω\\MiniClaw\\runtime\\bin\\miniclaw-server.exe',
      sha256: 'a'.repeat(64), ownerSid: 'S-1-5-21-123-456-789-1001', id: 'b'.repeat(32), scriptBody });
    assert.ok(command.args.at(-1).length < 30000);
  }
});

// The acceptance scripts read their test lists from a data file beside them and refuse one whose
// hash they do not hold, so the lists stay out of the size-limited payload without becoming unreviewed input.
test('native acceptance scripts hold the hash of their test lists', async () => {
  const { readFile } = await import('node:fs/promises');
  const { createHash } = await import('node:crypto');
  const lists = (await readFile(new URL('../scripts/windows-native-tests.psd1', import.meta.url), 'utf8')).replaceAll('\r', '');
  const digest = createHash('sha256').update(lists).digest('hex');
  for (const name of ['windows-native-acceptance.ps1', 'windows-native-standard-user.ps1']) {
    const script = await readFile(new URL(`../scripts/${name}`, import.meta.url), 'utf8');
    assert.ok(script.includes(`if ($seen -ne '${digest}') { throw`), `${name} checks the current lists`);
    assert.match(script, /Import-PowerShellDataFile -LiteralPath \$lists/, 'the lists are read as data, never run');
  }
  // PowerShell names ignore case: a loop variable named like the validated -Target parameter is
  // refused at run time (seen natively, 2026-10-05). Only the parameter itself is assigned.
  const acceptance = await readFile(new URL('../scripts/windows-native-acceptance.ps1', import.meta.url), 'utf8');
  assert.equal(acceptance.match(/\$target\s*(,|=)/gi).length, 1, 'nothing but the parameter default assigns $Target');
  // Every sandboxed entry point's gate tests, and the bundled Git over HTTPS, are on the lists.
  for (const must of ["'windows_gate::'", "'executor::gate_tests'", 'windows_tools git_ls_remote_through_the_approved_proxy',
    'the_switch_closes_every_sandboxed_entry_point', 'the_gate_follows_the_service']) assert.ok(lists.includes(must), must);
});

// The native PowerShell parser checks packaged scripts without executing privileged actions.
test('Windows PowerShell parses shipped network service scripts', { skip: process.platform !== 'win32' }, async () => {
  const { execFile } = await import('node:child_process');
  const { promisify } = await import('node:util');
  const { fileURLToPath } = await import('node:url');
  const { win32 } = await import('node:path');
  for (const name of ['windows-network-install.ps1', 'windows-network-update.ps1', 'windows-install-directory.ps1',
    '../windows-native-acceptance.ps1', '../windows-native-standard-user.ps1', '../windows-ci-stop-network.ps1', '../windows-ci-remove-profile.ps1']) {
    const path = fileURLToPath(new URL(`../scripts/setup/${name}`, import.meta.url)).replaceAll("'", "''");
    await promisify(execFile)(win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe'),
      ['-NoProfile', '-NonInteractive', '-Command', `$tokens=$null; $errors=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('${path}', [ref]$tokens, [ref]$errors); if ($errors.Count) { throw ($errors | Out-String) }`]);
  }
});

// A service may exist even if copying its client receipt failed; never erase that installation.
test('Windows setup retains a resumable private receipt on ambiguous service completion', async t => {
  const { mkdtemp, mkdir, rm, readFile } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { install } = await import('../scripts/setup/install.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-network-recovery-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const destination = join(root, 'install');
  await assert.rejects(install({ plan: { platform: 'win32', providers: { codex: true }, port: 8787,
    images: [], packages: [], peakBytes: 0 }, destination, cache: join(root, 'cache'),
    codexBin: process.execPath, approved: true, checkPort: async () => {}, freeBytes: async () => 1e9,
    ...hostFixture,
    installNetwork: async () => { throw Object.assign(new Error('Client copy failed'),
      { windowsNetwork: { serviceName: `MiniClawNetwork-${'b'.repeat(32)}` } }); } }), error => {
    assert.match(error.message, /retained/);
    // The wizard reads this to say the folder was kept, not removed.
    assert.equal(error.retained, destination);
    return true;
  });
  const pending = JSON.parse(await readFile(join(destination, 'data/windows-setup-pending.json'), 'utf8'));
  assert.equal(pending.hostReady, false);
  assert.equal(pending.port, 8787);
  const { resumeWindows } = await import('../scripts/setup/resume-windows.mjs');
  let secured = false;
  const saved = await resumeWindows(destination, { secure: async () => { secured = true; },
    recoverNetwork: async options => {
      assert.ok(secured);
      assert.equal(options.resume, true);
      return { serviceName: `MiniClawNetwork-${'b'.repeat(32)}` };
    } });
  assert.equal(saved.hostReady, true);
  assert.equal(saved.status, 'installed');
  assert.equal(JSON.parse(await readFile(join(destination, 'settings.json'), 'utf8')).hostReady, true);
});

// Azure AD interactive user SIDs have a distinct authority; service identities stay excluded.
test('network service accepts Azure AD users and rejects service principals', () => {
  const base = { source: 'C:\\build\\miniclaw-server.exe', sha256: 'a'.repeat(64),
    id: 'b'.repeat(32), scriptBody: 'param($Source,$ExpectedSha256,$OwnerSid,$Id)' };
  assert.doesNotThrow(() => networkServiceCommand({ ...base, ownerSid: 'S-1-12-1-123-456-789-1001' }));
  for (const ownerSid of ['S-1-5-18', 'S-1-5-19', 'S-1-5-20', 'S-1-5-80-1-2-3-4-5',
    'S-1-12-1-1-2-3', 'S-1-12-1-1-2-3-4294967296', 'S-1-12-1-01-2-3-4']) {
    assert.throws(() => networkServiceCommand({ ...base, ownerSid }));
  }
});

// Returns one shipped elevated script as text; these contracts cannot execute off Windows.
async function shipped(name) {
  const { readFile } = await import('node:fs/promises');
  return readFile(new URL(`../scripts/setup/${name}`, import.meta.url), 'utf8');
}

// A swap that fails after the drain must restart the original service binary.
test('service update restarts the original binary when the swap fails after the drain', async () => {
  const script = await shipped('windows-network-update.ps1');
  const body = script.slice(script.indexOf('$updateLock ='));
  assert.match(body, /Stop-DrainedService\s+\$stopped = \$true\s+\[IO\.File\]::Move\(\$binary, \$backup\)/);
  const recovery = body.slice(body.indexOf('} catch {'));
  const unswapped = recovery.slice(recovery.indexOf('elseif ($stopped)'));
  assert.ok(recovery.includes('elseif ($stopped)'), 'the catch handles a stop without a completed swap');
  assert.match(unswapped, /\[IO\.File\]::Exists\(\$binary\)[\s\S]*Start-Service -Name \$name/);
  assert.match(unswapped, /Write-Warning/);
});

// ERROR_SERVICE_NEVER_STARTED (1077) with an empty lease journal has nothing to clean up.
test('a never-started service without recovery records is safe to remove or replace', async () => {
  for (const name of ['windows-network-install.ps1', 'windows-network-update.ps1']) {
    const script = await shipped(name);
    const check = script.slice(script.indexOf('function Test-ServiceDrained'));
    assert.ok(script.includes('function Test-ServiceDrained'), name);
    assert.match(check, /ExitCode -eq 0 -and \$State\.ServiceSpecificExitCode -eq 0/, name);
    assert.match(check, /ExitCode -ne 1077/, name);
    assert.match(check, /Join-Path \$configFolder 'leases'/, name);
    assert.match(check, /EnumerateFileSystemEntries/, name);
    // The raw zero-only comparison no longer decides cleanup on its own.
    assert.doesNotMatch(script.replace(/function Test-ServiceDrained[\s\S]*?\n}\n/, ''), /\.ExitCode -ne 0/, name);
    assert.match(script, /Test-ServiceDrained \$stat(us|e)/, name);
  }
});

// A service that ran leaves a `leases` folder only SYSTEM may open. An elevated administrator
// reads it with backup semantics (SeBackupPrivilege + FILE_FLAG_BACKUP_SEMANTICS), read-only,
// never following a reparse point and never changing its ACL or owner.
test('the drained check reads a SYSTEM-only lease folder with backup semantics, read-only', async () => {
  // A fresh installation's own service that never started (1077) cannot have written the journal, so
  // there an unreadable folder is refused outright; the elevated payload also has no room for the reader.
  const install = await shipped('windows-network-install.ps1');
  assert.match(install, /catch \[UnauthorizedAccessException\] \{ throw 'The network service journal is readable only by SYSTEM; it is kept for administrator recovery' \}/);
  for (const name of ['windows-network-update.ps1']) {
    const script = await shipped(name);
    const check = script.slice(script.indexOf('function Test-ServiceDrained'), script.indexOf('\n}\n', script.indexOf('function Test-ServiceDrained')));
    assert.match(check, /catch \[UnauthorizedAccessException\] \{ return \(Get-BackupEntryCount \$leases\) -eq 0 \}/, name);
    assert.ok(script.includes('function Get-BackupEntryCount'), name);
    const start = script.indexOf('public static class MiniClawBackupRead');
    const reader = script.slice(start, script.indexOf("'@", start));
    assert.ok(start > 0, name);
    assert.match(reader, /"SeBackupPrivilege"/, name);
    // SYNCHRONIZE | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY with BACKUP_SEMANTICS | OPEN_REPARSE_POINT.
    assert.match(reader, /CreateFileW\(path,0x100081,7,IntPtr\.Zero,3,0x02200000,IntPtr\.Zero\)/, name);
    assert.match(reader, /&0x410\)!=0x10/, name, 'the folder must be a plain directory, not a reparse point');
    assert.match(reader, /AdjustTokenPrivileges\(t,false,ref o/, name, 'the previous privilege state is restored');
    for (const forbidden of [/SeRestorePrivilege/, /SetFileInformationByHandle/, /Set-Acl/, /takeown/i, /icacls/i, /SetOwner\(.*leases/]) {
      assert.doesNotMatch(reader + check, forbidden, `${name}: ${forbidden}`);
    }
  }
});

// The CI and acceptance cleanup removes a service's data folder after the service has run, with
// backup and restore semantics, only for the exact MiniClawNetwork-<id> folder under ProgramData.
test('the CI cleanup removes only the exact service data folder with backup and restore semantics', async () => {
  const script = await shipped('../windows-ci-stop-network.ps1');
  assert.match(script, /\[switch\]\$LeftoverDataOnly/);
  assert.match(script, /"SeBackupPrivilege"/);
  assert.match(script, /"SeRestorePrivilege"/);
  assert.match(script, /0x02200000/, 'backup semantics without following reparse points');
  assert.match(script, /FileDispositionInfo|SetFileInformationByHandle\(h,4,/);
  assert.match(script, /refusing a reparse point/);
  assert.match(script, /GetFolderPath\('CommonApplicationData'\)/);
  assert.match(script, /-cnotmatch '\^MiniClawNetwork-\[a-f0-9\]\{32\}\$'/);
  // The leftover mode never touches a registered service, and the data folder is never removed by Remove-Item.
  assert.match(script, /if \(\$LeftoverDataOnly\) \{[\s\S]*?Win32_Service[\s\S]*?still registered/);
  assert.doesNotMatch(script, /Remove-Item[^\n]*CommonApplicationData/);
  assert.doesNotMatch(script, /Set-Acl|takeown|icacls/i);
});

// The elevated step waits long enough for a user who looked away, and a timeout says exactly what to run.
test('the wait for administrator approval is five minutes, and its timeout names miniclaw resume', async () => {
  const { approvalWaitMs, runElevated } = await import('../scripts/setup/windows-network.mjs');
  assert.ok(approvalWaitMs >= 5 * 60_000, `${approvalWaitMs}`);
  const command = { file: 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe', args: ['-Command', 'x'.repeat(4000)] };
  let options;
  const timedOut = await runElevated(command, { action: 'install', run: async (file, args, given) => {
    options = given;
    throw Object.assign(new Error(`Command failed: ${file} ${args.join(' ')}`), { killed: true, signal: 'SIGTERM', code: null });
  } }).then(() => assert.fail('the step must fail'), error => error);
  assert.equal(options.timeout, approvalWaitMs);
  assert.match(timedOut.message, /5 minutes/);
  assert.match(timedOut.message, /miniclaw resume/);
  assert.ok(!timedOut.message.includes('x'.repeat(100)), 'the encoded command is not shown');
  assert.equal(timedOut.approvalTimedOut, true);
  // An update has nothing to resume: it was undone, so it is run again.
  const update = await runElevated(command, { action: 'update', run: async () => { throw Object.assign(new Error('Command failed'), { killed: true, signal: 'SIGTERM' }); } })
    .then(() => assert.fail('the step must fail'), error => error);
  assert.match(update.message, /miniclaw update/);
  assert.doesNotMatch(update.message, /resume/);
  const declined = await runElevated(command, { action: 'install', run: async () => {
    throw Object.assign(new Error('Command failed'), { code: 1, stderr: 'Start-Process : This command cannot be run due to the error: The operation was canceled by the user.\r\n' });
  } }).then(() => assert.fail('the step must fail'), error => error);
  assert.match(declined.message, /approval was declined/i);
  const ok = await runElevated(command, { action: 'install', run: async () => ({ stdout: '', stderr: '' }) });
  assert.deepEqual(ok, { stdout: '', stderr: '' });
});
