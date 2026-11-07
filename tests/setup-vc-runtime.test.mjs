// Checks how setup finds and installs Microsoft's Visual C++ runtime, without network access,
// registry access or a real installer.
import assert from 'node:assert/strict';
// Registers each runtime case.
import { test } from 'node:test';
// Computes the digest the elevated step must re-check.
import { createHash } from 'node:crypto';
// Builds disposable download folders and installation fixtures.
import { copyFile, mkdtemp, mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
// Loads the step under test.
import { VC_RUNTIME_MINIMUM, VC_RUNTIME_PAGE, compareVersions, downloadVcRuntime, ensureVcRuntime,
  installVcRuntime, parseRuntimeKey, startVcRuntime, vcRuntimeArch, vcRuntimeCommand, vcRuntimeStatus,
  vcRuntimeUrl } from '../scripts/setup/vc-runtime.mjs';
// The panels and the installation transaction that use it.
import { checks, review, stageList } from '../scripts/setup/view.mjs';
import { planSetup, GiB } from '../scripts/setup/plan.mjs';
import { install } from '../scripts/setup/install.mjs';
// On Windows the installation's private files are written by its built backend, as in production.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { nativeArtifacts } from './support/provider-fixture-paths.mjs';

const INSTALLER = Buffer.from('MZ fake Microsoft Visual C++ Redistributable');
const DIGEST = createHash('sha256').update(INSTALLER).digest('hex');
const MICROSOFT = { valid: true, subject: 'CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US',
  issuer: 'CN=Microsoft Code Signing PCA 2011, O=Microsoft Corporation, L=Redmond, S=Washington, C=US' };
const FINAL = 'https://download.visualstudio.microsoft.com/download/pr/ece44298/B70EF586/VC_redist.arm64.exe';

// reg.exe output for one Runtimes key, as Windows prints it.
function key(arch, { installed = 1, major = 14, minor = 51, bld = 36247, rbld = 0 } = {}) {
  const hex = value => `0x${value.toString(16)}`;
  return [`HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\${arch}`,
    `    Installed    REG_DWORD    ${hex(installed)}`, `    Version    REG_SZ    v${major}.${minor}.${bld}.00`,
    `    Major    REG_DWORD    ${hex(major)}`, `    Minor    REG_DWORD    ${hex(minor)}`,
    `    Bld    REG_DWORD    ${hex(bld)}`, `    Rbld    REG_DWORD    ${hex(rbld)}`, ''].join('\r\n');
}

// Serves Microsoft's permalink chain: two aka.ms redirects, then the file on the Visual Studio download host.
function microsoft({ body = INSTALLER, routes = {}, length } = {}) {
  const seen = [];
  const chain = {
    'https://aka.ms/vc14/vc_redist.arm64.exe': () => new Response(null, { status: 301, headers: { location: 'https://aka.ms/vs/18/release/vc_redist.arm64.exe' } }),
    'https://aka.ms/vs/18/release/vc_redist.arm64.exe': () => new Response(null, { status: 301, headers: { location: FINAL } }),
    [FINAL]: () => new Response(body, { headers: length === undefined ? {} : { 'content-length': String(length) } }),
    ...routes,
  };
  const fetcher = async (url, options) => {
    seen.push([String(url), options?.redirect]);
    return chain[String(url)]?.() ?? new Response('missing', { status: 404 });
  };
  return { fetcher, seen };
}

// A private download folder that disappears after the test.
async function folder(t) {
  const path = await mkdtemp(join(tmpdir(), 'mc-vc-runtime-'));
  t.after(() => rm(path, { recursive: true, force: true }));
  return path;
}

test('the runtime matches the release architecture and comes from Microsoft\'s documented permalinks', () => {
  assert.equal(vcRuntimeArch('win32', 'arm64'), 'arm64');
  assert.equal(vcRuntimeArch('win32', 'x64'), 'x64');
  for (const [platform, arch] of [['darwin', 'arm64'], ['linux', 'x64'], ['win32', 'ia32'], ['win32', '__proto__']]) {
    assert.equal(vcRuntimeArch(platform, arch), null);
  }
  assert.equal(vcRuntimeUrl('arm64'), 'https://aka.ms/vc14/vc_redist.arm64.exe');
  assert.equal(vcRuntimeUrl('x64'), 'https://aka.ms/vc14/vc_redist.x64.exe');
  assert.throws(() => vcRuntimeUrl('x86'), /architecture/);
  assert.equal(VC_RUNTIME_PAGE, 'https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist');
  // MiniClaw's Windows binaries were built with MSVC 14.51.36231.
  assert.equal(VC_RUNTIME_MINIMUM, '14.51.36231');
});

test('registry values are read from the documented Runtimes key, not from the display string', () => {
  assert.equal(parseRuntimeKey(key('arm64')), '14.51.36247.0');
  assert.equal(parseRuntimeKey(key('x64', { minor: 42, bld: 34438 })), '14.42.34438.0');
  assert.equal(parseRuntimeKey(key('arm64', { installed: 0 })), null);
  assert.equal(parseRuntimeKey(key('arm64').replace(/^\s+Bld.*$/m, '')), null);
  assert.equal(parseRuntimeKey(''), null);
  assert.equal(parseRuntimeKey('ERROR: The system was unable to find the specified registry key or value.'), null);
  assert.equal(compareVersions('14.51.36247.0', '14.51.36231'), 1);
  assert.equal(compareVersions('14.51.36231', '14.51.36231.0'), 0);
  assert.equal(compareVersions('14.42.34438.0', '14.51.36231'), -1);
  assert.equal(compareVersions('14.100.1', '14.51.36231'), 1);
});

test('both registry views are queried with the system reg.exe and the newer runtime counts', async () => {
  const asked = [];
  const query = views => async (file, args) => {
    asked.push([file, ...args]);
    const text = views[args.at(-1)];
    if (text === undefined) throw Object.assign(new Error('not found'), { code: 1 });
    return text;
  };
  const both = await vcRuntimeStatus({ arch: 'arm64', systemRoot: 'C:\\Windows',
    query: query({ '/reg:32': key('arm64'), '/reg:64': key('arm64', { minor: 42, bld: 34438 }) }) });
  assert.deepEqual(both, { arch: 'arm64', version: '14.51.36247.0', sufficient: true });
  assert.deepEqual(asked.map(item => item.slice(0, 3)), [
    ['C:\\Windows\\System32\\reg.exe', 'query', 'HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\arm64'],
    ['C:\\Windows\\System32\\reg.exe', 'query', 'HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\arm64']]);
  assert.deepEqual(asked.map(item => item.at(-1)).sort(), ['/reg:32', '/reg:64']);
  assert.deepEqual(await vcRuntimeStatus({ arch: 'x64', query: query({}) }), { arch: 'x64', version: null, sufficient: false });
  assert.deepEqual(await vcRuntimeStatus({ arch: 'x64', query: query({ '/reg:32': key('x64', { minor: 42, bld: 34438 }) }) }),
    { arch: 'x64', version: '14.42.34438.0', sufficient: false });
  assert.deepEqual(await vcRuntimeStatus({ arch: 'arm64', query: query({ '/reg:64': key('arm64', { bld: 36231 }) }) }),
    { arch: 'arm64', version: '14.51.36231.0', sufficient: true });
});

test('the installer is fetched over HTTPS through Microsoft hosts only and checked for Microsoft\'s signature', async t => {
  const path = await folder(t);
  const { fetcher, seen } = microsoft();
  const verified = [], progress = [];
  const result = await downloadVcRuntime({ arch: 'arm64', folder: path, fetcher,
    verifySignature: async file => { verified.push([file, (await readFile(file)).equals(INSTALLER)]); return MICROSOFT; },
    onProgress: (received) => progress.push(received) });
  assert.deepEqual(seen, [['https://aka.ms/vc14/vc_redist.arm64.exe', 'manual'],
    ['https://aka.ms/vs/18/release/vc_redist.arm64.exe', 'manual'], [FINAL, 'manual']]);
  assert.equal(result.arch, 'arm64');
  assert.equal(result.installer, join(path, 'VC_redist.arm64.exe'));
  assert.equal(result.sha256, DIGEST);
  assert.equal(result.bytes, INSTALLER.length);
  assert.deepEqual(verified, [[result.installer, true]]);
  assert.equal(progress.at(-1), INSTALLER.length);
});

test('a redirect away from Microsoft, plain HTTP or an endless chain never saves a file', async t => {
  for (const location of ['https://download.visualstudio.microsoft.com.evil.example/VC_redist.arm64.exe',
    'http://download.visualstudio.microsoft.com/VC_redist.arm64.exe', 'https://user@aka.ms/vc14/x.exe',
    'https://download.microsoft.com:8443/VC_redist.arm64.exe', 'https://github.com/VC_redist.arm64.exe']) {
    const path = await folder(t);
    const { fetcher } = microsoft({ routes: { 'https://aka.ms/vs/18/release/vc_redist.arm64.exe': () =>
      new Response(null, { status: 302, headers: { location } }) } });
    await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher, verifySignature: () => assert.fail('no file') }),
      /not a Microsoft download host/, location);
    assert.deepEqual(await readdir(path), []);
  }
  const path = await folder(t);
  const loop = microsoft({ routes: { [FINAL]: () => new Response(null, { status: 307, headers: { location: 'https://aka.ms/vc14/vc_redist.arm64.exe' } }) } });
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: loop.fetcher }), /Too many redirects/);
  const missing = microsoft({ routes: { [FINAL]: () => new Response('gone', { status: 404 }) } });
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: missing.fetcher }), /HTTP 404/);
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: async () => { throw new TypeError('fetch failed'); } }),
    /Could not reach Microsoft's download server.*internet connection/);
  assert.deepEqual(await readdir(path), []);
});

