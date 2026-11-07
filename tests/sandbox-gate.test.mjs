// Checks the sandbox report the page and the other suites read: whether sandboxed work can run on
// this installation, and the reason when it cannot.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launchApp, json } from './support/harness.mjs';

const windows = process.platform === 'win32';
const part = value => assert.deepEqual(Object.keys(value).filter(key => !['available', 'reason'].includes(key)), []);

test('the report says whether the sandbox, Git and command networking are available', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const report = await json(await app.request('/api/sandbox'));
  assert.equal(typeof report.available, 'boolean');
  part(report.git); part(report.network);
  if (report.available) {
    assert.equal(report.reason, undefined, 'an open gate gives no reason');
  } else {
    // A closed gate names one missing precondition in a sentence the user can act on.
    assert.match(report.reason, /^[A-Z].{20,220}\.$/);
    assert.equal(report.git.available, false);
    assert.equal(report.network.available, false);
    assert.equal(report.git.reason, report.reason);
  }
  if (!report.git.available) assert.match(report.git.reason, /\.$/);
  // The Mac has one fact, sandbox-exec, and everything follows it.
  if (!windows) assert.deepEqual(report, { available: true, git: { available: true }, network: { available: true } });
});

test('MINICLAW_DISABLE_SANDBOX closes the Windows gate and can open nothing', async t => {
  const app = await launchApp({}, { env: { MINICLAW_DISABLE_SANDBOX: '1' } });
  t.after(() => app.close());
  const report = await json(await app.request('/api/sandbox'));
  if (!windows) return assert.equal(report.available, true, 'the switch is a Windows switch; the Mac is unchanged');
  assert.equal(report.available, false);
  assert.equal(report.reason, 'The sandbox was switched off for this run with MINICLAW_DISABLE_SANDBOX.');
  assert.deepEqual([report.git, report.network], [{ available: false, reason: report.reason }, { available: false, reason: report.reason }]);
});
