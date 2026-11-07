// Checks the step-by-step guides for every value setup asks the user to find or create.
import assert from 'node:assert/strict';
// Runs guide checks without prompts or network.
import { test } from 'node:test';
// Compares the developer document with the shared guide text.
import { readFile } from 'node:fs/promises';
// The one guide module shared by setup, the CLI and the web page.
import { topics, guide, formatGuide, guideForValue, guideForServer, valueProblem,
  windowsHelperName, windowsHelperPathProblem, macComputerAppProblem, guidesMarkdown, unquote } from '../scripts/setup/guides.mjs';

const platforms = ['darwin', 'win32'];
// The whole text of a guide, one step per line, for content checks independent of console width.
const all = (id, ctx) => { const item = guide(id, ctx); return [...item.hint, ...item.steps, ...item.sources].join('\n'); };
// Official sources only: vendor documentation, never forums or third-party sites.
const official = /^https:\/\/(learn\.chatgpt\.com|developers\.openai\.com|docs\.claude\.com|learn\.microsoft\.com|support\.apple\.com)\//;

test('every topic has a short hint, numbered steps and official sources on both platforms', () => {
  assert.deepEqual(topics, ['computer-use', 'provider-sign-in']);
  for (const id of topics) for (const platform of platforms) {
    const item = guide(id, { platform });
    assert.equal(item.id, id);
    assert.ok(item.title.length > 5, `${id} title`);
    assert.ok(item.hint.length >= 1 && item.hint.length <= 3, `${id}/${platform}: ${item.hint.length} hint lines`);
    assert.ok(item.hint.every(line => line.length <= 110), `${id}/${platform}: a hint line is too wide`);
    assert.ok(item.steps.length >= 3, `${id}/${platform}: ${item.steps.length} steps`);
    assert.ok(item.sources.length >= 1, `${id}: sources`);
    for (const source of item.sources) assert.match(source, official, `${id}: ${source}`);
    assert.ok(item.asked && item.why, `${id}: inventory fields`);
  }
});

test('formatted guides are numbered and fit narrow consoles without breaking URLs', () => {
  for (const columns of [60, 80, 120]) for (const id of topics) for (const platform of platforms) {
    const item = guide(id, { platform, arch: 'arm64' });
    const text = formatGuide(item, { columns });
    const lines = text.split('\n');
    // Commands (the lines after a step's first) stay whole so they can be copied; a URL keeps its own line.
    const commands = new Set(item.steps.flatMap(step => step.split('\n').slice(1)));
    const long = line => line.length > columns - 8 && !/^\s*https:\/\/\S+$/.test(line) && !commands.has(line.trim());
    assert.ok(!lines.some(long), `${id}/${platform}/${columns}: ${lines.find(long)}`);
    for (const command of commands) assert.ok(lines.includes(`${' '.repeat(String(item.steps.length).length + 4)}${command}`)
      || lines.some(line => line.trim() === command), `${id}: command kept whole`);
    assert.match(text, /^1\. /m);
    assert.match(text, new RegExp(`^${item.steps.length}\\. `, 'm'));
    for (const source of item.sources) assert.ok(text.includes(source), `${id}: ${source} intact`);
  }
});

test('the Windows Computer Use guide names the package folder pattern and the file for this PC', () => {
  const arm = all('computer-use', { platform: 'win32', arch: 'arm64' });
  assert.match(arm, /C:\\Program Files\\WindowsApps\\OpenAI\.Codex_<version>_arm64__2p2nqsd0c76g0/);
  assert.match(arm, /app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows\\codex-computer-use-arm64\.exe/);
  assert.match(arm, /Get-AppxPackage -Name OpenAI\.Codex/);
  assert.match(arm, /Plugins > Computer Use/);
  assert.doesNotMatch(arm, /Finder|Keychain|\.app\b/);
  const x64 = all('computer-use', { platform: 'win32', arch: 'x64' });
  assert.match(x64, /OpenAI\.Codex_<version>_x64__2p2nqsd0c76g0/);
  assert.match(x64, /\\codex-computer-use\.exe/);
  assert.doesNotMatch(x64, /codex-computer-use-arm64\.exe/);
  assert.equal(windowsHelperName('arm64'), 'codex-computer-use-arm64.exe');
  assert.equal(windowsHelperName('x64'), 'codex-computer-use.exe');
  // MiniClaw finds the helper in the updated Codex app itself; setup need not run again.
  assert.match(arm, /After a Codex app update MiniClaw finds the helper in the new version/);
  assert.doesNotMatch(arm, /run miniclaw setup again/);
});

