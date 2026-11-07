// Rejects shutdown failures and misleading success.
import assert from 'node:assert/strict';

// Records accepted shutdowns without calling forced power-off a clean exit.
export function shutdownResult(code, output, ms) {
  assert.ok(code === 0 || code === 2, `Unexpected shutdown exit ${code}`);
  const forced = code === 2;
  assert.equal(output.includes('was forced off'), forced, 'Shutdown warning and exit code disagree');
  if (forced) assert.match(output, /90 seconds; powering off/);
  return { code, forced, ms };
}
