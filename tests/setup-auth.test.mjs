// Checks guest sign-in without real credentials.
import assert from 'node:assert/strict';
// Runs wizard authentication and cancellation cases.
import { test } from 'node:test';
// Exercises the wizard's guest account workflow.
import { signInGuest } from '../scripts/setup/auth.mjs';

const STATUS = '/api/vms/master/status', START = '/api/vms/master/auth/start';
const code = { status: 'pending', verificationUrl: 'https://auth.openai.com/codex/device', userCode: 'TEST-CODE' };

// Records the API contract and supplies explicitly scripted responses.
function fixture(scripts) {
  const calls = [];
  return { calls, fetcher: async (url, options = {}) => {
    const path = new URL(url).pathname;
    calls.push({ path, ...options });
    assert.ok(scripts[path]?.length, `Unscripted request ${path}`);
    return Response.json(scripts[path].shift());
  } };
}

test('a signed-in guest needs no new sign-in attempt', async () => {
  const mock = fixture({ [STATUS]: [{ state: 'running', ready: true, signedIn: true }] });
  await signInGuest('http://127.0.0.1:19878', { ...mock, showCode: () => assert.fail('Unexpected code') });
  assert.deepEqual(mock.calls.map(call => call.path), [STATUS]);
});

test('the wizard shows a device code from the guest and waits for the guest account', async () => {
  const mock = fixture({ [START]: [code],
    [STATUS]: [{ ready: true, signedIn: false }, { ready: true, signedIn: false }, { ready: true, signedIn: true }] });
  const codes = [];
  await signInGuest('http://127.0.0.1:19878', { ...mock, wait: async () => {}, showCode: value => codes.push(value) });
  assert.deepEqual(codes, [{ url: 'https://auth.openai.com/codex/device', code: 'TEST-CODE' }]);
  const login = mock.calls.find(call => call.path === START);
  assert.deepEqual(JSON.parse(login.body), { method: 'device' });
  assert.equal(login.headers.Origin, 'http://127.0.0.1:19878');
  assert.equal(login.headers['X-MiniClaw-Request'], '1');
  assert.equal(mock.calls.some(call => call.path === '/api/auth/start'), false, 'the Mac sign-in route is never used');
});

test('cancellation and a lost worker never report readiness', async () => {
  await assert.rejects(signInGuest('http://127.0.0.1:19878', {
    signal: AbortSignal.abort(), fetcher: () => assert.fail('Request after cancellation') }));
  const mock = fixture({ [START]: [code], [STATUS]: [{ ready: true, signedIn: false }, { ready: false, signedIn: false }] });
  await assert.rejects(signInGuest('http://127.0.0.1:19878', { ...mock, wait: async () => {}, showCode: () => {} }),
    /worker disconnected/i);
});

test('remote APIs and untrusted sign-in links are rejected', async () => {
  await assert.rejects(signInGuest('http://example.com', {
    fetcher: () => assert.fail('Remote API contacted') }), /loopback/i);
  const mock = fixture({ [STATUS]: [{ ready: true, signedIn: false }],
    [START]: [{ ...code, verificationUrl: 'https://evil.example' }] });
  await assert.rejects(signInGuest('http://127.0.0.1:19878', { ...mock,
    showCode: () => assert.fail('Untrusted link displayed') }), /sign-in response/i);
});