test('the download is size-bounded before and while it streams', async t => {
  const path = await folder(t);
  const big = microsoft({ length: 65 * 1024 ** 2 });
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: big.fetcher }), /larger than expected/);
  const endless = microsoft({ body: new ReadableStream({ pull(stream) { stream.enqueue(new Uint8Array(4 * 1024 ** 2)); } }) });
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: endless.fetcher, maxBytes: 16 * 1024 ** 2 }), /larger than expected/);
  assert.deepEqual(await readdir(path), []);
});

test('an unsigned, invalid or non-Microsoft signature removes the download and runs nothing', async t => {
  for (const signature of [{ valid: false, subject: MICROSOFT.subject, issuer: MICROSOFT.issuer },
    { valid: true, subject: 'CN=Contoso, O=Contoso', issuer: MICROSOFT.issuer },
    { valid: true, subject: 'CN=Microsoft Corporation, O=Microsoft Corporation Fake', issuer: MICROSOFT.issuer },
    { valid: true, subject: MICROSOFT.subject, issuer: 'CN=Some CA, O=Some CA' },
    { valid: true, subject: MICROSOFT.subject }]) {
    const path = await folder(t);
    await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: microsoft().fetcher, verifySignature: async () => signature }),
      /signature/, JSON.stringify(signature));
    assert.deepEqual(await readdir(path), []);
  }
  const path = await folder(t);
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: microsoft().fetcher,
    verifySignature: async () => { throw new Error('powershell failed'); } }), /signature/);
  assert.deepEqual(await readdir(path), []);
});

