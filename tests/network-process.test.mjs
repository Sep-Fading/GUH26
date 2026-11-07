// Checks readiness and owned helper cleanup.
import assert from 'node:assert/strict';
// Creates disposable fake helper executables only.
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
// Locates private files outside retained installations.
import { join } from 'node:path';
// Runs each process lifecycle case independently.
import { test } from 'node:test';
// Exercises the production network process owner.
import { startNetwork } from '../scripts/vm/network-process.mjs';

test('network readiness is explicit and shutdown reaps the helper', async t => {
  const root = await mkdtemp('/private/tmp/mc-network-owner-');
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'helper');
  await writeFile(binary, `#!${process.execPath}
process.on('SIGTERM',()=>process.exit(0));
setInterval(()=>{},1000);
console.log(JSON.stringify({status:'network_ready'}));
`, { mode: 0o700 });
  const helper = await startNetwork(binary, root);
  t.after(() => helper.close());
  assert.equal(await helper.close(), 0);
  assert.equal(await helper.done, 0);
  assert.equal(await helper.close(), 0);
});

test('network startup failure cannot silently start offline', async t => {
  const root = await mkdtemp('/private/tmp/mc-network-failure-');
  t.after(() => rm(root, { recursive: true, force: true }));
  const binary = join(root, 'helper');
  await writeFile(binary, `#!${process.execPath}
console.log(JSON.stringify({status:'not_ready'}));
process.exit(1);
`, { mode: 0o700 });
  await assert.rejects(startNetwork(binary, root), /network helper/i);
  await assert.rejects(startNetwork(join(root, 'missing'), root), /network helper/i);
});
