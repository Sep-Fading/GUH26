// Checks progress without relying on color.
import assert from 'node:assert/strict';
// Runs deterministic terminal progress formatting checks.
import { test } from 'node:test';
// Formats measured progress for terminal output.
import { progressBar } from '../scripts/setup/progress.mjs';

test('progress distinguishes remaining bytes without terminal colors', () => {
  assert.equal(progressBar(0, 100), '[------------------------] 0%');
  assert.equal(progressBar(50, 100), '[############------------] 50%');
  assert.equal(progressBar(100, 100), '[########################] 100%');
});

test('partial transfers cannot display completion through rounding', () => {
  assert.equal(progressBar(999, 1000), '[#######################-] 99%');
});
