// Checks host descriptor ownership across processes.
import assert from 'node:assert/strict';
// Compiles and starts disposable ownership probes.
import { execFileSync, spawn, spawnSync } from 'node:child_process';
// Observes child readiness and process termination.
import { once } from 'node:events';
// Creates isolated files and checks identities.
import {
  chmodSync, closeSync, constants, fstatSync, mkdtempSync, openSync, renameSync,
  rmSync, writeFileSync,
} from 'node:fs';
// Locates disposable files outside retained installations.
import { tmpdir } from 'node:os';
// Builds paths for disposable probe artifacts.
import { join } from 'node:path';
// Runs isolated operating system ownership cases.
import { test } from 'node:test';
// Resolves the native source beside tests.
import { fileURLToPath } from 'node:url';

// Starts a child that stays alive until stdin closes.
async function holder(t, code, args = [], descriptors = []) {
  const child = spawn(process.execPath, ['--input-type=module', '-e', code, ...args], {
    stdio: ['pipe', 'pipe', 'pipe', ...descriptors],
  });
  const closed = once(child, 'close');
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await closed;
  });
  let errors = '';
  child.stderr.on('data', chunk => { errors += chunk; });
  const ready = await Promise.race([
    once(child.stdout, 'data').then(([chunk]) => chunk.toString()),
    closed.then(() => { throw new Error(`Owner exited before readiness: ${errors}`); }),
  ]);
  assert.equal(ready, 'ready\n');
  return { child, closed };
}

test('session descriptor ownership', { timeout: 30_000 }, async t => {
  const dir = mkdtempSync(join(tmpdir(), 'mc-session-lock-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const binary = join(dir, 'session-lock');
  const source = fileURLToPath(new URL(
    '../../scripts/experiments/session-lock.c', import.meta.url,
  ));
  execFileSync('/usr/bin/clang', ['-Wall', '-Wextra', '-Werror', source, '-o', binary]);

  // Opens a new private file whose descriptor belongs to this test.
  function lockFile(name) {
    const path = join(dir, name);
    const fd = openSync(path, constants.O_CREAT | constants.O_EXCL
      | constants.O_RDWR | constants.O_NOFOLLOW, 0o600);
    return { path, fd };
  }

  // Acquires the inherited descriptor lock and waits for helper exit.
  function acquire(fd) {
    const result = spawnSync(binary, [], {
      stdio: ['ignore', 'pipe', 'pipe', fd], timeout: 5_000,
    });
    assert.ifError(result.error);
    return result.status;
  }

  // Tries the same inode through an independently opened descriptor.
  function contend(path) {
    const fd = openSync(path, constants.O_RDWR | constants.O_NOFOLLOW);
    try { return acquire(fd); } finally { closeSync(fd); }
  }

  await t.test('helper exit preserves parent ownership', () => {
    const { path, fd } = lockFile('parent.lock');
    try {
      assert.equal(acquire(fd), 0);
      assert.equal(contend(path), 3);
    } finally { closeSync(fd); }
    assert.equal(contend(path), 0);
  });

  await t.test('missing descriptor is refused', () => {
    assert.equal(spawnSync(binary, [], { stdio: 'ignore' }).status, 2);
  });

  await t.test('nonprivate file is refused', () => {
    const path = join(dir, 'public.lock');
    writeFileSync(path, '', { mode: 0o666 });
    chmodSync(path, 0o644);
    const fd = openSync(path, constants.O_RDWR);
    try { assert.equal(acquire(fd), 2); } finally { closeSync(fd); }
  });

  await t.test('owner death releases its descriptor', async caseTest => {
    const { path, fd } = lockFile('owner.lock');
    closeSync(fd);
    const { child, closed } = await holder(caseTest, `
      import { openSync, constants } from 'node:fs';
      import { spawnSync } from 'node:child_process';
      const fd = openSync(process.argv[1], constants.O_RDWR | constants.O_NOFOLLOW);
      const result = spawnSync(process.argv[2], [], { stdio: ['ignore', 'ignore', 'ignore', fd] });
      if (result.status !== 0) process.exit(1);
      process.stdout.write('ready\\n');
      process.stdin.resume();
    `, [path, binary]);
    assert.equal(contend(path), 3);
    child.kill('SIGKILL');
    await closed;
    assert.equal(contend(path), 0);
  });

  await t.test('explicit copy holds until final close', async caseTest => {
    const { path, fd } = lockFile('copy.lock');
    let open = true;
    try {
      assert.equal(acquire(fd), 0);
      const { child, closed } = await holder(caseTest,
        "process.stdout.write('ready\\n'); process.stdin.resume();", [], [fd]);
      closeSync(fd);
      open = false;
      assert.equal(contend(path), 3);
      child.kill('SIGKILL');
      await closed;
      assert.equal(contend(path), 0);
    } finally {
      if (open) closeSync(fd);
    }
  });

  await t.test('ordinary child does not inherit ownership', async caseTest => {
    const { path, fd } = lockFile('unrelated.lock');
    try {
      assert.equal(acquire(fd), 0);
      await holder(caseTest,
        "process.stdout.write('ready\\n'); process.stdin.resume();");
    } finally { closeSync(fd); }
    assert.equal(contend(path), 0);
  });

  await t.test('replacement path has different ownership', () => {
    const { path, fd } = lockFile('original.lock');
    try {
      assert.equal(acquire(fd), 0);
      const original = fstatSync(fd);
      renameSync(path, `${path}.retained`);
      const replacement = openSync(path, constants.O_CREAT | constants.O_EXCL
        | constants.O_RDWR, 0o600);
      try {
        const current = fstatSync(replacement);
        assert.notEqual(`${original.dev}:${original.ino}`, `${current.dev}:${current.ino}`);
        assert.equal(acquire(replacement), 0);
        assert.equal(contend(`${path}.retained`), 3);
      } finally { closeSync(replacement); }
    } finally { closeSync(fd); }
  });
});
