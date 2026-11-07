// Checks what the wizard shows, without a terminal.
import assert from 'node:assert/strict';
// Runs each panel case in isolation.
import { test } from 'node:test';
// Builds real plans for the review panel.
import { planSetup, GiB } from '../scripts/setup/plan.mjs';
// Loads the pure panel builders under test.
import { budgetProblems, checks, failureNotice, machineName, review, nextSteps, stageLabel, stageList } from '../scripts/setup/view.mjs';

const host = { platform: 'darwin', arch: 'arm64', cpuCount: 12,
  memoryBytes: 32 * GiB, diskBytes: 1024 * GiB, freeBytes: 800 * GiB };
const providers = [{ id: 'codex', name: 'Codex (ChatGPT)', path: '/a/codex', signedIn: true },
  { id: 'claude', name: 'Claude Code', path: '/b/claude', signedIn: false }];
const base = { port: 8787, providers: { codex: true, claude: false }, vmEnabled: false, master: null };
const catalog = [{ id: 'macos', name: 'macOS restore image', bytes: 20 * GiB, expandedBytes: 0,
  url: 'https://example.test/macos.ipsw', algorithm: 'sha256' }];

test('the system check lists every requirement with an aligned status', () => {
  const text = checks({ host, providers, tools: { node: 'v24.1.0', cargo: '/usr/bin/cargo', seatbelt: true } });
  const lines = text.split('\n');
  assert.ok(lines.some(line => /Hardware\s+12 CPUs, 32\.00 GiB RAM, 800\.00 GiB free/.test(line)));
  assert.ok(lines.some(line => /Codex \(ChatGPT\)\s+signed in/.test(line)));
  assert.ok(lines.some(line => /Claude Code\s+found, not signed in/.test(line)));
  assert.ok(lines.some(line => /Seatbelt sandbox\s+available/.test(line)));
  // Every value starts in the same column.
  assert.equal(new Set(lines.map(line => line.match(/^.*?\s{3,}/)[0].length)).size, 1);
  assert.equal(text.includes('/a/codex'), false, 'paths stay out of the summary');
});

test('missing requirements are named as problems', () => {
  const text = checks({ host, providers: [], tools: { node: 'v24.1.0', cargo: null, seatbelt: false } });
  assert.match(text, /Rust \(cargo\)\s+missing/);
  assert.match(text, /Seatbelt sandbox\s+missing/);
  assert.match(text, /Providers\s+none found/);
});

test('with no provider, the checks do not ask for Claude Code by hand when setup installs it', () => {
  for (const platform of ['darwin', 'win32']) {
    const lines = checks({ host: { ...host, platform }, providers: [], tools: { node: 'v24.1.0', cargo: null, seatbelt: false } }).split('\n');
    const named = lines.find(line => /^Providers\s/.test(line));
    assert.match(named, /^Providers\s+none found yet, setup installs Claude Code$/, platform);
    assert.ok(lines.some(line => /^Claude Code\s+not found, setup installs it automatically from Anthropic$/.test(line)), platform);
  }
  // Where no Claude Code build exists, both providers still have to be installed by hand.
  const manual = checks({ host: { ...host, arch: 'x64' }, providers: [], tools: { node: 'v24.1.0', cargo: null, seatbelt: false } });
  assert.match(manual, /Providers\s+none found, install Codex or Claude Code/);
  assert.equal(/setup installs/.test(manual), false);
});

test('the review groups choices and shows no VM section without a VM', () => {
  const text = review(planSetup(base, host, catalog), { destination: '/x/MiniClaw', cache: '/x/cache' });
  assert.match(text, /Install to\s+\/x\/MiniClaw/);
  assert.match(text, /Interface\s+http:\/\/127\.0\.0\.1:8787/);
  assert.match(text, /Providers\s+Codex/);
  assert.match(text, /Sandbox\s+Seatbelt/);
  assert.match(text, /VM\s+none/);
  assert.equal(/download/i.test(text), false);
  assert.equal(/linux/i.test(text), false);
});

test('the review states the VM cost, downloads and checksums', () => {
  const plan = planSetup({ ...base, vmEnabled: true, master: { cpuCount: 2, memoryGiB: 8, diskGiB: 64 } }, host, catalog);
  const text = review(plan, { destination: '/x/MiniClaw', cache: '/x/cache' });
  assert.match(text, /VM\s+2 vCPUs, 8 GiB RAM, 64 GiB disk/);
  assert.match(text, /Downloads\s+20\.\d+ GiB/);
  assert.match(text, /Peak disk use\s+\d+\.\d+ GiB/);
  assert.match(text, /macOS restore image.*sha256/);
  assert.match(text, /boots only when a project asks/);
});

