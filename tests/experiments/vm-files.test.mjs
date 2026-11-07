// Checks retained guest files across processes.
import assert from 'node:assert/strict';
// Runs explicit installed guest inspection checks.
import { test } from 'node:test';
// Starts separate supervisor processes without booting.
import { execFile } from 'node:child_process';
// Fingerprints saved platform and identity data.
import { createHash } from 'node:crypto';
// Reads metadata without changing guest storage.
import { readFile, stat } from 'node:fs/promises';
// Resolves files within the selected guest.
import { join } from 'node:path';
// Awaits bounded supervisor calls during inspection.
import { promisify } from 'node:util';

const run = promisify(execFile);

// Records identity hashes and disk metadata without reading private guest contents.
async function fingerprint(dir) {
  const result = {};
  for (const file of ['LabOwner', 'HardwareModel', 'MachineIdentifier', 'AuxiliaryStorage']) {
    result[file] = createHash('sha256').update(await readFile(join(dir, file))).digest('hex');
  }
  const disk = await stat(join(dir, 'Disk.img'), { bigint: true });
  assert.equal(disk.size, 64n * 1024n ** 3n);
  result.disk = { size: disk.size, inode: disk.ino, modified: disk.mtimeNs };
  return result;
}

test('installed guest reopens without replacing storage', { timeout: 30_000 }, async () => {
  const dir = process.env.MINICLAW_VM_TEST_DIR;
  const binary = process.env.MINICLAW_VM_LAB_BIN;
  assert.ok(dir && binary, 'Set MINICLAW_VM_TEST_DIR and MINICLAW_VM_LAB_BIN');
  const before = await fingerprint(dir);
  for (let attempt = 0; attempt < 2; attempt++) {
    const { stdout } = await run(binary, ['inspect', dir], { timeout: 10_000 });
    const guest = JSON.parse(stdout);
    assert.equal(guest.status, 'installed');
    assert.equal(guest.cpuCount, 4);
    assert.equal(guest.memoryBytes, 8 * 1024 ** 3);
    assert.equal(guest.networks, 0);
    assert.equal(guest.shares, 0);
    assert.equal(guest.sockets, 0);
    assert.deepEqual(await fingerprint(dir), before);
  }
});
