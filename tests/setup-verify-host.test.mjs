import { test } from 'node:test';
import assert from 'node:assert/strict';
import { verifyHost } from '../scripts/setup/verify-host.mjs';

// Host checks must never invoke VM routes or advertise a running service as sandbox proof.
test('Windows host verification checks backend account and service without VM access', async () => {
  const calls = [];
  const results = await verifyHost({ api: async path => {
    calls.push(path);
    if (path === '/api/health') return { backend: 'ok' };
    if (path === '/api/account') return { status: 'signed_in' };
    throw new Error(`Unexpected ${path}`);
  }, service: async () => ({ state: 'Running', exitCode: 0, serviceSpecificExitCode: 0 }) });
  assert.deepEqual(calls, ['/api/health', '/api/account']);
  assert.ok(results.every(result => result.state !== 'FAIL'));
  assert.ok(results.some(result => result.state === 'skip' && /native sandbox/.test(result.name)));
});

test('Windows host verification reports service cleanup failures', async () => {
  const results = await verifyHost({ api: async path => path.endsWith('health') ? { backend: 'ok' } : { status: 'signed_in' },
    service: async () => ({ state: 'Stopped', exitCode: 1, serviceSpecificExitCode: 0 }) });
  assert.ok(results.some(result => result.state === 'FAIL' && /service/.test(result.name)));
});
