// Checks command routing and safe startup.
import assert from 'node:assert/strict';
// Runs command tests without real services.
import { test } from 'node:test';
// Invokes the actual executable command entry.
import { spawnSync } from 'node:child_process';
// Creates disposable installed configurations for dispatch.
import { mkdtemp, writeFile, rm, readFile } from 'node:fs/promises';
// Resolves command and installation fixture paths.
import { join, resolve } from 'node:path';
// Locates temporary storage outside user installations.
import { tmpdir } from 'node:os';

const windows = process.platform === 'win32';
// Matches a path literally; Windows paths are full of backslashes.
const literal = text => new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));

// Returns command output without starting any interactive prompt.
function command(...args) {
  return spawnSync(process.execPath, [resolve('scripts/cli.mjs'), ...args],
    { encoding: 'utf8', timeout: 5000 });
}

test('miniclaw exposes setup and start commands', async () => {
  const pkg = JSON.parse(await readFile('package.json', 'utf8'));
  assert.equal(pkg.bin?.miniclaw, 'scripts/cli.mjs');
  const result = command('--help');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /miniclaw setup/);
  assert.match(result.stdout, /miniclaw start/);
});

test('setup preview reports this Mac and changes nothing', () => {
  const result = command('setup', '--preview', '--mac-only');
  assert.equal(result.status, 0, result.stderr);
  // Windows reports its own sandbox (AppContainer and the network service) in the same place.
  assert.match(result.stdout, windows ? /Windows sandbox +AppContainer \+ WFP service/ : /Seatbelt sandbox/);
  assert.match(result.stdout, /Nothing was changed|Install Codex or Claude Code first/);
  assert.doesNotMatch(result.stdout, /Linux/);
});

test('setup has no separate Master command', () => {
  const result = command('setup', 'master', '--preview');
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /Unknown setup option/);
  assert.doesNotMatch(command('--help').stdout, /setup master/);
});

test('an installation from the retired Linux backend is refused with a way forward', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-cli-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'settings.json'), JSON.stringify({ schemaVersion: 1,
    backend: 'linux', vmBackendReady: true, hostReady: true }));
  const result = command('start', '--installation', root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /retired Linux backend.*miniclaw setup/);
});

test('unknown commands fail without starting anything', () => {
  const result = command('shell', 'whoami');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Unknown command/);
});

// The per-user folders of a private profile. Windows keeps the record under LOCALAPPDATA,
// which wins over the home folder, so both are redirected there.
const profile = home => ({ ...process.env, HOME: home,
  ...(windows ? { USERPROFILE: home, LOCALAPPDATA: join(home, 'AppData', 'Local') } : {}) });

// Runs the CLI with a private HOME so the remembered installation never touches the real one.
function withHome(home, ...args) {
  return spawnSync(process.execPath, [resolve('scripts/cli.mjs'), ...args],
    { encoding: 'utf8', timeout: 5000, env: profile(home) });
}

// Creates a fake ready installation whose launcher prints what it was given.
async function fakeInstallation(root) {
  const { mkdir } = await import('node:fs/promises');
  await mkdir(join(root, 'runtime'), { recursive: true });
  await writeFile(join(root, 'settings.json'), JSON.stringify({ schemaVersion: 1, backend: 'host', hostReady: true }));
  await writeFile(join(root, 'runtime/start-host.mjs'), `console.log('started ' + ${JSON.stringify(root)});`);
}

test('the remembered installation makes start and the bare command need no path', async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-cli-home-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const install = join(home, 'My MiniClaw');
  await fakeInstallation(install);
  const { rememberInstallation, defaultInstallation } = await import('../scripts/setup/default-installation.mjs');
  const env = { env: profile(home) };
  assert.equal(await defaultInstallation(home, env), windows ? join(home, 'AppData', 'Local', 'MiniClaw Install')
    : join(home, 'Library/Application Support/MiniClaw Install'));
  await rememberInstallation(install, home, env);
  assert.equal(await defaultInstallation(home, env), install);
  for (const args of [[], ['start']]) {
    const result = withHome(home, ...args);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, literal(`started ${install}`));
  }
  assert.match(withHome(home, 'where').stdout, literal(install));
});

test('a path can be given plainly, and the old flag still works', async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-cli-home-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const install = join(home, 'elsewhere');
  await fakeInstallation(install);
  for (const args of [['start', install], ['start', '--installation', install]]) {
    const result = withHome(home, ...args);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /started/);
  }
});

test('with nothing installed the bare command says how to begin', async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-cli-home-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const result = withHome(home);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /miniclaw setup/);
  assert.match(withHome(home, 'start').stderr, /Nothing is installed yet.*miniclaw setup/s);
});

test('short commands map to the long ones', () => {
  const help = command('--help').stdout;
  for (const line of ['miniclaw setup', 'miniclaw start', 'miniclaw resume', 'miniclaw repair', 'miniclaw ledger',
    'miniclaw update-vm', 'miniclaw where']) {
    assert.match(help, new RegExp(line));
  }
  assert.match(command('ledger', '--bogus').stderr, /miniclaw ledger/);
  // Resume explains a missing installation before it needs a terminal, on Windows too.
  const resumed = command('resume', '/definitely/not/installed');
  assert.match(resumed.stdout + resumed.stderr, literal(`No installation at ${resolve('/definitely/not/installed')}`));
});

// The VM belongs to the Mac build; on Windows its commands stop with a plain reason instead of
// failing inside Mac-only code (the ledger checks POSIX owners with process.getuid).
test('the VM commands say plainly that Windows has no VM', { skip: !windows && 'macOS runs the VM commands (tests/vm-ledger, tests/vm-update-media)' }, () => {
  for (const args of [['ledger'], ['vm', 'ledger'], ['update-vm']]) {
    const result = command(...args);
    assert.equal(result.status, 1, args.join(' '));
    assert.match(result.stderr, /MiniClaw on Windows has no VM/);
    assert.doesNotMatch(result.stderr, /getuid|TypeError|at .*\.mjs/);
  }
});

// A Windows drive path must become the destination, not an unknown setup argument.
test('setup recognizes Windows backslash paths before platform validation', () => {
  const result = command('setup', 'C:\\MiniClaw Test', '--preview');
  assert.doesNotMatch(result.stdout + result.stderr, /Unknown setup option/);
});

// Repair restarts the Windows network service; elsewhere it says plainly that there is nothing to do.
test('repair is a Windows command', { skip: windows && 'tests/setup-windows-repair.test.mjs covers the Windows path' }, () => {
  const result = command('repair');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Windows network service/);
});

test('miniclaw guide lists the topics and prints one guide for this computer or the other', () => {
  const list = command('guide');
  assert.equal(list.status, 0, list.stderr);
  for (const topic of ['computer-use', 'provider-sign-in']) {
    assert.match(list.stdout, new RegExp(`\\b${topic}\\b`));
  }
  const mac = command('guide', 'computer-use', '--mac');
  assert.equal(mac.status, 0, mac.stderr);
  assert.match(mac.stdout, /^1\. /m);
  assert.match(mac.stdout, /Codex Computer Use\.app/);
  const pc = command('guide', 'computer-use', '--windows');
  assert.match(pc.stdout, /WindowsApps/);
  assert.doesNotMatch(pc.stdout, /Finder/);
  const unknown = command('guide', 'nonsense');
  assert.equal(unknown.status, 1);
  assert.match(unknown.stderr, /computer-use/);
  assert.match(command('--help').stdout, /miniclaw guide/);
});

test('setup --help points to the guides', () => {
  const result = command('setup', '--help');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /miniclaw guide/);
});
