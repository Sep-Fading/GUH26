// Checks readiness before marking installations complete.
import assert from 'node:assert/strict';
// Runs wizard startup without booting guests.
import { test } from 'node:test';
// Exercises startup, guest setup and OAuth ordering.
import { connectSetup } from '../scripts/setup/connect.mjs';

const pair = { port: 19878, mac: { worker: '/x/worker.json' } };
const ready = { state: 'running', ready: true, signedIn: true, computerReady: true };

// Builds injected collaborators that record the order of operations.
function parts(calls, overrides = {}) {
  return {
    start: async (_pair, options) => { calls.push('session');
      assert.equal(options.setup, true, 'setup must ask for an interactive VM window');
      return { linksPath: '/x/links.json',
      startMaster: async () => calls.push('master'), close: async () => calls.push('close-session') }; },
    launch: async links => { calls.push(`backend:${links}`); return { close: async () => calls.push('close-backend') }; },
    account: async () => ({ status: 'signed_in' }),
    guestSetup: async () => { calls.push('guest'); },
    status: async () => ready, wait: async () => {},
    authenticate: async () => calls.push('auth'), ...overrides,
  };
}

test('setup starts the session, the Mac backend, then the VM, then signs in the guest', async () => {
  const calls = [];
  const running = await connectSetup(pair, parts(calls));
  assert.deepEqual(calls, ['session', 'backend:/x/links.json', 'master', 'guest', 'auth']);
  await running.close();
  assert.deepEqual(calls.slice(-2), ['close-backend', 'close-session'], 'the backend stops before the VMs');
});

test('a Mac with no signed-in provider is asked to sign in first', async () => {
  const calls = [];
  let reads = 0;
  await connectSetup(pair, parts(calls, {
    account: async () => ({ status: ++reads > 2 ? 'signed_in' : 'signed_out' }),
    macSignIn: async url => { calls.push(`mac:${url}`); },
  }));
  assert.deepEqual(calls.slice(0, 4), ['session', 'backend:/x/links.json', 'mac:http://127.0.0.1:19878', 'master']);
});

test('the guest worker must attach before sign-in starts', async () => {
  const calls = [];
  let polls = 0;
  await connectSetup(pair, parts(calls, {
    status: async () => { polls++; return polls > 2 ? ready : { state: 'running', ready: false }; } }));
  assert.ok(polls >= 3);
  assert.equal(calls.at(-1), 'auth');
});

test('a failure closes the backend and the session and never completes', async () => {
  const calls = [];
  await assert.rejects(connectSetup(pair, parts(calls, {
    authenticate: async () => { throw new Error('Sign-in failed'); } })), /Sign-in failed/);
  assert.deepEqual(calls.slice(-2), ['close-backend', 'close-session']);
});

test('cancelled setup starts nothing', async () => {
  await assert.rejects(connectSetup(pair, { signal: AbortSignal.abort(),
    start: () => assert.fail('Unexpected start') }));
});

test('declining manual setup keeps the installation unfinished', async () => {
  const calls = [];
  await assert.rejects(connectSetup(pair, parts(calls, { guestSetup: async () => false,
    authenticate: () => assert.fail('Must not sign in') })), /Setup cancelled/);
  assert.equal(calls.includes('close-session'), true);
});

test('a VM that reports failure stops setup with its reason', async () => {
  const calls = [];
  await assert.rejects(connectSetup(pair, parts(calls, {
    status: async () => ({ state: 'failed', reason: 'VM resource limit exceeded.' }) })), /resource limit/);
});

test('selected Computer Use must be found in the guest', async () => {
  const calls = [];
  await assert.rejects(connectSetup(pair, parts(calls, { computerUse: true,
    status: async () => ({ ...ready, computerReady: false }) })), /Computer Use.*not installed/);
  await connectSetup(pair, parts(calls, { computerUse: true }));
});

test('only a Mac-backend pair is accepted', async () => {
  await assert.rejects(connectSetup({ port: 19878, controller: {} },
    { start: () => assert.fail('started') }), /Mac-backend/);
});
