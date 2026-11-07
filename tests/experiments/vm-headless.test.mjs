// Checks shutdown ownership and lifecycle results.
import assert from 'node:assert/strict';
// Runs isolated headless lifecycle contract cases.
import { test } from 'node:test';
// Compiles and executes controlled Swift callbacks.
import { execFileSync } from 'node:child_process';
// Removes compiler output after isolated tests.
import { mkdtemp, rm } from 'node:fs/promises';
// Locates temporary storage outside retained guests.
import { tmpdir } from 'node:os';
// Joins paths for disposable compiler output.
import { join } from 'node:path';
// Resolves source paths from this test.
import { fileURLToPath } from 'node:url';

test('headless lifecycle', { timeout: 240_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'mc-headless-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = join(dir, 'session-probe');
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/vm-headless.swift', import.meta.url,
  ));
  const harness = fileURLToPath(new URL('./support/headless-session.swift', import.meta.url));
  execFileSync('/usr/bin/xcrun', ['swiftc', '-swift-version', '6',
    '-warnings-as-errors', '-module-cache-path', join(dir, 'cache'),
    source, harness, '-o', binary], { timeout: 90_000 });

  // Runs one fake machine scenario and reads only observed callbacks.
  function check(scenario) {
    return JSON.parse(execFileSync(binary, [scenario], {
      encoding: 'utf8', timeout: scenario === 'default-deadline' ? 100_000 : 5_000,
      killSignal: 'SIGKILL',
    }));
  }

  for (const scenario of ['running', 'duplicate-start', 'early-timeout']) {
    await t.test(`${scenario} does not claim backend readiness`, () => {
      assert.deepEqual(check(scenario), {
        events: ['starting', 'vm_running'], exits: [], starts: 1, stops: 0,
      });
    });
  }

  await t.test('start failure exits once', () => {
    assert.deepEqual(check('start-failure'), {
      events: ['starting', 'start_failed'], exits: [1], starts: 1, stops: 0,
    });
  });

  await t.test('stop before startup cancels boot', () => {
    assert.deepEqual(check('stop-before-start'), {
      events: ['stopped'], exits: [0], starts: 0, stops: 0,
    });
  });

  for (const scenario of ['shutdown', 'stop-during-start', 'signal-term', 'signal-int']) {
    await t.test(`${scenario} waits for guest confirmation`, () => {
      assert.deepEqual(check(scenario), {
        events: ['starting', 'vm_running', 'shutdown_requested'],
        exits: [], starts: 1, stops: 1,
      });
    });
  }

  await t.test('confirmed stop exits only once', () => {
    assert.deepEqual(check('stopped'), {
      events: ['starting', 'vm_running', 'shutdown_requested', 'stopped'],
      exits: [0], starts: 1, stops: 1,
    });
  });

  await t.test('timeout starts power-off without releasing ownership', () => {
    assert.deepEqual(check('timeout'), {
      events: ['starting', 'vm_running', 'shutdown_requested', 'force_stopping'],
      exits: [], starts: 1, stops: 1,
    });
  });

  await t.test('failed graceful request still reaches power-off', () => {
    assert.deepEqual(check('stop-retry'), {
      events: ['starting', 'vm_running', 'shutdown_failed', 'force_stopping'],
      exits: [], starts: 1, stops: 1,
    });
  });

  for (const scenario of ['power-success', 'power-race']) {
    await t.test(`${scenario} reports forced exit once`, () => {
      assert.deepEqual(check(scenario), {
        events: ['starting', 'vm_running', 'shutdown_requested', 'force_stopping', 'forced_stopped'],
        exits: [2], starts: 1, stops: 1,
      });
    });
  }

  await t.test('default deadline fires at 90 seconds without resetting', () => {
    assert.deepEqual(check('default-deadline'), {
      events: ['starting', 'vm_running', 'shutdown_requested', 'force_stopping', 'forced_stopped'],
      exits: [2], starts: 1, stops: 1,
    });
  });

  await t.test('failed power-off retains ownership', () => {
    assert.deepEqual(check('power-failure'), {
      events: ['starting', 'vm_running', 'shutdown_requested', 'force_stopping', 'force_stop_failed'],
      exits: [], starts: 1, stops: 1,
    });
  });

  await t.test('power-off failure permits a new shutdown attempt', () => {
    assert.deepEqual(check('power-retry'), {
      events: ['starting', 'vm_running', 'shutdown_requested', 'force_stopping',
        'force_stop_failed', 'shutdown_requested', 'force_stopping', 'forced_stopped'],
      exits: [2], starts: 1, stops: 2,
    });
  });

  for (const scenario of ['timer-fire', 'timer-cancel']) {
    await t.test(`${scenario} follows the shutdown deadline`, () => {
      assert.deepEqual(check(scenario), {
        events: ['starting', 'vm_running', 'shutdown_requested',
          scenario === 'timer-fire' ? 'force_stopping' : 'stopped'],
        exits: scenario === 'timer-fire' ? [] : [0], starts: 1, stops: 1,
      });
    });
  }

  await t.test('guest failure exits without retry', () => {
    assert.deepEqual(check('guest-error'), {
      events: ['starting', 'vm_running', 'vm_failed'], exits: [1], starts: 1, stops: 0,
    });
  });

  await t.test('late start reply cannot revive a stopped guest', () => {
    assert.deepEqual(check('stopped-during-start'), {
      events: ['starting', 'stopped'], exits: [0], starts: 1, stops: 0,
    });
  });
});