test('cancelling the download removes the partial file', async t => {
  const path = await folder(t);
  const controller = new AbortController();
  const cut = microsoft({ body: new ReadableStream({
    start(stream) { stream.enqueue(INSTALLER.subarray(0, 4)); },
    pull(stream) { controller.abort(); stream.error(new DOMException('aborted', 'AbortError')); } }) });
  await assert.rejects(downloadVcRuntime({ arch: 'arm64', folder: path, fetcher: cut.fetcher, signal: controller.signal }), /Setup cancelled/);
  assert.deepEqual(await readdir(path), []);
});

test('the elevated step receives only the verified path and digest and re-checks both before running Microsoft\'s installer', async () => {
  const script = await readFile(new URL('../scripts/setup/windows-vc-runtime-install.ps1', import.meta.url), 'utf8');
  const installer = 'C:\\Users\\S\\AppData\\Local\\Temp\\miniclaw-vc-runtime-a1\\VC_redist.arm64.exe';
  const command = vcRuntimeCommand({ installer, sha256: DIGEST, scriptBody: script, systemRoot: 'C:\\Windows' });
  assert.equal(command.file, 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe');
  assert.deepEqual(command.args.slice(0, 4), ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command']);
  const outer = command.args[4];
  assert.match(outer, /Start-Process -FilePath 'C:\\Windows\\System32\\WindowsPowerShell\\v1\.0\\powershell\.exe' -Verb RunAs -Wait -PassThru/);
  // A declined approval prompt is reported as ERROR_CANCELLED.
  assert.match(outer, /catch \{ exit 1223 \}/);
  assert.ok(outer.length < 30000);
  const inner = Buffer.from(/-EncodedCommand ([A-Za-z0-9+/=]+)/.exec(outer)[1], 'base64').toString('utf16le');
  assert.ok(inner.includes(script));
  assert.ok(inner.endsWith(`-Source '${installer}' -ExpectedSha256 '${DIGEST}'`));
  for (const bad of ['C:\\Temp\\a\'b.exe', '\\\\server\\share\\x.exe', 'relative\\x.exe', 'C:\\Temp\\..\\x.exe', 'C:\\Temp\\x.exe:stream']) {
    assert.throws(() => vcRuntimeCommand({ installer: bad, sha256: DIGEST, scriptBody: script }), /local absolute/, bad);
  }
  assert.throws(() => vcRuntimeCommand({ installer, sha256: 'A'.repeat(64), scriptBody: script }), /digest/);
  // The elevated side copies into an administrators-only folder, re-hashes the bytes it copies,
  // re-checks Microsoft's signature on that copy and runs only that copy, silently and without restarting.
  assert.match(script, /\[IO\.Directory\]::CreateDirectory\(\$folder, \$security\)/);
  assert.match(script, /SetAccessRuleProtection\(\$true, \$false\)/);
  assert.match(script, /'S-1-5-18', 'S-1-5-32-544'/);
  assert.match(script, /SHA256/);
  assert.match(script, /Get-AuthenticodeSignature -LiteralPath \$copy/);
  assert.match(script, /O=Microsoft Corporation/);
  assert.match(script, /Start-Process -FilePath \$copy -ArgumentList '\/install', '\/quiet', '\/norestart' -Wait -PassThru/);
  assert.doesNotMatch(script, /Invoke-Expression|iex\b|Set-ExecutionPolicy|DownloadFile|Invoke-WebRequest/);
});

test('installer exit codes: success, already newer and restart pending continue; everything else stops with the official page', async () => {
  const sufficient = async () => ({ arch: 'arm64', version: '14.51.36247.0', sufficient: true });
  const fail = code => async () => { throw Object.assign(new Error(`exit ${code}`), { code }); };
  const download = { arch: 'arm64', installer: 'C:\\Temp\\mc\\VC_redist.arm64.exe', sha256: DIGEST };
  const ran = [];
  const ok = await installVcRuntime({ ...download, scriptBody: 'param()', run: async (file, args) => { ran.push([file, args]); }, status: sufficient });
  assert.deepEqual(ok, { version: '14.51.36247.0', restart: false });
  assert.equal(ran.length, 1);
  assert.deepEqual(await installVcRuntime({ ...download, scriptBody: 'param()', run: fail(1638), status: sufficient }), { version: '14.51.36247.0', restart: false });
  for (const code of [3010, 1641]) {
    assert.deepEqual(await installVcRuntime({ ...download, scriptBody: 'param()', run: fail(code), status: sufficient }), { version: '14.51.36247.0', restart: true });
  }
  for (const [code, text] of [[1223, /administrator approval was declined/], [1602, /administrator approval was declined/],
    [1618, /Another installation is in progress/], [1603, /exit code 1603/], [536870913, /did not match the verified download/],
    [536870914, /not signed by Microsoft/]]) {
    const error = await installVcRuntime({ ...download, scriptBody: 'param()', run: fail(code), status: sufficient }).catch(error => error);
    assert.match(error.message, text, String(code));
    assert.ok(error.message.includes(VC_RUNTIME_PAGE), String(code));
  }
  // The architecture is the release target and is never assumed.
  for (const arch of [undefined, null, 'ia32', '__proto__']) {
    await assert.rejects(installVcRuntime({ ...download, arch, scriptBody: 'param()', run: () => assert.fail('no run'), status: sufficient }), /architecture/, String(arch));
  }
  // A reported success that leaves no sufficient runtime is a failure.
  await assert.rejects(installVcRuntime({ ...download, scriptBody: 'param()', run: async () => {},
    status: async () => ({ arch: 'arm64', version: '14.42.34438.0', sufficient: false }) }), /still not installed.*14\.51\.36231/);
});

// Records prompts and messages like the wizard's terminal interface.
function prompts() {
  const asked = [], warned = [], spun = [], told = [];
  return { asked, warned, spun, told, isCancel: () => false,
    confirm: async options => { asked.push(options); return true; },
    spinner: () => ({ start: text => spun.push(text), message: text => spun.push(text), stop: text => spun.push(text) }),
    log: { warn: text => warned.push(text), info: text => told.push(text), success: text => spun.push(text) } };
}

const present = { arch: 'arm64', version: '14.51.36247.0', sufficient: true };
const absent = { arch: 'arm64', version: null, sufficient: false };

test('a present runtime or a Mac starts nothing and says nothing', () => {
  const never = () => assert.fail('must not download');
  for (const [platform, status] of [['win32', present], ['darwin', absent], ['darwin', null]]) {
    const ui = prompts();
    assert.equal(startVcRuntime(ui, status, { platform, download: never }), null);
    assert.deepEqual([ui.asked, ui.told, ui.spun, ui.warned], [[], [], [], []]);
  }
});

test('a missing runtime downloads in the background after one notice and no question', async () => {
  for (const status of [absent, { ...absent, version: '14.42.34438.0' }]) {
    const ui = prompts();
    const calls = [];
    let settle;
    const pending = startVcRuntime(ui, status, { platform: 'win32',
      download: options => { calls.push(options); return new Promise(resolve => { settle = resolve; }); } });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].arch, 'arm64');
    assert.ok(calls[0].signal instanceof AbortSignal);
    assert.equal(ui.asked.length, 0);
    assert.equal(ui.told.length, 1);
    assert.match(ui.told[0], /Microsoft Visual C\+\+ runtime.*(not installed|older).*downloading Microsoft's installer \(aka\.ms\/vc14\).*administrator approval/s);
    const waiting = pending.finish();
    await new Promise(resolve => setImmediate(resolve));
    const download = { arch: 'arm64', installer: 'C:\\Temp\\VC_redist.arm64.exe', sha256: DIGEST, bytes: INSTALLER.length };
    settle(download);
    assert.deepEqual(await waiting, download);
    assert.deepEqual(ui.warned, []);
  }
});

test('a failed download stops setup before anything is changed and names the official page', async () => {
  const ui = prompts();
  const pending = startVcRuntime(ui, absent, { platform: 'win32',
    download: async () => { throw new Error('Could not reach Microsoft\'s download server (fetch failed). Check the internet connection.'); } });
  const error = await pending.finish().catch(error => error);
  assert.match(error.message, /Could not reach Microsoft's download server/);
  assert.ok(error.message.includes(VC_RUNTIME_PAGE));
  assert.match(error.message, /arm64/);
  assert.match(error.message, /Nothing was changed/);
});

test('cancelling setup stops the background download', async () => {
  let seen;
  const pending = startVcRuntime(prompts(), absent, { platform: 'win32', download: ({ signal }) => {
    seen = signal;
    return new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(new Error('Setup cancelled'))));
  } });
  await pending.cancel();
  assert.equal(seen.aborted, true);
  await assert.rejects(pending.finish(), /Setup cancelled/);
});

