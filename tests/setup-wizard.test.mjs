// Checks the wizard's conditional questions and defaults.
import assert from 'node:assert/strict';
// Runs terminal workflow tests without input.
import { test } from 'node:test';
// Uses the same host-size unit convention.
import { GiB } from '../scripts/setup/plan.mjs';
// Collects wizard choices through injected prompts.
import { collectChoices, selectionLegend } from '../scripts/setup/questions.mjs';

const host = { platform: 'darwin', arch: 'arm64', cpuCount: 8,
  memoryBytes: 8 * GiB, diskBytes: 256 * GiB, freeBytes: 150 * GiB };
const large = { ...host, cpuCount: 12, memoryBytes: 32 * GiB, diskBytes: 1024 * GiB, freeBytes: 800 * GiB };
const found = [{ id: 'codex', name: 'Codex (ChatGPT)', path: '/test/codex', signedIn: true },
  { id: 'claude', name: 'Claude Code', path: '/test/claude', signedIn: false }];
const VM = 'Add an isolated macOS VM for untrusted projects?';
const HELPER = 'Connect the installed ChatGPT Windows Computer Use helper?';
const HELPER_PATH = 'Full path to codex-computer-use-arm64.exe (type ? for a step-by-step guide)';
const MAC_APP = 'Computer Use helper for the VM';
const MAC_APP_PATH = 'Full path to Codex Computer Use.app (type ? for a step-by-step guide)';

// Records questions and supplies explicit answers or their displayed recommendation.
function prompts(answers) {
  const asked = [];
  const answer = async options => {
    asked.push(options.message);
    return answers[options.message] ?? options.initialValue ?? options.initialValues ?? options.defaultValue;
  };
  return { asked, confirm: answer, select: answer, text: answer, multiselect: answer, isCancel: () => false };
}

test('the default path installs no VM', async () => {
  const ui = prompts({});
  const result = await collectChoices(ui, host, {}, { providers: found });
  assert.deepEqual(ui.asked, ['Providers to enable', 'UI/API port', VM]);
  assert.deepEqual(result.providers, { codex: true, claude: true });
  assert.equal(result.port, 8787);
  assert.equal(result.vmEnabled, false);
  assert.equal(result.master, null);
  assert.equal(result.backend, 'host');
});

test('only installed providers are offered, and one provider skips the question', async () => {
  const ui = prompts({});
  ui.multiselect = async options => {
    assert.deepEqual(options.options.map(option => option.value), ['codex', 'claude']);
    assert.match(options.options[1].hint, /sign in/i);
    assert.equal(options.required, true);
    return ['claude'];
  };
  const chosen = await collectChoices(ui, host, {}, { providers: found });
  assert.deepEqual(chosen.providers, { codex: false, claude: true });
  const single = prompts({});
  const result = await collectChoices(single, host, {}, { providers: [found[1]] });
  assert.deepEqual(result.providers, { codex: false, claude: true });
  assert.equal(single.asked.includes('Providers to enable'), false);
});

test('no installed provider stops before any question', async () => {
  const ui = prompts({});
  await assert.rejects(collectChoices(ui, host, {}, { providers: [] }), /Codex or Claude/);
  assert.deepEqual(ui.asked, []);
});

test('choosing the VM asks for its size with recommendations that fit', async () => {
  const ui = prompts({ [VM]: true });
  const result = await collectChoices(ui, large, {}, { providers: found });
  assert.deepEqual(result.master, { cpuCount: 2, memoryGiB: 8, diskGiB: 64 });
  assert.equal(result.vmEnabled, true);
  assert.equal(result.limits.memoryPercent, 50);
  assert.equal(result.computerApp, null);
  assert.ok(ui.asked.includes('Change global VM resource limits?'));
});

test('the VM question states its cost and warns when the Mac is small', async () => {
  const ui = prompts({});
  const confirm = ui.confirm;
  ui.confirm = async options => {
    if (options.message === VM) assert.match(options.inactive, /recommended/i);
    return confirm(options);
  };
  await collectChoices(ui, host, {}, { providers: found });
});

