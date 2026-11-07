// Verifies the approved Apple restore download.
import assert from 'node:assert/strict';
// Runs explicit large image integrity checks.
import { test } from 'node:test';
// Hashes the image without loading everything.
import { createHash } from 'node:crypto';
// Streams the restore image from disk.
import { createReadStream } from 'node:fs';
// Checks exact file size before hashing.
import { stat } from 'node:fs/promises';

test('approved restore image matches Apple checksum', { timeout: 300_000 }, async t => {
  const path = process.env.MINICLAW_RESTORE_IMAGE;
  assert.ok(path, 'Set MINICLAW_RESTORE_IMAGE to the downloaded lab image');
  assert.equal((await stat(path)).size, 19_772_231_540);
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  const digest = hash.digest('hex');
  t.diagnostic(`SHA256 ${digest}`);
  assert.equal(digest,
    '885503b7f4b06609e9a512f2befd40f59730640a3f1233e3892d60affdd51c95');
});
