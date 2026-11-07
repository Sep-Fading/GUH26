// Checks MiniClaw's preload for sandboxed Node on Windows (scripts/setup/node-sandbox/child-pipes.cjs):
// a child that needs a pipe libuv 1.52.1 cannot create inside the sandbox is refused at once, the
// way Node reports a child that cannot start; every other child starts as before.
import assert from 'node:assert/strict';
// Runs each case in a child Node with the preload installed.
import { execFileSync } from 'node:child_process';
// Writes the probes.
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
// Locates temporary storage.
import { tmpdir } from 'node:os';
// Joins fixture paths.
import { join } from 'node:path';
// Runs the cases.
import { test } from 'node:test';
// Names the preload for a child Node.
import { fileURLToPath } from 'node:url';

const preload = fileURLToPath(new URL('../scripts/setup/node-sandbox/child-pipes.cjs', import.meta.url));
const CODE = 'ERR_MINICLAW_SANDBOX_CHILD_PIPES';
// Whether the preload acts by itself in a Node on this host: Windows with libuv before 1.53. The
// preload is never loaded into this test process, where it would refuse the probes' own pipes.
const [uvMajor, uvMinor] = process.versions.uv.split('.').map(Number);
const activeHere = process.platform === 'win32' && uvMajor === 1 && uvMinor < 53;