test('Mac-only mode never asks about a VM', async () => {
  const ui = prompts({ [VM]: true });
  const result = await collectChoices(ui, large, {}, { hostOnly: true, providers: found });
  assert.equal(result.vmEnabled, false);
  assert.equal(ui.asked.includes(VM), false);
});

test('reviewing again keeps earlier answers and clears a declined VM', async () => {
  const ui = prompts({ [VM]: false });
  const result = await collectChoices(ui, large, { port: 9000, vmEnabled: true,
    providers: { codex: false, claude: true }, master: { cpuCount: 4, memoryGiB: 8, diskGiB: 80 },
    computerApp: '/test/old.app' }, { providers: found });
  assert.equal(result.port, 9000);
  assert.deepEqual(result.providers, { codex: false, claude: true });
  assert.equal(result.master, null);
  assert.equal(result.computerApp, null);
});

test('Computer Use is an explicit VM choice', async () => {
  const ui = prompts({ [VM]: true, 'Include Computer Use in the VM?': true,
    [MAC_APP_PATH]: '/test/Codex Computer Use.app' });
  ui.log = { message: () => {} };
  const result = await collectChoices(ui, large, {}, { providers: found });
  assert.equal(result.computerApp, '/test/Codex Computer Use.app');
});

test('cancellation does not become an accepted answer', async () => {
  const ui = prompts({});
  ui.isCancel = () => true;
  await assert.rejects(collectChoices(ui, host, {}, { providers: found }), /cancel/i);
});

test('the VM question says when the macOS image is already downloaded', async () => {
  const labels = [];
  for (const imageCached of [true, false]) {
    const ui = prompts({});
    const confirm = ui.confirm;
    ui.confirm = async options => { if (options.message === VM) labels.push(options.active); return confirm(options); };
    await collectChoices(ui, large, {}, { providers: found, imageCached });
  }
  assert.match(labels[0], /already downloaded/);
  assert.doesNotMatch(labels[0], /20 GiB/);
  assert.match(labels[1], /20 GiB download/);
});

// Supplies answers in order per question and records every guide shown, as the real wizard's ui does.
function guided(answers) {
  const asked = [], notes = [], selects = [];
  const answer = async options => {
    asked.push(options.message);
    const queue = answers[options.message];
    if (Array.isArray(queue)) {
      if (!queue.length) throw new Error(`no answer left for ${options.message}`);
      const value = queue.shift();
      // A text answer must pass the prompt's own validation, as clack would insist.
      if (options.validate && typeof value === 'string') {
        const problem = options.validate(value);
        if (problem) { notes.push({ title: 'validation', text: problem }); return answer(options); }
      }
      return value;
    }
    return options.initialValue ?? options.initialValues ?? options.defaultValue;
  };
  return { asked, notes, selects, isCancel: () => false, confirm: answer, text: answer, multiselect: answer,
    select: async options => { selects.push(options); return answer(options); },
    note: (text, title) => notes.push({ title, text }),
    log: { message: text => notes.push({ title: text.split('\n')[0], text }) } };
}
const winHost = { ...host, platform: 'win32', arch: 'arm64' };
const helper = 'C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.930.3930.0_arm64__2p2nqsd0c76g0\\app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows\\codex-computer-use-arm64.exe';
const shortPrompt = options => [options.message, ...(options.options ?? []).map(option => `${option.label} ${option.hint ?? ''}`)]
  .every(text => text.split('\n').length <= 3 && text.split('\n').every(line => line.length <= 110));

test('a detected Windows helper is offered as the answer, without typing a path', async () => {
  const ui = guided({ [HELPER]: ['detected'] });
  const result = await collectChoices(ui, winHost, {}, { providers: found, detected: { windowsComputerHelper: helper } });
  assert.equal(result.windowsComputerHelper, helper);
  assert.equal(ui.asked.includes(HELPER_PATH), false);
  const question = ui.selects.find(options => options.message === HELPER);
  assert.deepEqual(question.options.map(option => option.value), ['detected', 'path', 'guide', 'skip']);
  assert.ok(question.options[0].hint.includes('codex-computer-use-arm64.exe'));
  assert.equal(question.initialValue, 'skip', 'connecting stays an explicit choice');
  assert.ok(shortPrompt(question));
  assert.equal(ui.notes.length, 0, 'no guide when detection worked');
});

