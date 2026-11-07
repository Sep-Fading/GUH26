// Checks accepted shutdown outcomes without guests.
import assert from 'node:assert/strict';
// Runs independent shutdown report contract cases.
import { test } from 'node:test';
// Classifies native outcomes for live acceptance.
import { shutdownResult } from './support/vm-shutdown.mjs';

test('clean shutdown remains distinct from forced shutdown', () => {
  assert.deepEqual(shutdownResult(0, '', 500), { code: 0, forced: false, ms: 500 });
  const output = 'Master VM has not stopped after 90 seconds; powering off.\n'
    + 'Master VM was forced off; unsaved data or unflushed writes may have been lost.';
  assert.deepEqual(shutdownResult(2, output, 30_500), { code: 2, forced: true, ms: 30_500 });
});

test('failed or unreported shutdown never passes acceptance', () => {
  for (const code of [1, null, 137]) assert.throws(() => shutdownResult(code, '', 500));
  assert.throws(() => shutdownResult(2, '', 30_500));
  assert.throws(() => shutdownResult(0, 'Master VM was forced off', 30_500));
});