test('the macOS Computer Use guide points to the hidden ~/.codex folder and stays Mac-only', () => {
  const text = all('computer-use', { platform: 'darwin', home: '/Users/sam' });
  assert.match(text, /~\/\.codex\/computer-use\/Codex Computer Use\.app/);
  assert.match(text, /\/Users\/sam\/\.codex\/computer-use\/Codex Computer Use\.app/);
  assert.match(text, /Go > Go to Folder/);
  assert.match(text, /Screen Recording/);
  assert.doesNotMatch(text, /WindowsApps|PowerShell|\.exe\b|Credential Manager/);
});

test('no MCP server value or server has a guide or a known shape', () => {
  assert.equal(guideForValue('SOMETHING_ELSE'), null);
  assert.equal(guideForServer('other'), null);
  assert.equal(valueProblem('UNKNOWN_VALUE', 'anything at all'), null);
});

test('helper path checks explain the expected file and offer the guide', () => {
  assert.match(windowsHelperPathProblem('codex-computer-use-arm64.exe', { arch: 'arm64' }), /full path/i);
  const wrong = windowsHelperPathProblem('C:\\Program Files\\WindowsApps\\OpenAI.Codex_1_arm64__2p2nqsd0c76g0\\app\\Codex.exe', { arch: 'arm64' });
  assert.match(wrong, /codex-computer-use-arm64\.exe/);
  assert.match(wrong, /\?/);
  assert.equal(windowsHelperPathProblem('C:\\x\\codex-computer-use-arm64.exe', { arch: 'arm64' }), undefined);
  assert.equal(windowsHelperPathProblem('C:\\x\\codex-computer-use.exe', { arch: 'x64' }), undefined);
  assert.equal(windowsHelperPathProblem('?', { arch: 'x64' }), undefined);
  assert.ok(wrong.split('\n').every(line => line.length <= 110));
  assert.match(macComputerAppProblem('relative/Codex Computer Use.app'), /full path/i);
  assert.match(macComputerAppProblem('/Users/sam/.codex/computer-use/config.json'), /\.app/);
  assert.equal(macComputerAppProblem('/Users/sam/.codex/computer-use/Codex Computer Use.app'), undefined);
  assert.equal(unquote('  "C:\\a b\\c.exe" '), 'C:\\a b\\c.exe');
  assert.equal(unquote("'/x/y.app'"), '/x/y.app');
});

test('the developer inventory document is generated from the same guides', async () => {
  const saved = await readFile(new URL('../docs/setup-guides.md', import.meta.url), 'utf8');
  assert.equal(saved, guidesMarkdown(), 'run: node scripts/setup/guides.mjs --markdown > docs/setup-guides.md');
});

// A guide the wizard prints just before re-asking must fit a 120x30 console together with its question
// (VM1, 2026-10-06: the Windows helper guide pushed its own title off a 30-row console).
test('the guides the wizard prints before a question fit 20 rows of a 120-column console', () => {
  for (const [id, platform, arch] of [['computer-use', 'win32', 'arm64'], ['computer-use', 'win32', 'x64'], ['computer-use', 'win32', undefined],
    ['computer-use', 'darwin']]) {
    const rows = formatGuide(guide(id, { platform, arch }), { columns: 120 }).split('\n')
      .reduce((sum, line) => sum + Math.max(1, Math.ceil(line.length / 120)), 0);
    assert.ok(rows <= 20, `${id}/${platform}/${arch}: ${rows} rows`);
  }
});

// User report 2026-10-07: on Windows the Claude sign-in seemed to do nothing. The guide says what
// the page shows: the browser opens and the sign-in finishes by itself (lead decision 2026-10-07:
// the automatic flow on both platforms), with the link and the pasted code only as a fallback.
test('the provider sign-in guide describes what the Claude sign-in shows', () => {
  for (const platform of ['win32', 'darwin']) {
    const text = all('provider-sign-in', { platform });
    assert.match(text, /Claude Code opens its sign-in page/, platform);
    assert.match(text, /notices the sign-in by itself/, platform);
    assert.match(text, /Open the Claude sign-in page/, platform);
    assert.match(text, /Code from Claude/, platform);
    assert.match(text, /Finish sign-in/, platform);
  }
});