test('stage names are plain and the list matches the plan', () => {
  // macOS keeps its original wording; only Windows uses the neutral name.
  assert.equal(stageLabel('build', 'darwin'), 'Building MiniClaw for this Mac');
  assert.equal(stageLabel('build', 'win32'), 'Building MiniClaw for this computer');
  assert.equal(stageLabel('build'), `Building MiniClaw for ${process.platform === 'win32' ? 'this computer' : 'this Mac'}`);
  assert.equal(stageLabel('install-macos'), 'Installing macOS in the VM');
  assert.deepEqual(stageList(planSetup(base, host, catalog)), ['build']);
  const plan = planSetup({ ...base, vmEnabled: true, master: { cpuCount: 2, memoryGiB: 8, diskGiB: 64 } }, host, catalog);
  // The long macOS install is last, so no quick build can fail after it.
  assert.deepEqual(stageList(plan), ['download', 'build', 'build-worker', 'master-media', 'build-network', 'install-macos']);
});

test('next steps name the start command and any provider still to sign in', () => {
  const text = nextSteps({ destination: '/x/MiniClaw', port: 8787, providers,
    enabled: { codex: true, claude: true }, vm: false });
  // The wizard remembers the installation, so starting it needs no path.
  assert.match(text, /^Start\s+miniclaw$/m);
  assert.match(text, /http:\/\/127\.0\.0\.1:8787/);
  assert.match(text, /Claude Code.*sign in/i);
  assert.equal(/Codex.*sign in/i.test(text), false);
});

test('the review says what is already downloaded and what is left to fetch', () => {
  const plan = planSetup({ ...base, vmEnabled: true, master: { cpuCount: 2, memoryGiB: 8, diskGiB: 64 } }, host, catalog);
  const cached = new Set(['macos', 'codex']);
  const text = review(plan, { destination: '/x/MiniClaw', cache: '/x/cache', cached });
  assert.match(text, /macOS restore image.*already downloaded/);
  assert.match(text, /Downloads\s+6\d\.\d+ MiB left to fetch, 2 of 3 already in/);
  assert.equal(/20\.\d+ GiB into/.test(text), false, 'a cached image is not announced as a download');
});

test('the review shows the VM budget already in use and stops an install that cannot fit', () => {
  const plan = planSetup({ ...base, vmEnabled: true, master: { cpuCount: 2, memoryGiB: 8, diskGiB: 64 } }, host, catalog);
  const ledger = { used: { cpuCount: 3, memoryBytes: 10 * GiB, diskBytes: 100 * GiB },
    capacity: { cpuCount: 6, memoryBytes: 16 * GiB, diskBytes: 256 * GiB }, unknown: 2 };
  const text = review(plan, { destination: '/x/MiniClaw', cache: '/x/cache', ledger });
  assert.match(text, /Already reserved\s+3 CPU, 10\.0 GiB RAM, 100\.0 GiB disk by other VMs/);
  assert.deepEqual(budgetProblems(plan, ledger), ['RAM: 10.0 GiB reserved + 8.0 GiB new is over the 16.0 GiB budget']);
  assert.deepEqual(budgetProblems(plan, { ...ledger, used: { cpuCount: 0, memoryBytes: 0, diskBytes: 0 } }), []);
  assert.deepEqual(budgetProblems(planSetup(base, host, catalog), ledger), [], 'no VM, no budget question');
});

test('the machine is named a Mac on macOS and a computer on Windows', () => {
  assert.equal(machineName('darwin'), 'this Mac');
  assert.equal(machineName('win32'), 'this computer');
  assert.match(review(planSetup(base, host, catalog), { destination: '/x/MiniClaw', cache: '/x/cache' }), /\(this Mac only\)/);
  const windows = { ...planSetup(base, host, catalog), platform: 'win32', packages: [] };
  assert.match(review(windows, { destination: 'C:\\MiniClaw', cache: 'C:\\cache' }), /\(this computer only\)/);
});

test('a Windows plan lists the Git and network service stages it reports', () => {
  // Git is bundled with the release, so a Windows plan has nothing to download.
  const windows = { ...planSetup(base, host, catalog), platform: 'win32', packages: [],
    windowsGit: { id: 'windows-git', bytes: 1 } };
  assert.deepEqual(stageList(windows), ['build', 'windows-git', 'windows-network']);
});

test('a failed installation says whether its folder was removed or kept for resume', () => {
  assert.match(failureNotice(new Error('Build failed')), /Nothing was installed; the unfinished folder was removed/);
  const kept = failureNotice(Object.assign(new Error('Network service may be installed'), { retained: 'C:\\MiniClaw' }));
  assert.doesNotMatch(kept, /removed|Nothing was installed/);
  assert.match(kept, /kept at C:\\MiniClaw/);
  assert.match(kept, /miniclaw resume "C:\\MiniClaw"/);
});

