// Verifies dispatch boundaries using synthetic adapters.
import assert from 'node:assert/strict';
// Compiles isolated policy and native adapter.
import { execFileSync } from 'node:child_process';
// Removes temporary compiled artifacts after checks.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates compiler output outside retained guests.
import { tmpdir } from 'node:os';
// Joins paths for isolated compiler artifacts.
import { join } from 'node:path';
// Runs policy cases without native actions.
import { test } from 'node:test';
// Resolves experiment sources from this harness.
import { fileURLToPath } from 'node:url';

test('native scope dispatch', { timeout: 120_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-native-scope-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = join(dir, 'native-scope-probe');
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/native-scope-policy.swift', import.meta.url,
  ));
  const harness = fileURLToPath(new URL('./support/native-scope.swift', import.meta.url));
  const native = fileURLToPath(new URL(
    '../../scripts/experiments/native-scope-worker.swift', import.meta.url,
  ));
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6',
    '-warnings-as-errors', '-module-cache-path', join(dir, 'cache'),
    source, native, harness, '-o', binary], { timeout: 90_000 });

  // Reads one fake dispatch journal without starting any GUI process.
  function check(scenario) {
    return JSON.parse(execFileSync(binary, [scenario], {
      encoding: 'utf8', timeout: 5_000,
    }));
  }

  for (const operation of ['observe', 'open', 'replaceText', 'save']) {
    await t.test(`${operation} uses the bound document`, () => {
      assert.deepEqual(check(operation), { statuses: ['ok'],
        calls: [`${operation}:import-1`], observations: ['synthetic-only'] });
    });
  }

  await t.test('save verification waits only for file bytes', () => {
    assert.deepEqual(check('save-check-delayed'), { verified: true, reads: 3 });
    assert.deepEqual(check('save-check-failed'), { verified: false, reads: 5 });
  });

  await t.test('edit evidence requires changed bytes', () => {
    assert.deepEqual(check('unchanged-edit'), { allowed: false });
  });

  for (const [scenario, status] of [
    ['wrong-key', 'unauthenticated'], ['extra-field', 'invalidRequest'],
    ['unknown-document', 'invalidRequest'], ['generic-operation', 'invalidRequest'],
    ['missing-text', 'invalidRequest'], ['unexpected-text', 'invalidRequest'],
    ['large-request', 'tooLarge'], ['read-only', 'readOnly'],
    ['stale-generation', 'stale'], ['stale-revision', 'stale'],
    ['sequence-gap', 'replay'], ['inactive', 'stale'],
    ['wrong-session', 'stale'], ['process-reused', 'stale'], ['revoked', 'revoked'],
  ]) {
    await t.test(`${scenario} refuses before dispatch`, () => {
      assert.deepEqual(check(scenario), { statuses: [status], calls: [], observations: [] });
    });
  }

  await t.test('replay dispatches only once', () => {
    assert.deepEqual(check('replay'), { statuses: ['ok', 'replay'],
      calls: ['observe:import-1'], observations: ['synthetic-only'] });
  });

  await t.test('overlapping calls cannot enter the adapter', () => {
    assert.deepEqual(check('overlap'), { statuses: ['ok', 'busy'],
      calls: ['observe:import-1'], observations: ['synthetic-only'] });
  });

  for (const [scenario, status] of [
    ['revoke-during-edit', 'revoked'], ['adapter-failure-edit', 'adapterFailed'],
    ['large-response-edit', 'tooLarge'],
  ]) {
    await t.test(`${scenario} reports an uncertain mutation`, () => {
      assert.deepEqual(check(scenario), { statuses: [status],
        calls: ['replaceText:import-1'], observations: [], uncertain: true });
    });
  }

  for (const [scenario, status] of [
    ['switch-during-call', 'stale'], ['revoke-during-call', 'revoked'],
    ['adapter-failure', 'adapterFailed'], ['large-response', 'tooLarge'],
  ]) {
    await t.test(`${scenario} discards observations`, () => {
      assert.deepEqual(check(scenario), { statuses: [status],
        calls: ['observe:import-1'], observations: [] });
    });
  }
});
