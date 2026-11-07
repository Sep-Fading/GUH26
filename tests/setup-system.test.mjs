// Checks actual loopback port availability behavior.
import assert from 'node:assert/strict';
// Runs local-only installation preflight acceptance cases.
import { test } from 'node:test';
// Holds an occupied port during validation.
import { createServer } from 'node:net';
// Waits until fixture sockets are listening.
import { once } from 'node:events';
// Exercises actual installer port and capacity probes.
import { checkPort, inspectHost, run } from '../scripts/setup/system.mjs';

test('port probe refuses conflicts and leaves no listener behind', async t => {
  const server = createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  await assert.rejects(checkPort(port), /unavailable/);
  await new Promise(resolve => server.close(resolve));
  await checkPort(port);
  await checkPort(port);
  for (const invalid of [0, 80, 65536]) await assert.rejects(checkPort(invalid), /port/i);
});

test('host inspection resolves the volume of a new directory', async () => {
  const host = await inspectHost('/private/tmp/miniclaw-not-created/nested');
  assert.ok(host.cpuCount > 0);
  assert.ok(host.memoryBytes > 0);
  assert.ok(host.diskBytes >= host.freeBytes && host.freeBytes > 0);
});

test('a cancelled build reports cancellation, not a command failure with its log', {
  skip: process.platform === 'win32' && 'POSIX process group: the Windows taskkill case is below',
}, async () => {
  const controller = new AbortController();
  const running = run('/bin/sh', ['-c', 'echo building; sleep 30'], { signal: controller.signal });
  setTimeout(() => controller.abort(), 100);
  await assert.rejects(running, error => {
    assert.equal(error.message, 'Setup cancelled');
    assert.doesNotMatch(error.message, /building|Command failed/);
    return true;
  });
  await assert.rejects(run('/bin/sh', ['-c', 'echo broken >&2; exit 3']), /Command failed \(3\)[\s\S]*broken/);
});

// Windows has no process group to signal: cancellation runs taskkill /T, which must also end
// the build's own children. The build is Node itself, so no shell is involved.
test('a cancelled Windows build stops its whole process tree and reports cancellation', {
  skip: process.platform !== 'win32' && 'Windows taskkill path only',
}, async () => {
  const controller = new AbortController();
  const build = "const { spawn } = require('node:child_process');"
    + "const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 30000)'], { stdio: 'ignore' });"
    + "console.log('building ' + child.pid); setTimeout(() => {}, 30000);";
  let output = '';
  const running = run(process.execPath, ['-e', build], { signal: controller.signal,
    onOutput: text => { output += text; } });
  let grandchild;
  for (let tries = 0; !grandchild && tries < 200; tries++) {
    grandchild = Number(/building (\d+)/.exec(output)?.[1]) || undefined;
    if (!grandchild) await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.ok(grandchild, `The build never started its child: ${output}`);
  controller.abort();
  await assert.rejects(running, error => {
    assert.equal(error.message, 'Setup cancelled');
    assert.doesNotMatch(error.message, /building|Command failed/);
    return true;
  });
  const alive = () => { try { process.kill(grandchild, 0); return true; } catch { return false; } };
  for (let tries = 0; alive() && tries < 100; tries++) await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(alive(), false, 'taskkill /T left the build child running');
  await assert.rejects(run(process.execPath, ['-e', "console.error('broken'); process.exit(3)"]),
    /Command failed \(3\)[\s\S]*broken/);
});
