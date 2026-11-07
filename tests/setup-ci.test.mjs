// Checks the Windows workflow cannot pass by selecting no tests or by trusting moving action tags.
import assert from 'node:assert/strict';
// Registers the workflow contract tests.
import { test } from 'node:test';
// Runs the counting test runner as CI does.
import { spawnSync } from 'node:child_process';
// Creates a disposable test file to select from.
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const runner = fileURLToPath(new URL('../scripts/test-at-least.mjs', import.meta.url));

// Runs the counting runner against a two-test fixture and returns its exit status and output.
async function counted(t, args) {
  const root = await mkdtemp(join(tmpdir(), 'mc-ci-count-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const file = join(root, 'sample.test.mjs');
  await writeFile(file, "import { test } from 'node:test';\ntest('alpha passes', () => {});\n"
    + "test('beta passes', () => {});\ntest('gamma fails', () => { if (process.env.MC_FAIL) throw new Error('gamma'); });\n");
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  const result = spawnSync(process.execPath, [runner, ...args, file], { encoding: 'utf8', env: { ...env, ...t.extraEnv } });
  return { status: result.status, output: result.stdout + result.stderr };
}

test('a name pattern that selects no test fails instead of passing', async t => {
  const none = await counted(t, ['1', '--test-name-pattern=no such test']);
  assert.notEqual(none.status, 0);
  assert.match(none.output, /expected at least 1 passing test, found 0/i);
});

test('the runner passes only when enough selected tests pass', async t => {
  const two = await counted(t, ['2', '--test-name-pattern=alpha|beta']);
  assert.equal(two.status, 0, two.output);
  const short = await counted(t, ['3', '--test-name-pattern=alpha|beta']);
  assert.notEqual(short.status, 0);
  assert.match(short.output, /expected at least 3 passing tests, found 2/i);
  for (const bad of [['0', '--test-name-pattern=alpha'], ['many', '--test-name-pattern=alpha'], []]) {
    assert.notEqual((await counted(t, bad)).status, 0, `minimum ${bad[0]}`);
  }
});

test('a failing selected test still fails the run', async t => {
  t.extraEnv = { MC_FAIL: '1' };
  const failed = await counted(t, ['1', '--test-name-pattern=alpha|gamma']);
  assert.notEqual(failed.status, 0);
});

test('the Windows workflow counts pattern-selected tests and pins every action to a commit', async () => {
  const workflow = await readFile(new URL('../.github/workflows/windows.yml', import.meta.url), 'utf8');
  for (const line of workflow.split('\n').filter(line => line.includes('--test-name-pattern'))) {
    assert.match(line, /node scripts\/test-at-least\.mjs [1-9]\d* /, line.trim());
  }
  const uses = [...workflow.matchAll(/^\s*-?\s*uses:\s*(\S+)/gm)].map(match => match[1]);
  assert.ok(uses.length >= 5);
  for (const action of uses) assert.match(action, /^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/, action);
});
