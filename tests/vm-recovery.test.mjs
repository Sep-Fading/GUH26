// Checks recovery without trusting health alone.
import assert from 'node:assert/strict';
// Runs an explicitly selected live experiment.
import { test } from 'node:test';
// Compares history without recording private content.
import { createHash } from 'node:crypto';
// Retains bounded evidence for failed checks.
import { mkdir, writeFile } from 'node:fs/promises';
// Resolves the explicit private evidence directory.
import { isAbsolute, join } from 'node:path';
// Bounds polling without replaying model requests.
import { setTimeout as delay } from 'node:timers/promises';

// Hashes message roles and text, ignoring regenerated history item identifiers.
function history(snapshot) {
  return createHash('sha256').update(JSON.stringify(snapshot.messages
    .map(({ role, text }) => ({ role, text })))).digest('hex');
}

// Observes a manually restarted worker; never stops guests or resends messages.
test('guest worker loss recovers storage and retained history', { timeout: 150_000 }, async t => {
  const base = process.env.MINICLAW_PAIRED_URL;
  const id = process.env.MINICLAW_RECOVERY_THREAD;
  const root = process.env.MINICLAW_RECOVERY_EVIDENCE;
  assert.match(base ?? '', /^http:\/\/127\.0\.0\.1:\d+$/);
  // Testing the primary interface requires a separate, explicit opt-in.
  assert.ok(new URL(base).port !== '8787'
    || process.env.MINICLAW_RECOVERY_PRIMARY === '1',
  'Set MINICLAW_RECOVERY_PRIMARY=1 only for an approved primary-interface test');
  assert.match(id ?? '', /^[a-f0-9-]{36}$/);
  assert.ok(root && isAbsolute(root));
  const report = { passed: false, before: {}, after: {}, interrupted: false };
  const directory = join(root, `recovery-${Date.now()}`);
  await mkdir(directory, { mode: 0o700 });
  // Sends bounded local API requests with the normal mutation headers.
  async function request(path, body) {
    const response = await fetch(base + path, { signal: AbortSignal.timeout(3000),
      ...(body === undefined ? {} : { method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-MiniClaw-Request': '1', Origin: base,
      }, body: JSON.stringify(body) }),
    });
    return { status: response.status, value: await response.json() };
  }
  try {
    const path = `/api/threads/${id}`;
    const before = await request(path);
    assert.equal(before.status, 200);
    assert.equal(before.value.activeTurn, null, 'Do not interrupt active work for this test');
    assert.equal(before.value.pending, false);
    report.historyBefore = history(before.value);
    for (const endpoint of ['/api/health', '/api/agents', '/api/projects', '/api/threads']) {
      const result = await request(endpoint);
      report.before[endpoint] = result.status;
      assert.equal(result.status, 200, `${endpoint} is not healthy before restart`);
    }
    t.diagnostic('Baseline captured. Restart only the idle guest worker now.');
    const began = Date.now();
    while (Date.now() - began < 90_000) {
      try {
        const health = await request('/api/health');
        if (health.status !== 200 || health.value.codex !== 'connected') break;
      } catch { break; }
      await delay(100);
    }
    assert.ok(Date.now() - began < 90_000, 'No worker disconnection observed');
    report.interrupted = true;
    const lost = Date.now();
    let recovered = false;
    while (Date.now() - lost < 45_000) {
      try {
        const health = await request('/api/health');
        if (health.status === 200 && health.value.codex === 'connected') {
          recovered = true;
          break;
        }
      } catch { /* Wait only until the recovery deadline. */ }
      await delay(200);
    }
    assert.ok(recovered, 'Controller did not reconnect within 45 seconds');
    report.recoveryMs = Date.now() - lost;
    for (const endpoint of ['/api/agents', '/api/projects', '/api/threads', '/api/computer']) {
      const result = await request(endpoint);
      report.after[endpoint] = result.status;
      assert.equal(result.status, 200, `${endpoint} remained unavailable`);
    }
    const account = await request('/api/account');
    assert.equal(account.value.status, 'signed_in');
    assert.equal((await request(path + '/activate', {})).status, 200);
    const after = await request(path);
    assert.equal(after.status, 200);
    assert.equal(after.value.activeTurn, null);
    assert.equal(after.value.pending, false);
    assert.equal(after.value.outcomeUnknown, false);
    report.historyAfter = history(after.value);
    assert.equal(report.historyAfter, report.historyBefore, 'History changed without a send');
    assert.notEqual(after.value.eventCursor.split(':')[0], before.value.eventCursor.split(':')[0],
      'The controller event epoch did not change');
    const blocked = await fetch(base + '/api/health', {
      headers: { Origin: 'https://invalid.example' }, signal: AbortSignal.timeout(3000),
    });
    assert.equal(blocked.status, 403);
    report.passed = true;
  } finally {
    await writeFile(join(directory, 'report.json'), JSON.stringify(report, null, 2), { mode: 0o600 });
    t.diagnostic(`Recovery evidence: ${directory}`);
  }
});