test('"Show me how" prints the numbered Windows guide and asks again', async () => {
  const ui = guided({ [HELPER]: ['guide', 'skip'] });
  const result = await collectChoices(ui, winHost, {}, { providers: found, detected: { windowsComputerHelper: null } });
  assert.equal(result.windowsComputerHelper, null);
  assert.equal(ui.asked.filter(message => message === HELPER).length, 2);
  const shown = ui.notes.map(note => note.text).join('\n');
  assert.match(shown, /^1\. /m);
  assert.match(shown, /Get-AppxPackage -Name OpenAI\.Codex/);
  assert.doesNotMatch(shown, /Finder|\.app\b/);
  const question = ui.selects.find(options => options.message === HELPER);
  assert.deepEqual(question.options.map(option => option.value), ['path', 'guide', 'skip']);
});

test('an undetected helper shows the guide, accepts ? and a quoted pasted path, and explains a wrong file', async () => {
  const ui = guided({ [HELPER]: ['path'], [HELPER_PATH]: ['?', 'C:\\Apps\\Codex.exe', `"${helper}"`] });
  const result = await collectChoices(ui, winHost, {}, { providers: found, detected: { windowsComputerHelper: null } });
  assert.equal(result.windowsComputerHelper, helper);
  const guides = ui.notes.filter(note => /^1\. /m.test(note.text));
  assert.equal(guides.length, 2, 'once because nothing was found, once for ?');
  const error = ui.notes.find(note => note.title === 'validation').text;
  assert.match(error, /codex-computer-use-arm64\.exe/);
  assert.match(error, /\?/);
});

test('on the Mac a found Computer Use app is the default and its guide stays Mac-only', async () => {
  const app = '/Users/sam/.codex/computer-use/Codex Computer Use.app';
  const ui = guided({ [VM]: [true], 'Include Computer Use in the VM?': [true], [MAC_APP]: ['detected'] });
  const result = await collectChoices(ui, large, {}, { providers: found, detected: { computerApp: app } });
  assert.equal(result.computerApp, app);
  assert.equal(ui.notes.length, 0);
  const missing = guided({ [VM]: [true], 'Include Computer Use in the VM?': [true], [MAC_APP_PATH]: ['/tmp/config.json', '/Apps/Codex Computer Use.app'] });
  const chosen = await collectChoices(missing, large, {}, { providers: found, detected: { computerApp: null } });
  assert.equal(chosen.computerApp, '/Apps/Codex Computer Use.app');
  assert.equal(missing.asked.includes(MAC_APP), false, 'nothing to offer, so the path is asked directly');
  const shown = missing.notes.map(note => note.text).join('\n');
  assert.match(shown, /~\/\.codex\/computer-use/);
  assert.doesNotMatch(shown, /WindowsApps|PowerShell/);
  assert.match(missing.notes.find(note => note.title === 'validation').text, /\.app/);
});

test('a plain Windows console is told what the ASCII selection marks mean', () => {
  const plain = selectionLegend({}, 'win32');
  assert.match(plain, /Space to choose or clear/);
  assert.match(plain, /\[\+\] means chosen/);
  assert.match(plain, /\[•\] only marks the highlighted row/);
  const terminal = selectionLegend({ WT_SESSION: '1' }, 'win32');
  assert.doesNotMatch(terminal, /\[\+\]/);
  assert.match(terminal, /Space to choose or clear/);
  assert.doesNotMatch(selectionLegend({}, 'darwin'), /\[\+\]/);
});

test('the legend is shown before each multi-choice question', async () => {
  const shown = [];
  const ui = prompts({});
  ui.log = { info: text => shown.push(text) };
  await collectChoices(ui, host, {}, { providers: found });
  assert.equal(shown.length, 1, 'one legend before Providers to enable');
  assert.match(shown[0], /Space to choose or clear/);
});
