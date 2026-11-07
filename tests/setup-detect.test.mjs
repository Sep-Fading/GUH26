// Checks what setup finds by itself before asking, with fake file systems and package queries.
import assert from 'node:assert/strict';
// Runs detection without touching this machine's real apps.
import { test } from 'node:test';
// Reads the fixed detection script to check it takes no input.
import { readFile } from 'node:fs/promises';
// The detectors under test.
import { findWindowsComputerHelper, findMacComputerApp, detectSetupValues } from '../scripts/setup/detect.mjs';

const cua = 'app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows';
const sky = 'app\\resources\\cua_node\\bin\\node_modules\\@oai\\sky\\bin\\windows';
// A fake file system that only knows the listed files.
const files = list => async path => list.includes(path);

test('the Windows helper is found inside the newest installed Codex package for this CPU', async () => {
  const old = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.900.1.0_arm64__2p2nqsd0c76g0';
  const current = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0';
  const packages = async () => ({ codex: [{ location: old, version: '26.900.1.0', architecture: 'Arm64' },
    { location: current, version: '26.930.3930.0', architecture: 'Arm64' }] });
  const exists = files([`${old}\\${cua}\\codex-computer-use-arm64.exe`, `${current}\\${cua}\\codex-computer-use-arm64.exe`,
    `${current}\\${sky}\\codex-computer-use-arm64.exe`]);
  assert.equal(await findWindowsComputerHelper({ arch: 'arm64', query: packages, exists }),
    `${current}\\${cua}\\codex-computer-use-arm64.exe`);
});

test('an x64 PC gets codex-computer-use.exe, and the sky copy is the fallback', async () => {
  const folder = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_x64__2p2nqsd0c76g0';
  const query = async () => ({ codex: [{ location: folder, version: '26.930.3930.0', architecture: 'X64' }] });
  assert.equal(await findWindowsComputerHelper({ arch: 'x64', query, exists: files([`${folder}\\${sky}\\codex-computer-use.exe`]) }),
    `${folder}\\${sky}\\codex-computer-use.exe`);
  // The ARM64 file is never offered to an x64 PC.
  assert.equal(await findWindowsComputerHelper({ arch: 'x64', query, exists: files([`${folder}\\${cua}\\codex-computer-use-arm64.exe`]) }), null);
});

test('no package, a failing query or a relative location finds nothing', async () => {
  assert.equal(await findWindowsComputerHelper({ arch: 'arm64', query: async () => ({ codex: [] }), exists: files([]) }), null);
  assert.equal(await findWindowsComputerHelper({ arch: 'arm64', query: async () => { throw new Error('no powershell'); }, exists: files([]) }), null);
  assert.equal(await findWindowsComputerHelper({ arch: 'arm64', query: async () => ({ codex: [{ location: 'relative\\folder' }] }),
    exists: async () => true }), null);
});

test('the Mac Computer Use app is found in ~/.codex/computer-use', async () => {
  const path = '/Users/sam/.codex/computer-use/Codex Computer Use.app';
  assert.equal(await findMacComputerApp({ home: '/Users/sam', exists: files([path]) }), path);
  assert.equal(await findMacComputerApp({ home: '/Users/sam', exists: files([]) }), null);
});

test('detection runs one fixed read-only script on Windows and none on the Mac', async () => {
  let calls = 0;
  const query = async () => { calls++; return { codex: [] }; };
  const mac = await detectSetupValues({ platform: 'darwin', arch: 'arm64', home: '/Users/sam', exists: files([]), query });
  assert.equal(calls, 0);
  assert.deepEqual(Object.keys(mac).sort(), ['computerApp', 'windowsComputerHelper']);
  await detectSetupValues({ platform: 'win32', arch: 'arm64', env: {}, exists: files([]), query });
  assert.equal(calls, 1, 'one PowerShell run finds the helper');
  const script = await readFile(new URL('../scripts/setup/windows-detect.ps1', import.meta.url), 'utf8');
  assert.match(script, /Get-AppxPackage -Name 'OpenAI\.Codex'/);
  assert.doesNotMatch(script, /param\s*\(|\$args|Invoke-Expression|iex\b|Set-|Remove-|New-Item/);
});
