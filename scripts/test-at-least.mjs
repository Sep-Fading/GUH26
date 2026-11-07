#!/usr/bin/env node
// Runs `node --test` and fails unless a minimum number of tests passed. A name pattern that
// matches nothing exits 0 under plain `node --test`, which would let CI pass with no test run.
// Runs the test process and waits for it.
import { spawnSync } from 'node:child_process';
// Holds the machine-readable summary beside the readable output.
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';

// Runs the tests with the given arguments and returns the exit status and passing count.
function run(args) {
  const folder = mkdtempSync(join(tmpdir(), 'mc-test-count-'));
  const summary = join(folder, 'summary.tap');
  try {
    const result = spawnSync(process.execPath, ['--test', '--test-reporter=spec', '--test-reporter-destination=stdout',
      '--test-reporter=tap', `--test-reporter-destination=${summary}`, ...args], { stdio: 'inherit' });
    // A file whose tests were all filtered out is itself reported as one passing test named
    // after the file, so only top-level results that are not a test file are counted.
    const files = args.filter(arg => !arg.startsWith('-')).map(file => basename(file));
    const passed = readFileSync(summary, 'utf8').split('\n')
      .map(line => /^ok \d+ - (.*?)(?: # .*)?$/.exec(line))
      .filter(match => match && !/ # (SKIP|TODO)/i.test(match[0]) && !files.includes(basename(match[1]))).length;
    return { status: result.status ?? 1, passed };
  } finally { rmSync(folder, { recursive: true, force: true }); }
}

const [minimum, ...args] = process.argv.slice(2);
if (!/^[1-9]\d*$/.test(minimum ?? '') || !args.length) {
  console.error('Usage: test-at-least.mjs MINIMUM [node --test options] FILES');
  process.exit(2);
}
const { status, passed } = run(args);
if (status !== 0) process.exit(status);
if (passed < Number(minimum)) {
  console.error(`Expected at least ${minimum} passing test${minimum === '1' ? '' : 's'}, found ${passed}`);
  process.exit(1);
}