test('ensureVcRuntime downloads nothing when the runtime is present, otherwise installs it and removes the download', async t => {
  const root = await folder(t);
  const output = [];
  const nothing = await ensureVcRuntime({ arch: 'arm64', status: async () => present, onOutput: text => output.push(text),
    download: () => assert.fail('no download'), install: () => assert.fail('no install') });
  assert.deepEqual(nothing, { installed: false, version: '14.51.36247.0', restart: false });
  assert.deepEqual(output, []);
  const order = [];
  let downloadFolder;
  const result = await ensureVcRuntime({ arch: 'arm64', temp: root, status: async () => absent, onOutput: text => output.push(text),
    download: async ({ arch, folder: into }) => {
      downloadFolder = into; order.push(`download:${arch}`);
      await writeFile(join(into, 'VC_redist.arm64.exe'), INSTALLER);
      return { arch, installer: join(into, 'VC_redist.arm64.exe'), sha256: DIGEST, bytes: INSTALLER.length };
    },
    install: async ({ installer, sha256 }) => { order.push(`install:${sha256 === DIGEST}`); assert.ok((await stat(installer)).isFile()); return { version: '14.51.36247.0', restart: true }; } });
  assert.deepEqual(order, ['download:arm64', 'install:true']);
  assert.deepEqual(result, { installed: true, version: '14.51.36247.0', restart: true });
  assert.equal(dirname(downloadFolder), root);
  await assert.rejects(stat(downloadFolder), { code: 'ENOENT' });
  assert.match(output.join(''), /administrator approval/);
  assert.match(output.join(''), /restart/i);
  const failed = await ensureVcRuntime({ arch: 'arm64', temp: root, status: async () => absent,
    download: async ({ folder: into }) => { downloadFolder = into; throw new Error('HTTP 503'); } }).catch(error => error);
  assert.match(failed.message, /HTTP 503/);
  assert.ok(failed.message.includes(VC_RUNTIME_PAGE));
  await assert.rejects(stat(downloadFolder), { code: 'ENOENT' });
  assert.deepEqual(await readdir(root), []);
});

