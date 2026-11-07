// Verifies evidence without releasing real reservations.
import assert from 'node:assert/strict';
// Compiles and calls disposable descriptor probes.
import { execFileSync, spawnSync } from 'node:child_process';
// Creates isolated files and inspects publication.
import { closeSync, constants, existsSync, fstatSync, mkdtempSync, openSync,
  readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
// Locates fixtures outside every retained installation.
import { tmpdir } from 'node:os';
// Constructs only disposable fixture file paths.
import { join } from 'node:path';
// Runs focused evidence and classification checks.
import { test } from 'node:test';
// Resolves owned source for the compiler.
import { fileURLToPath } from 'node:url';
// Exercises experimental publication without production settlement.
import { classifyRecovery, publishReceipt } from '../../scripts/experiments/recovery-proof.mjs';

const receipt = { version: 1, lease: 'lease-a', attempt: 'attempt-a', role: 'master',
  proof: 'guest_did_stop', identity: 'synthetic-generation-a' };
const evidence = { lease: 'lease-a', attempt: 'attempt-a', role: 'master',
  phase: 'spawn_intent', fenceHeld: true, guestLockHeld: true,
  identity: 'synthetic-generation-a', observedIdentity: 'synthetic-generation-a',
  publicationConfirmed: true, receipt };

test('recovery classification stays isolated', async t => {
  await t.test('matching native confirmation is only a candidate', () => {
    assert.deepEqual(classifyRecovery(evidence), {
      classification: 'confirmed_stop', productionRelease: false,
    });
  });
  await t.test('prepared phase can prove no spawn intent', () => {
    assert.equal(classifyRecovery({ ...evidence, phase: 'prepared', receipt: null })
      .classification, 'never_started');
  });
  for (const [name, patch] of [
    ['no native receipt after intent', { receipt: null }],
    ['unconfirmed publication', { publicationConfirmed: false }],
    ['nonboolean publication', { publicationConfirmed: 'yes' }],
    ['missing prepared lease', { phase: 'prepared', lease: undefined }],
    ['missing prepared attempt', { phase: 'prepared', attempt: undefined }],
    ['invalid prepared role', { phase: 'prepared', role: 'other' }],
    ['missing generation proof', { identity: null, observedIdentity: null }],
    ['replaced generation', { observedIdentity: 'new-generation' }],
    ['launcher fence busy', { fenceHeld: false }],
    ['native guest lock busy', { guestLockHeld: false }],
    ['stale lease', { receipt: { ...receipt, lease: 'old-lease' } }],
    ['stale attempt', { receipt: { ...receipt, attempt: 'old-attempt' } }],
    ['wrong role', { receipt: { ...receipt, role: 'controller' } }],
    ['old receipt identity', { receipt: { ...receipt, identity: 'old-generation' } }],
    ['failed force stop', { receipt: { ...receipt, proof: 'force_stop_failed' } }],
    ['native error', { receipt: { ...receipt, proof: 'vm_failed' } }],
    ['unknown version', { receipt: { ...receipt, version: 2 } }],
  ]) {
    await t.test(name, () => {
      assert.deepEqual(classifyRecovery({ ...evidence, ...patch }), {
        classification: 'unknown', productionRelease: false,
      });
    });
  }
  await t.test('forced stop preserves forced classification', () => {
    assert.equal(classifyRecovery({ ...evidence,
      receipt: { ...receipt, proof: 'force_stop_success' } }).classification, 'forced_stop');
  });
});

test('disposable identity and publication', async t => {
  const dir = mkdtempSync(join(tmpdir(), 'mc-recovery-proof-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const binary = join(dir, 'file-evidence');
  execFileSync('/usr/bin/clang', ['-Wall', '-Wextra', '-Werror', fileURLToPath(
    new URL('../../scripts/experiments/file-evidence.c', import.meta.url)), '-o', binary]);

  // Calls the helper with one explicitly inherited fixture descriptor.
  function call(mode, fd) {
    const result = spawnSync(binary, [mode], {
      stdio: ['ignore', 'pipe', 'pipe', fd], encoding: 'utf8', timeout: 5_000,
    });
    assert.ifError(result.error);
    if (result.status !== 0) throw new Error(`Probe refused: ${mode}`);
    return result.stdout.trim() ? JSON.parse(result.stdout) : null;
  }
  // Flushes only a descriptor already opened by the isolated publisher.
  function syncFile(fd) { call('sync-file', fd); }
  // Flushes only the fixture directory descriptor supplied by the publisher.
  function syncDirectory(fd) { call('sync-directory', fd); }

  await t.test('descriptor identity survives rename but not replacement', () => {
    const path = join(dir, 'identity');
    writeFileSync(path, 'fixture', { mode: 0o600 });
    const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    try {
      const measured = call('inspect', fd);
      const actual = fstatSync(fd, { bigint: true });
      assert.equal(measured.device, String(actual.dev));
      assert.equal(measured.inode, String(actual.ino));
      assert.match(measured.generation, /^\d+$/);
      renameSync(path, `${path}.old`);
      writeFileSync(path, 'replacement', { mode: 0o600 });
      assert.deepEqual(call('inspect', fd), measured);
      const replacement = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
      try { assert.notEqual(call('inspect', replacement).inode, measured.inode); }
      finally { closeSync(replacement); }
      t.diagnostic(`Observed non-root st_gen=${measured.generation}; this alone is not immutable identity proof`);
    } finally { closeSync(fd); }
  });
  await t.test('private publication flushes before visibility', () => {
    const events = [];
    const path = publishReceipt(dir, 'receipt.json', receipt, {
      syncFile(fd) { assert.equal(existsSync(join(dir, 'receipt.json')), false);
        syncFile(fd); events.push('file'); },
      syncDirectory(fd) { syncDirectory(fd); events.push('directory'); },
    });
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), receipt);
    assert.deepEqual(events, ['file', 'directory']);
    assert.throws(() => publishReceipt(dir, 'receipt.json', { changed: true },
      { syncFile, syncDirectory }));
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), receipt);
  });
  await t.test('file sync failure publishes nothing', () => {
    assert.throws(() => publishReceipt(dir, 'file-failure.json', receipt, {
      syncFile() { throw new Error('injected file sync'); }, syncDirectory,
    }), /injected file sync/);
    assert.equal(existsSync(join(dir, 'file-failure.json')), false);
  });
  await t.test('directory sync failure never returns confirmation', () => {
    assert.throws(() => publishReceipt(dir, 'directory-failure.json', receipt, {
      syncFile, syncDirectory() { throw new Error('injected directory sync'); },
    }), /injected directory sync/);
    assert.equal(existsSync(join(dir, 'directory-failure.json')), true);
    assert.equal(classifyRecovery({ ...evidence, publicationConfirmed: false }).classification, 'unknown');
  });
  for (const phase of ['before-file-sync', 'before-directory-sync']) {
    await t.test(`publisher exit ${phase} stays unconfirmed`, () => {
      const name = `${phase}.json`;
      const result = spawnSync(process.execPath, ['--input-type=module', '-e', `
        import { spawnSync } from 'node:child_process';
        const { publishReceipt } = await import(process.argv[1]);
        publishReceipt(process.argv[2], process.argv[3], JSON.parse(process.argv[4]), {
          syncFile(fd) {
            if (process.argv[5] === 'before-file-sync') process.exit(7);
            const flushed = spawnSync(process.argv[6], ['sync-file'],
              { stdio: ['ignore', 'ignore', 'ignore', fd] });
            if (flushed.status !== 0) process.exit(8);
          },
          syncDirectory() { process.exit(7); },
        });
      `, new URL('../../scripts/experiments/recovery-proof.mjs', import.meta.url).href,
      dir, name, JSON.stringify(receipt), phase, binary], { timeout: 5_000 });
      assert.ifError(result.error);
      assert.equal(result.status, 7);
      assert.equal(existsSync(join(dir, name)), phase === 'before-directory-sync');
      assert.equal(classifyRecovery({ ...evidence, publicationConfirmed: false }).classification, 'unknown');
    });
  }
  await t.test('oversized publication is refused before file creation', () => {
    assert.throws(() => publishReceipt(dir, 'oversized.json', { text: 'x'.repeat(4096) },
      { syncFile, syncDirectory }));
    assert.equal(existsSync(join(dir, 'oversized.json')), false);
  });
  await t.test('publication cannot select a parent path', () => {
    assert.throws(() => publishReceipt(dir, '../outside.json', receipt, { syncFile, syncDirectory }));
  });
});
