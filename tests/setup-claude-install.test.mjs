// Checks the offered Claude Code installation without network access or a real installer.
import assert from 'node:assert/strict';
// Registers each installation case.
import { test } from 'node:test';
// Computes the checksum a fake manifest declares.
import { createHash } from 'node:crypto';
// Builds a disposable Windows-style profile folder.
import { mkdtemp, mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Loads the installation step under test.
import { administratorTerminal, claudeInstallable, claudePlatform, installClaude, startClaude } from '../scripts/setup/claude-install.mjs';
// Loads provider discovery for the extra Windows locations.
import { detectProviders } from '../scripts/setup/providers.mjs';

const BASE = 'https://downloads.claude.ai/claude-code-releases';
const BINARY = Buffer.from('MZ fake claude build');
const DIGEST = createHash('sha256').update(BINARY).digest('hex');
const SIGNED = { valid: true, subject: 'CN="Anthropic, PBC", O="Anthropic, PBC", L=San Francisco, S=California, C=US' };

// Serves the release pointer, manifest and binary as Anthropic's download host does.
function release({ version = '2.1.289', checksum = DIGEST, size = BINARY.length, body = BINARY } = {}) {
  const seen = [];
  const routes = {
    [`${BASE}/latest`]: () => new Response(`${version}\n`),
    [`${BASE}/${version}/manifest.json`]: () => Response.json({ version,
      platforms: { 'win32-arm64': { binary: 'claude.exe', checksum, size },
        'win32-x64': { binary: 'claude.exe', checksum, size },
        'darwin-arm64': { binary: 'claude', checksum, size } } }),
    [`${BASE}/${version}/win32-arm64/claude.exe`]: () => new Response(body),
    [`${BASE}/${version}/win32-x64/claude.exe`]: () => new Response(body),
    [`${BASE}/${version}/darwin-arm64/claude`]: () => new Response(body),
  };
  const fetcher = async (url, options) => {
    seen.push([String(url), options?.redirect]);
    return routes[url]?.() ?? new Response('missing', { status: 404 });
  };
  return { fetcher, seen };
}

// Builds a profile and an installer that places the launcher where the real one does.
async function profile(t, name = 'claude.exe') {
  const home = await mkdtemp(join(tmpdir(), 'mc-claude-install-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const launcher = join(home, '.local', 'bin', name);
  const runs = [];
  const runInstaller = async (path, args) => {
    runs.push([path, args, (await readFile(path)).equals(BINARY)]);
    await mkdir(join(home, '.local', 'bin'), { recursive: true });
    await writeFile(launcher, BINARY);
  };
  return { home, launcher, runs, runInstaller };
}

const base = { platform: 'win32', arch: 'arm64', env: {}, elevated: async () => false,
  verifySignature: async () => SIGNED, version: async () => '2.1.289 (Claude Code)' };

const mac = { ...base, platform: 'darwin', verifySignature: async () => ({ valid: true, team: 'Q6L2SF6YDW' }) };

test('each release target has its own official build', () => {
  assert.equal(claudePlatform('win32', 'arm64'), 'win32-arm64');
  assert.equal(claudePlatform('win32', 'x64'), 'win32-x64');
  assert.equal(claudePlatform('win32', 'x64', { PROCESSOR_ARCHITECTURE: 'ARM64' }), 'win32-arm64');
  assert.equal(claudePlatform('darwin', 'arm64'), 'darwin-arm64');
  for (const arch of ['ia32', 'toString', '__proto__']) assert.throws(() => claudePlatform('win32', arch), /architecture/);
  for (const pair of [['darwin', 'x64'], ['linux', 'arm64']]) assert.throws(() => claudePlatform(...pair), /architecture/);
});

test('the Mac build is checked against Anthropic\'s Developer ID team, then installed', async t => {
  const { home, launcher, runs, runInstaller } = await profile(t, 'claude');
  const { fetcher, seen } = release();
  const verified = [];
  let mode;
  const result = await installClaude({ ...mac, home, fetcher,
    verifySignature: async path => { verified.push(path); return { valid: true, team: 'Q6L2SF6YDW' }; },
    runInstaller: async (path, args) => { mode = (await stat(path)).mode & 0o777; return runInstaller(path, args); } });
  assert.deepEqual(result, { path: launcher, version: '2.1.289' });
  const download = join(home, '.claude', 'downloads', 'claude-2.1.289-darwin-arm64');
  assert.equal(seen[2][0], `${BASE}/2.1.289/darwin-arm64/claude`);
  assert.deepEqual(runs, [[download, ['install', 'latest'], true]]);
  assert.deepEqual(verified, [download, launcher]);
  // POSIX mode bits exist only on a Mac host; a Windows host reports 0o666 for any writable file.
  // The Windows plan has no mode step (its build stays in the user's own profile folder).
  if (process.platform !== 'win32') assert.equal(mode, 0o700);
  assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
});

test('a Mac build from another team, or run as root, never runs', async t => {
  for (const signature of [{ valid: false, team: 'Q6L2SF6YDW' }, { valid: true, team: 'ABCDE12345' }, { valid: true }]) {
    const { home, runs, runInstaller } = await profile(t, 'claude');
    await assert.rejects(installClaude({ ...mac, home, fetcher: release().fetcher, runInstaller,
      verifySignature: async () => signature }), /signature|signed by Anthropic/i);
    assert.deepEqual(runs, []);
  }
  const { home, runs, runInstaller } = await profile(t, 'claude');
  await assert.rejects(installClaude({ ...mac, home, fetcher: release().fetcher, runInstaller,
    elevated: async () => true }), /sudo/i);
  assert.deepEqual(runs, []);
});

test('the official build is checked, signed, then installed by its own installer', async t => {
  const { home, launcher, runs, runInstaller } = await profile(t);
  const { fetcher, seen } = release();
  const order = [];
  const result = await installClaude({ ...base, home, fetcher,
    verifySignature: async path => { order.push(['verify', path]); return SIGNED; },
    runInstaller: async (path, args) => { order.push(['run', path]); return runInstaller(path, args); } });
  assert.deepEqual(result, { path: launcher, version: '2.1.289' });
  assert.deepEqual(seen.map(([url]) => url), [`${BASE}/latest`, `${BASE}/2.1.289/manifest.json`,
    `${BASE}/2.1.289/win32-arm64/claude.exe`]);
  assert.ok(seen.every(([, redirect]) => redirect === 'error'));
  const download = join(home, '.claude', 'downloads', 'claude-2.1.289-win32-arm64.exe');
  assert.deepEqual(runs, [[download, ['install', 'latest'], true]]);
  assert.deepEqual(order, [['verify', download], ['run', download], ['verify', launcher]]);
  assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
});

test('the x64 package installs the x64 build', async t => {
  const { home, runInstaller } = await profile(t);
  const { fetcher, seen } = release();
  await installClaude({ ...base, arch: 'x64', home, fetcher, runInstaller });
  assert.equal(seen[2][0], `${BASE}/2.1.289/win32-x64/claude.exe`);
});

test('a download that differs from the manifest never runs', async t => {
  for (const wrong of [{ checksum: 'a'.repeat(64) }, { size: BINARY.length - 1 }, { size: BINARY.length + 1 }]) {
    const { home, runs, runInstaller } = await profile(t);
    const { fetcher } = release(wrong);
    await assert.rejects(installClaude({ ...base, home, fetcher, runInstaller }), /checksum|size/i);
    assert.deepEqual(runs, []);
    assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
  }
});

test('a build without Anthropic\'s valid signature never runs', async t => {
  for (const signature of [{ valid: false, subject: SIGNED.subject }, { valid: true, subject: 'CN=Someone, O=Someone Else' },
    { valid: true, subject: 'CN="Anthropic, PBC", O="Not Anthropic, PBC"' }, { valid: true, subject: null }]) {
    const { home, runs, runInstaller } = await profile(t);
    await assert.rejects(installClaude({ ...base, home, fetcher: release().fetcher, runInstaller,
      verifySignature: async () => signature }), /signature|signed by Anthropic/i);
    assert.deepEqual(runs, []);
    assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
  }
});

test('unexpected server answers stop before any download', async t => {
  const { home, runs, runInstaller } = await profile(t);
  const html = release();
  await assert.rejects(installClaude({ ...base, home, runInstaller,
    fetcher: async url => url.endsWith('/latest') ? new Response('<html>blocked</html>') : html.fetcher(url) }), /version/i);
  await assert.rejects(installClaude({ ...base, home, runInstaller,
    fetcher: async url => url.endsWith('/latest') ? new Response('2.1.289/../../x') : html.fetcher(url) }), /version/i);
  await assert.rejects(installClaude({ ...base, home, runInstaller, fetcher: release({ checksum: 'not-a-hash' }).fetcher }), /manifest/i);
  await assert.rejects(installClaude({ ...base, home, runInstaller, fetcher: release({ size: 2 ** 40 }).fetcher }), /manifest/i);
  assert.deepEqual(runs, []);
});

test('no network gives a plain message and changes nothing', async t => {
  const { home, runs, runInstaller } = await profile(t);
  await assert.rejects(installClaude({ ...base, home, runInstaller,
    fetcher: async () => { throw new TypeError('fetch failed'); } }), /downloads\.claude\.ai.*internet/i);
  await assert.rejects(installClaude({ ...base, home, runInstaller,
    fetcher: async () => new Response('nope', { status: 403 }) }), /HTTP 403/);
  assert.deepEqual(runs, []);
});

test('an administrator terminal or another system is refused', async t => {
  const { home, runs, runInstaller } = await profile(t);
  const { fetcher, seen } = release();
  await assert.rejects(installClaude({ ...base, home, fetcher, runInstaller, elevated: async () => true }), /administrator/i);
  await assert.rejects(installClaude({ ...base, platform: 'linux', home, fetcher, runInstaller }), /architecture/);
  assert.deepEqual([runs, seen], [[], []]);
});

test('a failed installer or a missing launcher is reported', async t => {
  const { home, runInstaller } = await profile(t);
  await assert.rejects(installClaude({ ...base, home, fetcher: release().fetcher,
    runInstaller: async () => { throw new Error('exit 1'); } }), /installer failed/i);
  await assert.rejects(installClaude({ ...base, home, fetcher: release().fetcher,
    runInstaller: async () => {} }), /claude\.exe was not found at/i);
  await assert.rejects(installClaude({ ...base, home, fetcher: release().fetcher, runInstaller,
    version: async () => 'something else' }), /version/i);
  assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
});

// Records prompts and messages like the wizard's terminal interface.
function prompts() {
  const asked = [], warned = [], spun = [], told = [];
  return { asked, warned, spun, told, isCancel: () => false,
    confirm: async options => { asked.push(options); return true; },
    spinner: () => ({ start: text => spun.push(text), message: text => spun.push(text), stop: text => spun.push(text) }),
    log: { warn: text => warned.push(text), info: text => told.push(text), success: text => spun.push(text) } };
}

const codex = { id: 'codex', name: 'Codex (ChatGPT)', path: 'C:\\codex.exe', signedIn: true };
const claude = { id: 'claude', name: 'Claude Code', path: 'C:\\Users\\S\\.local\\bin\\claude.exe', signedIn: false };
const done = { path: claude.path, version: '2.1.289' };

// A background installation the test finishes by hand.
function slow() {
  const calls = [];
  let settle;
  const install = options => { calls.push(options); return new Promise((resolve, reject) => { settle = { resolve, reject }; }); };
  return { calls, install, finish: value => settle.resolve(value), fail: error => settle.reject(error) };
}

test('an installed Claude Code or an unsupported system starts nothing', () => {
  const never = () => assert.fail('must not install');
  for (const platform of ['win32', 'darwin']) {
    const ui = prompts();
    assert.equal(startClaude(ui, [codex, claude], { platform, arch: 'arm64', install: never }), null);
    assert.deepEqual([ui.asked, ui.told, ui.spun, ui.warned], [[], [], [], []]);
  }
  for (const [platform, arch] of [['linux', 'arm64'], ['darwin', 'x64'], ['win32', 'ia32']]) {
    assert.equal(startClaude(prompts(), [], { platform, arch, install: never }), null);
    assert.equal(claudeInstallable(platform, arch), false);
  }
  for (const [platform, arch] of [['win32', 'arm64'], ['win32', 'x64'], ['darwin', 'arm64']]) {
    assert.equal(claudeInstallable(platform, arch), true);
  }
});

test('a missing Claude Code installs in the background after one notice and no question', async () => {
  for (const [platform, arch] of [['win32', 'arm64'], ['win32', 'x64'], ['darwin', 'arm64']]) {
    const ui = prompts(), work = slow();
    const pending = startClaude(ui, [codex], { platform, arch, install: work.install, detect: async () => [codex, claude] });
    assert.equal(work.calls.length, 1);
    assert.deepEqual([work.calls[0].platform, work.calls[0].arch], [platform, arch]);
    assert.equal(ui.asked.length, 0);
    assert.equal(ui.told.length, 1);
    assert.match(ui.told[0], /Claude Code is not installed.*downloading it from Anthropic \(downloads\.claude\.ai, about 250 MB\).*background/);
    // The wizard asks its other questions meanwhile; nothing is shown until it needs the providers.
    work.calls[0].onStatus('Downloading Claude Code 2.1.289 (237 MB) from downloads.claude.ai');
    work.calls[0].onProgress(118_500_000, 237_000_000);
    assert.deepEqual(ui.spun, []);
    const waiting = pending.finish();
    await new Promise(resolve => setImmediate(resolve));
    assert.match(ui.spun[0], /Downloading Claude Code 2\.1\.289.*50%/);
    work.calls[0].onProgress(118_600_000, 237_000_000);
    assert.equal(ui.spun.length, 1);
    work.calls[0].onStatus('Checking Anthropic\'s signature');
    assert.match(ui.spun.at(-1), /signature/);
    work.finish(done);
    assert.deepEqual(await waiting, [codex, claude]);
    assert.match(ui.spun.at(-1), /Claude Code 2\.1\.289 installed.*Sign in/);
    assert.deepEqual(ui.warned, []);
  }
});

test('an installation that finished early is reported without waiting', async () => {
  const ui = prompts();
  const pending = startClaude(ui, [], { platform: 'darwin', arch: 'arm64', install: async () => done, detect: async () => [claude] });
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(await pending.finish(), [claude]);
  assert.equal(ui.spun.length, 1);
  assert.match(ui.spun[0], /Claude Code 2\.1\.289 installed/);
});

test('a failed installation gives one message and keeps Codex-only setup working', async () => {
  for (const [platform, command] of [['win32', /PowerShell: +irm https:\/\/claude\.ai\/install\.ps1 \| iex/],
    ['darwin', /curl -fsSL https:\/\/claude\.ai\/install\.sh \| bash/]]) {
    const ui = prompts();
    const pending = startClaude(ui, [codex], { platform, arch: 'arm64',
      install: async () => { throw new Error('Could not reach downloads.claude.ai. Check the internet connection.'); },
      detect: async () => assert.fail('nothing to detect') });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(await pending.finish(), [codex]);
    assert.equal(ui.warned.length, 1);
    assert.match(ui.warned[0], /Could not reach downloads\.claude\.ai/);
    assert.match(ui.warned[0], command);
    assert.match(ui.warned[0], /continues without Claude Code/i);
    const alone = prompts();
    const none = startClaude(alone, [], { platform, arch: 'arm64', install: async () => { throw new Error('HTTP 403'); } });
    assert.deepEqual(await none.finish(), []);
    assert.doesNotMatch(alone.warned[0], /continues without/);
  }
});

test('cancelling setup stops the background installation', async () => {
  const ui = prompts();
  let seen;
  const pending = startClaude(ui, [codex], { platform: 'win32', arch: 'x64', install: ({ signal }) => {
    seen = signal;
    return new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(new Error('Setup cancelled'))));
  } });
  assert.equal(seen.aborted, false);
  await pending.cancel();
  assert.equal(seen.aborted, true);
  await assert.rejects(pending.finish(), /Setup cancelled/);
  assert.deepEqual(ui.warned, []);
});

test('an aborted download removes the partial file and never runs the installer', async t => {
  for (const settings of [base, mac]) {
    const { home, runs, runInstaller } = await profile(t, settings === mac ? 'claude' : 'claude.exe');
    const controller = new AbortController();
    const { fetcher } = release();
    const cut = async (url, options) => {
      if (!/\/claude(\.exe)?$/.test(url)) return fetcher(url, options);
      assert.ok(options.signal);
      return new Response(new ReadableStream({
        start(stream) { stream.enqueue(BINARY.subarray(0, 4)); },
        pull(stream) { controller.abort(); stream.error(new DOMException('aborted', 'AbortError')); },
      }));
    };
    await assert.rejects(installClaude({ ...settings, home, fetcher: cut, runInstaller, signal: controller.signal }), /Setup cancelled/);
    assert.deepEqual(runs, []);
    assert.deepEqual(await readdir(join(home, '.claude', 'downloads')), []);
  }
});

test('the installer is stopped by the same cancellation', async t => {
  const { home } = await profile(t);
  const controller = new AbortController();
  let passed;
  await assert.rejects(installClaude({ ...base, home, fetcher: release().fetcher, signal: controller.signal,
    runInstaller: async (path, args, options) => { passed = options.signal; controller.abort(); throw new Error('killed'); } }),
  /Setup cancelled/);
  assert.equal(passed, controller.signal);
});

test('previews say setup installs a missing Claude Code and never download', async () => {
  const { checks } = await import('../scripts/setup/view.mjs');
  const host = { platform: 'win32', arch: 'arm64', cpuCount: 8, memoryBytes: 2 ** 34, freeBytes: 2 ** 36 };
  const tools = { node: 'v24.21.0', cargo: null, seatbelt: false };
  assert.match(checks({ host, providers: [codex], tools }), /Claude Code\s+not found, setup installs it automatically from Anthropic/);
  assert.match(checks({ host: { ...host, platform: 'darwin' }, providers: [], tools }), /Claude Code\s+not found, setup installs it automatically/);
  assert.doesNotMatch(checks({ host, providers: [codex, claude], tools }), /automatically/);
  assert.doesNotMatch(checks({ host: { ...host, arch: 'ia32' }, providers: [], tools }), /automatically/);
  const wizard = await readFile(new URL('../scripts/setup/wizard.mjs', import.meta.url), 'utf8');
  const preview = wizard.slice(wizard.indexOf('if (preview) {'), wizard.indexOf('Preview only. Nothing was changed'));
  assert.ok(preview.includes('claudeInstallable('));
  assert.equal(/startClaude|installClaude/.test(preview), false);
});

test('WinGet and npm installations of Claude Code are found as native executables', async () => {
  const seen = {};
  await detectProviders({ platform: 'win32', arch: 'arm64', home: 'C:\\Users\\S',
    env: { APPDATA: 'C:\\Users\\S\\AppData\\Roaming', LOCALAPPDATA: 'C:\\Users\\S\\AppData\\Local', PATH: '' },
    find: async (name, candidates) => { seen[name] = candidates; return null; } });
  assert.ok(seen.claude.includes('C:\\Users\\S\\AppData\\Local\\Microsoft\\WinGet\\Links\\claude.exe'));
  assert.ok(seen.claude.includes('C:\\Users\\S\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\node_modules\\@anthropic-ai\\claude-code-win32-arm64\\claude.exe'));
  assert.equal(seen.claude.some(path => !path.endsWith('.exe')), false);
});

test('both release notes explain the automatic installation and the Mac note keeps its wording', async () => {
  const source = await readFile(new URL('../scripts/package-release.mjs', import.meta.url), 'utf8');
  assert.ok(source.includes('Claude Code is not bundled. If it is missing, setup downloads and installs it from Anthropic'));
  assert.ok(source.includes('Nothing else needs to be installed. MiniClaw uses your existing Codex and Claude Code sign-ins.'));
  assert.ok(source.includes('If Claude Code is missing, setup downloads and installs it from Anthropic (downloads.claude.ai) by itself.'));
});

test('only a terminal raised above the user\'s normal rights counts as administrator', () => {
  const high = 'Mandatory Label\\High Mandatory Level  Label  S-1-16-12288';
  const medium = 'Mandatory Label\\Medium Mandatory Level  Label  S-1-16-8192';
  const on = '    EnableLUA    REG_DWORD    0x1', off = '    EnableLUA    REG_DWORD    0x0';
  assert.equal(administratorTerminal(high, on), true);
  assert.equal(administratorTerminal(high, ''), true);
  assert.equal(administratorTerminal(medium, on), false);
  // With User Account Control off, an administrator has no lower token to run with.
  assert.equal(administratorTerminal(high, off), false);
  assert.equal(administratorTerminal('S-1-16-16384', off), true);
});