const winHost = { platform: 'win32', arch: 'arm64', cpuCount: 8, memoryBytes: 16 * GiB, diskBytes: 512 * GiB, freeBytes: 100 * GiB };
const choice = { port: 8787, providers: { codex: true, claude: false }, vmEnabled: false, master: null };
const pendingRuntime = { arch: 'arm64', installer: 'C:\\Temp\\mc\\VC_redist.arm64.exe', sha256: DIGEST, bytes: INSTALLER.length };

test('the checks panel and the preview report the runtime; the Mac panel does not mention it', async () => {
  const tools = { node: 'v24.21.0', cargo: null, seatbelt: false };
  const providers = [{ id: 'codex', name: 'Codex (ChatGPT)', path: 'C:\\codex.exe', signedIn: true }];
  assert.match(checks({ host: winHost, providers, tools: { ...tools, vcRuntime: present } }), /Microsoft Visual C\+\+ runtime\s+installed \(14\.51\.36247\.0\)/);
  assert.match(checks({ host: winHost, providers, tools: { ...tools, vcRuntime: absent } }), /Microsoft Visual C\+\+ runtime\s+setup installs it automatically from Microsoft/);
  assert.match(checks({ host: winHost, providers, tools: { ...tools, vcRuntime: { ...absent, version: '14.42.34438.0' } } }),
    /Microsoft Visual C\+\+ runtime\s+14\.42\.34438\.0 is too old, setup installs it automatically from Microsoft/);
  assert.doesNotMatch(checks({ host: { ...winHost, platform: 'darwin' }, providers, tools }), /Visual C\+\+/);
  const wizard = await readFile(new URL('../scripts/setup/wizard.mjs', import.meta.url), 'utf8');
  const preview = wizard.slice(wizard.indexOf('if (preview) {'), wizard.indexOf('Preview only. Nothing was changed'));
  assert.ok(preview.includes('inspect('));
  assert.equal(/startVcRuntime|downloadVcRuntime|ensureVcRuntime|installVcRuntime/.test(preview), false);
  assert.ok(wizard.slice(wizard.indexOf('async function inspect'), wizard.indexOf('function launchBackend')).includes('vcRuntimeStatus('));
});