// Runs a probe in a child Node: `boot` installs the preload as the sandbox would on Windows, then
// the probe prints one JSON line. Bounded, so a regression that hangs fails instead of waiting.
async function probe(t, source, { module = false, install = true } = {}) {
  const folder = await mkdtemp(join(tmpdir(), 'mc-child-pipes-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const boot = join(folder, 'boot.cjs');
  await writeFile(boot, install ? `require(${JSON.stringify(preload)}).install();\n` : `require(${JSON.stringify(preload)});\n`);
  const script = join(folder, module ? 'probe.mjs' : 'probe.cjs');
  await writeFile(script, source);
  const started = Date.now();
  let stdout;
  try {
    stdout = execFileSync(process.execPath, ['--require', boot, script], { cwd: folder, encoding: 'utf8', timeout: 20000, stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (error) {
    assert.fail(`the probe failed: ${error.message}\n${error.stdout}\n${error.stderr}`);
  }
  const line = stdout.trim().split('\n').at(-1);
  return { result: JSON.parse(line), elapsed: Date.now() - started, folder };
}

test('the options that make libuv create a pipe are recognised as Node reads them', async t => {
  const piped = [undefined, null, 'pipe', 'overlapped', [], ['inherit'], ['inherit', 'inherit'], ['inherit', 'inherit', 'pipe'],
    ['ignore', 'ignore', 'ignore', 'ipc'], ['inherit', 'inherit', 'inherit', 'pipe'], [0, -1, 2], [null, 'inherit', 'inherit']];
  const plain = ['inherit', 'ignore', ['inherit', 'inherit', 'inherit'], ['ignore', 'ignore', 'ignore'], [0, 1, 2], [3, 4, 5],
    ['ignore', 'inherit', 'ignore', undefined, null], [{ fd: 3 }, 'inherit', 'ignore'], 'bogus', 42];
  const versions = ['1.52.1', '1.51.0', '1.9.9', '1.53.0', '1.54.2', '2.0.0'];
  const { result } = await probe(t, `
const { CODE, fixedLibuv, needsPipe } = require(${JSON.stringify(preload)});
const piped = ${JSON.stringify(piped)}, plain = ${JSON.stringify(plain)};
piped[0] = undefined; plain[6][3] = undefined;
console.log(JSON.stringify({ CODE, piped: piped.map(stdio => needsPipe(stdio)), plain: plain.map(stdio => needsPipe(stdio)),
  input: [needsPipe('inherit', 'text'), needsPipe('inherit', Buffer.from('x'))], versions: ${JSON.stringify(versions)}.map(fixedLibuv) }));
`, { install: false });
  assert.equal(result.CODE, CODE);
  assert.deepEqual(result.piped, piped.map(() => true));
  assert.deepEqual(result.plain, plain.map(() => false));
  // spawnSync's `input` is written through a pipe.
  assert.deepEqual(result.input, [true, true]);
  assert.deepEqual(result.versions, [false, false, false, true, true, true]);
});

test('asynchronous children with pipes are refused at once, as a child that cannot start', async t => {
  const { result, elapsed } = await probe(t, `
const cp = require('node:child_process');
const out = {};
const pending = [];
const settle = (name, child) => new Promise(resolve => {
  const events = [];
  child.stdout?.on('end', () => events.push('stdout-end'));
  child.on('error', error => events.push('error:' + error.code));
  child.on('close', () => { events.push('close'); out[name] = { events, pid: child.pid ?? null, killed: child.kill() }; resolve(); });
});
pending.push(settle('spawn', cp.spawn(process.execPath, ['-e', '1'])));
pending.push(settle('overlapped', cp.spawn(process.execPath, ['-e', '1'], { stdio: 'overlapped' })));
pending.push(new Promise(resolve => cp.exec('echo hi', (error, stdout) => { out.exec = [error?.code, stdout]; resolve(); })));
pending.push(new Promise(resolve => cp.execFile(process.execPath, ['-e', '1'], (error, stdout) => { out.execFile = [error?.code, stdout]; resolve(); })));
const fork = cp.fork('-e', [], { execArgv: [] });
out.forkSend = fork.send({ a: 1 }, error => { out.forkSendError = error?.code; });
pending.push(settle('fork', fork));
const stdin = cp.spawn(process.execPath, ['-e', '1']);
stdin.stdin.write('ignored');
stdin.stdin.end();
pending.push(settle('stdin', stdin));
Promise.all(pending).then(() => console.log(JSON.stringify(out)));
`);
  for (const name of ['spawn', 'overlapped', 'fork', 'stdin']) {
    assert.deepEqual(result[name].events.filter(event => event !== 'stdout-end'), [`error:${CODE}`, 'close'], name);
    assert.equal(result[name].pid, null, name);
    assert.equal(result[name].killed, false, name);
  }
  assert.deepEqual(result.exec, [CODE, '']);
  assert.deepEqual(result.execFile, [CODE, '']);
  assert.equal(result.forkSend, false);
  assert.equal(result.forkSendError, CODE);
  assert.ok(elapsed < 10000, `refusals took ${elapsed} ms`);
});

test('synchronous children with pipes are refused at once', async t => {
  const { result } = await probe(t, `
const cp = require('node:child_process');
const out = {};
const sync = cp.spawnSync(process.execPath, ['-e', '1']);
out.spawnSync = [sync.error?.code, sync.status, sync.pid, sync.output];
out.spawnSyncOptions = cp.spawnSync(process.execPath, { stdio: 'inherit', input: 'x' }).error?.code;
for (const [name, call] of [['execSync', () => cp.execSync('echo hi')], ['execFileSync', () => cp.execFileSync(process.execPath, ['-e', '1'])],
  ['execFileSyncOptions', () => cp.execFileSync(process.execPath, { encoding: 'utf8' })], ['execSyncInput', () => cp.execSync('echo hi', { stdio: 'inherit', input: 'x' })]]) {
  try { call(); out[name] = 'ran'; } catch (error) { out[name] = [error.code, error.status, error.stdout, error.message.includes('MiniClaw')]; }
}
console.log(JSON.stringify(out));
`);
  assert.deepEqual(result.spawnSync, [CODE, null, 0, null]);
  assert.equal(result.spawnSyncOptions, CODE);
  for (const name of ['execSync', 'execFileSync', 'execFileSyncOptions', 'execSyncInput']) assert.deepEqual(result[name], [CODE, null, null, true], name);
});

test('children with inherited, ignored or file handles still start, and ES modules see the same rule', async t => {
  const { result, folder } = await probe(t, `
import { execSync, spawn, spawnSync } from 'node:child_process';
import { closeSync, openSync, readFileSync } from 'node:fs';
const out = {};
out.inherit = spawnSync(process.execPath, ['-e', 'process.exitCode = 3'], { stdio: 'inherit' }).status;
out.ignore = spawnSync(process.execPath, ['-e', 'process.exitCode = 4'], { stdio: 'ignore' }).status;
const fd = openSync('child.txt', 'w');
out.file = spawnSync(process.execPath, ['-e', 'console.log("to a file")'], { stdio: ['ignore', fd, 'ignore'] }).status;
closeSync(fd);
out.fileText = readFileSync('child.txt', 'utf8').trim();
out.execInherit = execSync('exit 0', { stdio: 'inherit' });
try { execSync('echo hi'); out.esm = 'ran'; } catch (error) { out.esm = error.code; }
spawn(process.execPath, ['-e', 'process.exitCode = 5'], { stdio: ['ignore', 'inherit', 'inherit'] }).on('close', code => {
  out.async = code;
  console.log(JSON.stringify(out));
});
`, { module: true });
  assert.deepEqual(result, { inherit: 3, ignore: 4, file: 0, fileText: 'to a file', execInherit: null, esm: CODE, async: 5 });
  assert.ok(folder);
});

test('the refusal is reported once on standard error with its code', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-child-pipes-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const boot = join(folder, 'boot.cjs');
  await writeFile(boot, `require(${JSON.stringify(preload)}).install();\n`);
  const run = (await import('node:child_process')).spawnSync(process.execPath, ['--require', boot, '-e',
    'const cp=require("child_process");cp.spawnSync("a");cp.spawnSync("b");try{cp.execSync("c")}catch{}'], { encoding: 'utf8', timeout: 20000 });
  assert.equal(run.status, 0, run.stderr);
  const lines = run.stderr.split('\n').filter(line => line.includes(CODE));
  assert.equal(lines.length, 1, run.stderr);
  assert.match(lines[0], /^MiniClaw ERR_MINICLAW_SANDBOX_CHILD_PIPES: .* Refused: a$/);
});

test('loaded without install, the preload changes nothing unless this is Windows with libuv before 1.53', async t => {
  const { result } = await probe(t, `
const cp = require('node:child_process');
let piped;
try { piped = cp.execFileSync(process.execPath, ['-e', 'process.stdout.write("piped")'], { encoding: 'utf8' }); } catch (error) { piped = error.code; }
console.log(JSON.stringify({ piped }));
`, { install: false });
  assert.deepEqual(result, { piped: activeHere ? CODE : 'piped' });
});
