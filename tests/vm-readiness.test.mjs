// Checks startup failures and visible state changes.
import assert from 'node:assert/strict';
// Runs bounded readiness tests without guests.
import { test } from 'node:test';
// Exercises the shared VM startup contract.
import { waitReady, waitStopped } from '../scripts/vm/readiness.mjs';

test('readiness reports backend, worker and ready once', async () => {
  const states = [];
  const replies = [null, null, { backend: 'ok', codex: 'unavailable' },
    { backend: 'ok', codex: 'unavailable' }, { backend: 'ok', codex: 'connected' }];
  await waitReady('http://127.0.0.1:19878', {}, {
    health: async () => replies.shift(), wait: async () => {},
    onStatus: state => states.push(state),
  });
  assert.deepEqual(states, ['backend', 'worker', 'ready']);
});

test('worker timeout identifies the failed stage', async () => {
  await assert.rejects(waitReady('http://127.0.0.1:19878', {}, {
    timeoutMs: 30, health: async () => ({ backend: 'ok', codex: 'unavailable' }),
  }), /timed out.*worker/i);
});

test('hung health request cannot defeat the deadline', async () => {
  await assert.rejects(waitReady('http://127.0.0.1:19878', {}, {
    timeoutMs: 30, health: () => new Promise(() => {}),
  }), /timed out.*backend/i);
});

test('native exit stops readiness before authentication', async () => {
  await assert.rejects(waitReady('http://127.0.0.1:19878', { done: Promise.resolve(0) }, {
    health: () => new Promise(() => {}),
  }), /VM stopped unexpectedly.*0/);
});

test('cancellation stops a pending health request', async () => {
  const controller = new AbortController();
  const pending = waitReady('http://127.0.0.1:19878', {}, {
    signal: controller.signal, health: () => new Promise(() => {}),
  });
  controller.abort(new Error('User cancelled'));
  await assert.rejects(pending, /User cancelled/);
});

test('health failures report safe diagnostics without response contents', async () => {
  const states = [];
  await assert.rejects(waitReady('http://127.0.0.1:19878', {}, {
    timeoutMs: 30, onStatus: state => states.push(state),
    health: async () => { throw new Error('private upstream payload'); },
  }), error => /timed out.*backend/i.test(error.message)
    && !error.message.includes('private upstream payload'));
  assert.deepEqual(states, ['backend']);
});

test('unexpected runtime exit fails even with a zero exit code', async () => {
  await assert.rejects(waitStopped({ done: Promise.resolve(0) }), /VM stopped unexpectedly/);
});

test('requested shutdown is not reported as a runtime failure', async () => {
  await waitStopped({ done: new Promise(() => {}) }, AbortSignal.abort());
});