test('the reviewed plan shows the runtime step first, with administrator approval', () => {
  const plan = { ...planSetup(choice, winHost), vcRuntime: pendingRuntime };
  assert.deepEqual(stageList(plan), ['vc-runtime', 'build', 'windows-git', 'windows-network']);
  assert.deepEqual(stageList(planSetup(choice, winHost)), ['build', 'windows-git', 'windows-network']);
  assert.match(review(plan, { destination: 'C:\\MiniClaw', cache: 'C:\\cache' }),
    /Microsoft runtime\s+Visual C\+\+ Redistributable \(arm64\) from Microsoft, installed with administrator approval/);
  assert.doesNotMatch(review(planSetup(choice, winHost), { destination: 'C:\\MiniClaw', cache: 'C:\\cache' }), /Microsoft runtime/);
});

// The backend built for the native tests; only a Windows run copies it into an installation.
const nativeBackend = () => nativeArtifacts(fileURLToPath(new URL('../', import.meta.url)), process.env.MINICLAW_TEST_TARGET).backend;

// A Windows installation whose steps only record their order.
async function windowsInstall(t, overrides) {
  const root = await folder(t);
  const destination = join(root, 'installation');
  const calls = [];
  const options = { plan: { ...planSetup(choice, winHost), vcRuntime: pendingRuntime }, destination, cache: join(root, 'cache'),
    codexBin: '/test/codex', claudeBin: null, approved: true, checkPort: async () => {}, freeBytes: async () => winHost.freeBytes,
    onStage: stage => calls.push(`stage:${stage}`),
    installRuntime: async options => { calls.push(`runtime:${options.installer}:${options.sha256 === DIGEST}`); return { version: '14.51.36247.0', restart: false }; },
    buildHost: async path => {
      calls.push('build');
      await mkdir(join(path, 'bin'), { recursive: true });
      if (process.platform === 'win32') await copyFile(nativeBackend(), join(path, 'bin', 'miniclaw-server.exe'));
    },
    secureData: async (runtime, data) => {
      calls.push('secure');
      if (process.platform === 'win32') await promisify(execFile)(join(runtime, 'bin', 'miniclaw-server.exe'), ['--secure-data-dir', data]);
      else await mkdir(data, { mode: 0o700 });
    },
    installGit: async () => { calls.push('git'); },
    installNetwork: async () => { calls.push('network'); return { serviceName: `MiniClawNetwork-${'c'.repeat(32)}`, executable: 'C:\\x.exe' }; },
    ...overrides };
  return { destination, calls, options };
}

