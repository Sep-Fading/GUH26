// Checks what the wizard shows while it installs, without a terminal.
import assert from 'node:assert/strict';
// Runs each reporting case in isolation.
import { test } from 'node:test';
// Loads the reporter under test.
import { createReporter } from '../scripts/setup/reporter.mjs';

// Records every call the reporter makes on the prompt library.
function fakeUi() {
  const events = [];
  const record = kind => ({
    start: message => events.push([kind, 'start', message]),
    advance: (step, message) => events.push([kind, 'advance', step, message]),
    message: message => events.push([kind, 'message', message]),
    stop: message => events.push([kind, 'stop', message]),
    error: (message, options) => events.push([kind, 'error', message, options]),
    success: message => events.push([kind, 'success', message]),
  });
  return { events, progress: options => { events.push(['progress', 'create', options.max]); return record('progress'); },
    taskLog: options => { events.push(['log', 'create', options.title]); return record('log'); } };
}
const image = { id: 'macos', name: 'macOS restore image' };
// The build stage names a Mac on macOS, as before the Windows port.
const machine = process.platform === 'win32' ? 'this computer' : 'this Mac';

test('each stage is numbered against the plan', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['download', 'build', 'install-macos']);
  report.onStage('download', image);
  report.onStage('build');
  report.onStage('install-macos');
  report.finish();
  const titles = ui.events.filter(event => event[1] === 'create' || event[1] === 'start').map(event => event.at(-1));
  assert.ok(titles.includes('Step 1 of 3  macOS restore image'));
  assert.ok(titles.includes(`Step 2 of 3  Building MiniClaw for ${machine}`));
  assert.ok(titles.includes('Step 3 of 3  Installing macOS in the VM'));
});

test('several downloads share one step number', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['download', 'build']);
  report.onStage('download', image);
  report.onStage('download', { id: 'codex', name: 'Codex package' });
  report.onStage('build');
  const starts = ui.events.filter(event => event[1] === 'start' || event[0] === 'log' && event[1] === 'create').map(event => event.at(-1));
  assert.deepEqual(starts, ['Step 1 of 2  macOS restore image', 'Step 1 of 2  Codex package',
    `Step 2 of 2  Building MiniClaw for ${machine}`]);
});

test('download progress advances the bar by the bytes received and names the rate', () => {
  const ui = fakeUi();
  let now = 0;
  const report = createReporter(ui, ['download'], { clock: () => now });
  report.onStage('download', image);
  now = 1000;
  report.onProgress(image, { received: 250, total: 1000 });
  now = 2000;
  report.onProgress(image, { received: 500, total: 1000 });
  const steps = ui.events.filter(event => event[1] === 'advance');
  assert.deepEqual(steps.map(event => event[2]), [250, 250], 'the bar moves by permille of the total');
  assert.match(steps[1][3], /MiB|B/);
  assert.match(steps[1][3], /left/);
  report.onProgress(image, { received: 1000, total: 1000, phase: 'verify' });
  report.onProgress(image, { received: 1000, total: 1000, verified: true });
  assert.ok(ui.events.some(event => /verifying checksum/.test(event.at(-1))));
  report.finish();
  assert.ok(ui.events.some(event => event[1] === 'stop' && /checksum verified/.test(event[2])));
});

test('a resumed download measures its rate from where it resumed', () => {
  const ui = fakeUi();
  let now = 0;
  const report = createReporter(ui, ['download'], { clock: () => now });
  report.onStage('download', image);
  now = 1000;
  report.onProgress(image, { received: 900, total: 1000 });
  now = 2000;
  report.onProgress(image, { received: 950, total: 1000 });
  const last = ui.events.filter(event => event[1] === 'advance').at(-1);
  assert.match(last[3], /~1s left/);
});

test('build output goes to a log that strips escape codes and keeps short lines', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['build']);
  report.onStage('build');
  report.onOutput('\u001b[32m   Compiling\u001b[0m miniclaw-server\npartial');
  report.onOutput(' line\n' + 'x'.repeat(500) + '\n');
  const lines = ui.events.filter(event => event[0] === 'log' && event[1] === 'message').map(event => event[2]);
  assert.deepEqual(lines.slice(0, 2), ['Compiling miniclaw-server', 'partial line']);
  assert.ok(lines[2].length <= 120);
  report.finish();
  assert.ok(ui.events.some(event => event[0] === 'log' && event[1] === 'success'));
});

test('macOS installation is one bar that updates in place, not a line per percent', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['install-macos']);
  report.onStage('install-macos');
  for (const percent of [73, 74, 74, 78]) report.onOutput(`{"status":"installing","percent":${percent}}\n`);
  report.onOutput('Compiling something noisy\n');
  assert.deepEqual(ui.events[0], ['progress', 'create', 100]);
  assert.equal(ui.events[1].at(-1), 'Step 1 of 1  Installing macOS in the VM');
  const moves = ui.events.filter(event => event[1] === 'advance');
  assert.deepEqual(moves.map(event => event[2]), [73, 1, 4], 'a repeated percent moves nothing');
  assert.equal(moves.at(-1)[3], 'macOS installation 78%');
  assert.equal(ui.events.some(event => event[0] === 'log'), false, 'no log lines pile up');
  report.finish();
  assert.match(ui.events.at(-1)[2], /macOS installed/);
});

test('a failed macOS installation shows the last thing the installer said', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['install-macos']);
  report.onStage('install-macos');
  report.onOutput('{"status":"installing","percent":12}\nrestore image rejected\n');
  report.fail('stopped');
  const error = ui.events.find(event => event[1] === 'error');
  assert.match(error[2], /stopped/);
  assert.match(error[2], /restore image rejected/);
});

test('a failure keeps the log visible', () => {
  const ui = fakeUi();
  const report = createReporter(ui, ['build']);
  report.onStage('build');
  report.fail('Installation stopped');
  const error = ui.events.find(event => event[1] === 'error');
  assert.equal(error[0], 'log');
  assert.match(error[2], /Installation stopped/);
  assert.deepEqual(error[3], { showLog: true });
  const cancelled = fakeUi();
  const quiet = createReporter(cancelled, ['build']);
  quiet.onStage('build');
  quiet.fail('cancelled', { showLog: false });
  assert.deepEqual(cancelled.events.find(event => event[1] === 'error')[3], { showLog: false });
});