// Since 2026-10-05 the Windows sandbox is available once setup has installed its network service;
// the checks and the review no longer say it stays disabled.
test('Windows checks and review describe the sandbox setup installs, not a disabled one', () => {
  const windowsHost = { ...host, platform: 'win32' };
  const text = checks({ host: windowsHost, providers, tools: { node: 'v24.1.0', cargo: null, vcRuntime: null } });
  const row = text.split('\n').find(line => /^Windows sandbox\s/.test(line));
  assert.match(row, /^Windows sandbox\s+AppContainer \+ WFP service, set up by this installation$/);
  const plan = { ...planSetup(base, host, catalog), platform: 'win32', packages: [] };
  const summary = review(plan, { destination: 'C:\\MiniClaw', cache: 'C:\\cache' });
  assert.match(summary, /Sandbox\s+AppContainer \+ WFP service: agent commands see only granted folders; network only through approved hosts/);
  assert.equal(/validation pending|remain disabled/.test(text + summary), false);
});

// A Windows release is started with its own launcher, from the folder setup ran from.
test('next steps name the release launcher on Windows', () => {
  const text = nextSteps({ destination: 'C:\\MiniClaw', port: 8787, providers, enabled: { codex: true, claude: true },
    vm: false, launcher: 'C:\\Users\\Ana\\Downloads\\MiniClaw é\\miniclaw.cmd' });
  assert.match(text, /^Start\s+\.\\miniclaw\.cmd  \(in C:\\Users\\Ana\\Downloads\\MiniClaw é\)$/m);
});

// A release carries its compiled backend, so the checks do not ask for Rust there (VM2 release run, 2026-10-05).
test('a release does not ask for Rust in the checks', () => {
  const text = checks({ host, providers, tools: { node: 'v24.1.0', cargo: null, seatbelt: true, prebuilt: true } });
  assert.match(text, /^Rust \(cargo\)\s+not needed, this release is prebuilt$/m);
  assert.equal(/install Rust/.test(text), false);
});

// A Windows release is not a source checkout, and says so; the Mac and a checkout keep their words.
test('setup help and the install choice name a release or a checkout as they are', async () => {
  const { setupHelp, installHint } = await import('../scripts/setup/view.mjs');
  const mac = setupHelp({ platform: 'darwin', release: false });
  assert.equal(mac[1], 'Installs MiniClaw on this Apple silicon Mac from the source checkout.');
  assert.equal(setupHelp({ platform: 'darwin', release: true })[1], mac[1]);
  assert.equal(setupHelp({ platform: 'win32', release: false })[1], 'Installs MiniClaw on this Windows computer from the source checkout.');
  const release = setupHelp({ platform: 'win32', release: true });
  assert.equal(release[1], 'Installs MiniClaw on this Windows computer from this release.');
  assert.ok(!release.join('\n').includes('checkout'));
  assert.equal(release[0], mac[0]);
  assert.equal(installHint({ platform: 'darwin', release: true }), 'builds from this checkout');
  assert.equal(installHint({ platform: 'win32', release: false }), 'builds from this checkout');
  assert.equal(installHint({ platform: 'win32', release: true }), 'installs this release');
});

// A second installation becomes the one commands without a path use; setup says so and names the earlier one.
test('setup names the earlier installation that commands without a path no longer use', async () => {
  const { rememberedNotice } = await import('../scripts/setup/view.mjs');
  const text = rememberedNotice('C:\\Users\\S\\AppData\\Local\\MiniClaw Install', 'D:\\MiniClaw Test');
  assert.match(text, /D:\\MiniClaw Test/);
  assert.match(text, /without a path/);
  assert.match(text, /miniclaw uninstall "C:\\Users\\S\\AppData\\Local\\MiniClaw Install"/);
  assert.equal(rememberedNotice(null, 'D:\\MiniClaw Test'), null);
  assert.equal(rememberedNotice('D:\\MiniClaw Test', 'd:\\miniclaw test', 'win32'), null);
});

// What setup found by itself is shown before any question, each row short enough for a narrow console.
test('the checks panel shows what was found and names the guide for what was not', async () => {
  const { checks } = await import('../scripts/setup/view.mjs');
  const tools = { node: 'v24.1.0', cargo: null, vcRuntime: null, prebuilt: true };
  const pc = { platform: 'win32', arch: 'arm64', cpuCount: 4, memoryBytes: 8 * 1024 ** 3, freeBytes: 50 * 1024 ** 3 };
  const found = checks({ host: pc, providers: [], tools, detected: { windowsComputerHelper: 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0\\app\\x\\codex-computer-use-arm64.exe' } });
  assert.match(found, /Computer Use helper\s+found in Codex app 26\.930\.3930\.0/);
  const missing = checks({ host: pc, providers: [], tools, detected: { windowsComputerHelper: null } });
  assert.match(missing, /Computer Use helper\s+not found; see: miniclaw guide computer-use/);
  const mac = checks({ host: { ...pc, platform: 'darwin' }, providers: [], tools: { ...tools, seatbelt: true },
    detected: { computerApp: '/Users/s/.codex/computer-use/Codex Computer Use.app' } });
  assert.match(mac, /Computer Use app\s+found in ~\/\.codex\/computer-use/);
  assert.doesNotMatch(mac, /WindowsApps|helper/);
  for (const line of [...found.split('\n'), ...missing.split('\n'), ...mac.split('\n')]) assert.ok(line.length <= 110, line);
});
