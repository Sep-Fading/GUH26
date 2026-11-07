// Checks retained project-VM identity without launching macOS.
import assert from 'node:assert/strict';
// Runs isolated registry cases in parallel with other Node tests.
import { test } from 'node:test';
// Names idempotent create requests and disposable roots.
import { randomUUID } from 'node:crypto';
// Owns only each test's private registry directory.
import { chmod, mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
// Places fixtures outside the retained installation.
import { tmpdir } from 'node:os';
// Builds paths within the disposable test root.
import { join } from 'node:path';
// Exercises persisted setup and status transitions.
import { openDedicatedRegistry } from '../scripts/vm/dedicated-registry.mjs';

test('project VM create is idempotent and never aliases Master', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-dedicated-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const seen = [];
  const registry = await openDedicatedRegistry(root, {
    setup: async (machine, report) => {
      seen.push(machine);
      await report('installing', 42);
      await report('awaiting_setup', 100);
    },
  });
  t.after(() => registry.close());
  const requestId = randomUUID();
  const first = await registry.provision(requestId);
  const repeat = await registry.provision(requestId);
  assert.equal(first.vmId, repeat.vmId);
  assert.notEqual(first.vmId, 'master');
  await registry.idle(first.vmId);
  assert.equal(seen.length, 1);
  assert.equal(registry.status(first.vmId).stage, 'awaiting_setup');
  assert.equal(registry.status(first.vmId).percent, 100);
  assert.equal(registry.status('master').state, 'missing');
  const [entry] = registry.list();
  assert.equal(entry.vmId, first.vmId);
  assert.equal(entry.slot, 0);
  assert.match(entry.key, /^[a-f0-9]{64}$/);
  assert.equal(entry.cwd, '/Users/Shared/MiniClaw/Workspace');
  assert.equal(JSON.stringify(first).includes(entry.key), false);
  assert.equal(JSON.stringify(registry.status(first.vmId)).includes(entry.key), false);
  await registry.close();
  const reopened = await openDedicatedRegistry(root, { setup: async () => {} });
  t.after(() => reopened.close());
  assert.equal((await reopened.provision(requestId)).vmId, first.vmId);
  assert.equal(reopened.list()[0].key, entry.key);
});

test('failed and cancelled setup retains its ID and retries in a new attempt', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-dedicated-retry-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  const attempts = [];
  const registry = await openDedicatedRegistry(root, {
    setup: async (machine, report, signal) => {
      attempts.push(machine.attempt);
      if (machine.attempt === 1) {
        const error = new Error('/private/user/secret/restore.ipsw');
        error.code = 'resource_limit';
        throw error;
      }
      await report('installing', 10);
      await new Promise(resolve => signal.aborted ? resolve()
        : signal.addEventListener('abort', resolve, { once: true }));
    },
  });
  t.after(() => registry.close());
  const { vmId } = await registry.provision(randomUUID());
  await registry.idle(vmId);
  assert.deepEqual(attempts, [1]);
  assert.equal(registry.status(vmId).state, 'failed');
  assert.match(registry.status(vmId).reason, /resource limit/i);
  assert.doesNotMatch(JSON.stringify(registry.status(vmId)), /private|secret|ipsw/);
  await registry.retry(vmId);
  assert.equal(registry.status(vmId).stage, 'installing');
  await registry.cancel(vmId);
  await registry.idle(vmId);
  assert.deepEqual(attempts, [1, 2]);
  assert.equal(registry.status(vmId).state, 'failed');
  assert.match(registry.status(vmId).reason, /cancelled/i);
  assert.equal(registry.list()[0].vmId, vmId);
});

test('removing a project VM deletes its folder and frees its slot, never during setup', async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-dedicated-remove-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o700);
  let release;
  const held = new Promise(resolve => { release = resolve; });
  const registry = await openDedicatedRegistry(root, {
    setup: async (machine, report) => {
      await mkdir(join(root, 'dedicated-vms', machine.vmId, 'Attempt-1'), { recursive: true });
      await writeFile(join(root, 'dedicated-vms', machine.vmId, 'Attempt-1/Disk.img'), 'disk');
      await held;
      await report('awaiting_setup', 100);
    },
  });
  t.after(() => registry.close());
  const { vmId } = await registry.provision(randomUUID());
  await assert.rejects(registry.remove(vmId), /still being set up/);
  release();
  await registry.idle(vmId);
  await assert.rejects(registry.remove('master'), /does not exist/);
  await assert.rejects(registry.remove(randomUUID()), /does not exist/);
  await registry.remove(vmId);
  assert.equal(registry.status(vmId).state, 'missing');
  assert.deepEqual(registry.list(), []);
  assert.deepEqual(await readdir(join(root, 'dedicated-vms')), ['registry.json'], 'the VM folder is gone');
  await registry.close();
  const reopened = await openDedicatedRegistry(root, { setup: async () => {} });
  t.after(() => reopened.close());
  assert.deepEqual(reopened.list(), [], 'the removal was saved');
});
