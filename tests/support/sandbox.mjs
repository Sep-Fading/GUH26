// Reads the backend's own sandbox report, so a suite can state both sides of the Windows gate.
import assert from 'node:assert/strict';
import { json } from './harness.mjs';

// The fixed sentences a closed gate answers with; the gate's reason follows each.
export const TOOLS_UNAVAILABLE = 'Sandboxed tools are unavailable on this platform. A shell command requires the user to grant timed Unsandboxed shell access in Tools.';
export const MCP_GATE_CLOSED = 'This PC cannot enforce the sandbox, so the server was not started.';
export const TERMINAL_UNAVAILABLE = 'The sandbox required by the terminal is not available on this platform.';
export const DISCARD_UNAVAILABLE = 'Changes cannot be discarded while the Windows sandbox that runs Git is unavailable.';

/** Whether sandboxed work can run on this backend, with the reason when it cannot. */
export const sandboxReport = async app => json(await app.request('/api/sandbox'));

/** The exact refusal a closed gate gives: the fixed sentence, then the report's reason. Only a
 *  Windows backend whose report says the sandbox is unavailable may reach this. */
export async function closedRefusal(app, sentence, part = 'sandbox') {
  const report = await sandboxReport(app);
  const { available, reason } = part === 'git' ? report.git : report;
  assert.equal(process.platform, 'win32', 'only Windows has a gate that closes');
  assert.equal(available, false, 'the backend took the closed-gate path, so its report says unavailable');
  assert.match(reason, /\.$/);
  return `${sentence} ${reason}`;
}

/** Checks that what a suite observed (`closed`) is what the backend reports. */
export async function assertGate(app, closed) {
  assert.equal((await sandboxReport(app)).available, !closed, 'the observed behaviour matches the sandbox report');
  return closed;
}
