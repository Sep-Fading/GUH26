// Checks `miniclaw repair`: the elevated path that sets up the network service's access again.
import assert from 'node:assert/strict';
// Runs the repair cases with injected service steps.
import { test } from 'node:test';
// The module under test.
import { describeAccess, repairWindows } from '../scripts/setup/repair-windows.mjs';

const service = `MiniClawNetwork-${'a'.repeat(32)}`;
// A file reader over a fixed set of contents, keyed by the end of the path.
const files = contents => async path => {
  const found = Object.entries(contents).find(([name]) => path.replaceAll('/', '\\').endsWith(name));
  if (!found) throw Object.assign(new Error('missing'), { code: 'ENOENT' });
  return found[1];
};

test('the service status is said in one sentence', () => {
  assert.deepEqual(describeAccess({ ok: true, repaired: [] }), { ok: true, text: 'Network access for sandboxed tools is set up.' });
  assert.match(describeAccess({ ok: true, repaired: ['a', 'b'] }).text, /set up again \(2 Windows settings restored\)/);
  assert.equal(describeAccess({ ok: false, error: 'Cannot open HKLM\\x: Access is denied.\n' }).ok, false);
  assert.match(describeAccess({ ok: false, error: 'Cannot open HKLM\\x: Access is denied.\n' }).text, /could not set up .*Access is denied\. $/);
  assert.match(describeAccess(undefined).text, /no status was published/);
  assert.equal(describeAccess({ ok: true, removed: true }).ok, false);
});

test('repair restarts the installed service once and reports its new status', async () => {
  const calls = [], output = [];
  const result = await repairWindows('C:\\MiniClaw', { platform: 'win32', programData: 'C:\\ProgramData', onOutput: text => output.push(text),
    update: async options => { calls.push(options); },
    read: files({ 'data\\windows-network.json': JSON.stringify({ service_name: service }),
      [`${service}\\winsock-acl-status.json`]: JSON.stringify({ ok: true, repaired: ['k'] }) }) });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].runtime, 'C:\\MiniClaw\\runtime');
  assert.equal(calls[0].data, 'C:\\MiniClaw\\data');
  assert.equal(result.ok, true);
  assert.match(output.join(''), /set up again/);
  // Which installation is repaired is said first, before the administrator approval is asked.
  assert.equal(output[0], 'Repairing the MiniClaw installation at C:\\MiniClaw.\n');
});

test('a status that still fails is an error, and a missing service names the setup commands', async () => {
  await assert.rejects(repairWindows('C:\\MiniClaw', { platform: 'win32', update: async () => {}, onOutput: () => {},
    read: files({ 'data\\windows-network.json': JSON.stringify({ service_name: service }),
      [`${service}\\winsock-acl-status.json`]: JSON.stringify({ ok: false, error: 'blocked by a security product' }) }) }),
  /blocked by a security product/);
  let updated = false;
  await assert.rejects(repairWindows('C:\\MiniClaw', { platform: 'win32', update: async () => { updated = true; }, read: files({}) }),
    /miniclaw resume.*miniclaw setup/);
  assert.equal(updated, false, 'nothing is elevated without an installed service');
  await assert.rejects(repairWindows('C:\\MiniClaw', { platform: 'win32', update: async () => { updated = true; },
    read: files({ 'data\\windows-network.json': JSON.stringify({ service_name: 'registryRead' }) }) }), /Invalid installed network service/);
  assert.equal(updated, false);
  await assert.rejects(repairWindows('/x', { platform: 'darwin' }), /does not use it/);
});