test('install runs Microsoft\'s installer before the backend is built or started', async t => {
  const { calls, options } = await windowsInstall(t);
  await install(options);
  assert.deepEqual(calls, ['stage:vc-runtime', `runtime:${pendingRuntime.installer}:true`, 'stage:build', 'build', 'secure', 'stage:windows-git', 'git', 'stage:windows-network', 'network']);
});

test('install passes the plan\'s architecture and refuses a download for another one', async t => {
  const seen = [];
  const { calls, options } = await windowsInstall(t, { installRuntime: async ({ arch }) => { seen.push(arch); return { version: '14.51.36247.0', restart: false }; } });
  await install(options);
  assert.deepEqual(seen, ['arm64']);
  const other = await windowsInstall(t, { plan: { ...planSetup(choice, winHost), vcRuntime: { ...pendingRuntime, arch: 'x64' } } });
  await assert.rejects(install(other.options), /another architecture/);
  assert.deepEqual(other.calls, []);
  assert.ok(calls.includes('build'));
});

test('a failed runtime installation stops before the backend and leaves nothing behind', async t => {
  const { destination, calls, options } = await windowsInstall(t, {
    installRuntime: async () => { throw new Error(`Setup could not install the Microsoft Visual C++ runtime. ${VC_RUNTIME_PAGE}`); } });
  await assert.rejects(install(options), /Visual C\+\+ runtime/);
  assert.deepEqual(calls, ['stage:vc-runtime']);
  await assert.rejects(readdir(destination), { code: 'ENOENT' });
});

test('the Windows release note says setup installs the Microsoft runtime; the Mac note does not', async () => {
  const { releaseReadme } = await import('../scripts/package-release.mjs');
  for (const arch of ['arm64', 'x64']) {
    const windows = releaseReadme({ vm: false, version: '1.0.0', arch });
    assert.match(windows, /Microsoft Visual C\+\+ runtime, which is not bundled\. If it is missing or too old, setup downloads it\nfrom Microsoft \(aka\.ms\/vc14\), checks Microsoft's signature and installs it, with one more administrator approval\./);
  }
  assert.doesNotMatch(releaseReadme({ vm: true, version: '1.0.0', arch: 'arm64' }), /Visual C\+\+/);
});

// The built archives live outside the repository; when present, they must match the pins and carry no Microsoft file.
test('the pinned Git archives contain no Microsoft Visual C++ runtime file', async t => {
  const { windowsGitBundle } = await import('../scripts/setup/windows-git-bundle.mjs');
  const { gitArchiveEntries } = await import('../scripts/setup/windows-git.mjs');
  const evidence = new URL('../../work/windows-vm/', import.meta.url);
  for (const arch of ['arm64', 'x64']) {
    const pin = windowsGitBundle(arch);
    const bytes = await readFile(new URL(pin.file, evidence)).catch(() => null);
    if (!bytes) { t.diagnostic(`${pin.file} not present; skipped`); continue; }
    assert.equal(bytes.length, pin.bytes);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), pin.digest);
    const names = [...gitArchiveEntries(bytes).keys()];
    // lpac5 adds licenses/git-lpac-console-handle.patch to the lpac4 file set.
    assert.equal(names.length, { arm64: 75, x64: 69 }[arch]);
    assert.ok(names.includes('git/licenses/git-lpac-console-handle.patch'));
    assert.deepEqual(names.filter(name => /(vcruntime|msvcp|concrt|vccorlib|ucrtbase|redist)/i.test(name)), []);
    assert.ok(names.includes('git/bin/git.exe') && names.includes('git/licenses/NOTICE.txt'));
  }
});
