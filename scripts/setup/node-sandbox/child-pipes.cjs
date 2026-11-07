// MiniClaw: loaded into every Node process inside MiniClaw's Windows sandbox (`--require` in
// NODE_OPTIONS, added by the process launcher). The bundled Node 24.21.0 has libuv 1.52.1, which
// names the pipes for a child's piped standard handles and for `fork`'s channel
// `\\?\pipe\uv\<n>-<pid>`. Inside an AppContainer that name is refused, and libuv retries with the
// next number forever: the call that starts the child never returns and the whole program stops.
// libuv 1.53.0 names them in the run's own namespace (`LOCAL\`). Until the bundled Node has it,
// a child that needs such a pipe is refused at once, the way Node reports a child that cannot
// start, and one line on standard error says why. Children whose standard handles are inherited,
// ignored or open files start as before. With libuv 1.53.0 or later, or outside Windows, this
// file changes nothing.
'use strict';

const CODE = 'ERR_MINICLAW_SANDBOX_CHILD_PIPES';
const MESSAGE = 'MiniClaw\'s Windows sandbox cannot start a child process with piped standard input, output or error, or a fork channel, ' +
  'with the bundled Node yet (its libuv cannot create those pipes inside the sandbox). Use stdio "inherit", "ignore" or a file instead.';

// Whether this Node's libuv names its pipes inside the sandbox's namespace (1.53.0 or later).
function fixedLibuv(version) {
  const [major, minor] = String(version).split('.').map(Number);
  return major > 1 || (major === 1 && minor >= 53);
}

// Whether a `stdio` option, or `input`, makes libuv create a pipe for the child, as Node reads it
// (`getValidStdio` in lib/internal/child_process.js): the default and missing entries 0 to 2 are
// pipes; 'pipe', 'overlapped', 'ipc' and negative numbers are pipes. Anything Node would refuse
// is left to Node.
function needsPipe(stdio, input) {
  if (input !== undefined && input !== null) return true;
  if (stdio === undefined || stdio === null) return true;
  if (typeof stdio === 'string') return stdio === 'pipe' || stdio === 'overlapped';
  if (!Array.isArray(stdio)) return false;
  for (let index = 0; index < Math.max(stdio.length, 3); index++) {
    const entry = stdio[index];
    if (entry === undefined || entry === null) { if (index < 3) return true; continue; }
    if (entry === 'pipe' || entry === 'overlapped' || entry === 'ipc' || (typeof entry === 'number' && entry < 0)) return true;
  }
  return false;
}

// The options argument of spawnSync(file, args?, options?) and execFileSync, as Node reads it.
function optionsOf(args, options) {
  if (Array.isArray(args) || args === undefined || args === null) return options;
  return typeof args === 'object' ? args : options;
}

let told = false;
// The error a refused child gets, with the fields Node gives an error from a child that could
// not start. The first refusal of a process is also written to standard error, once.
function refusal(file, args) {
  const error = new Error(`${MESSAGE} (${file})`);
  Object.assign(error, { code: CODE, syscall: `spawn ${file}`, path: file, spawnargs: Array.isArray(args) ? args.slice(1) : [] });
  if (!told) {
    told = true;
    try { process.stderr.write(`MiniClaw ${CODE}: ${MESSAGE} Refused: ${file}\n`); } catch {}
  }
  return error;
}

// Replaces the child_process entry points that would start a piped child. `exec`, `execFile`,
// `fork` and `spawn` all go through ChildProcess.prototype.spawn; the synchronous functions are
// replaced on the module, since execSync and execFileSync call spawnSync inside Node.
function install(childProcess = require('node:child_process')) {
  const { ChildProcess } = childProcess;
  const { Readable, Writable } = require('node:stream');
  const spawn = ChildProcess.prototype.spawn;
  ChildProcess.prototype.spawn = function miniclawSpawn(options) {
    if (options === null || typeof options !== 'object' || !needsPipe(options.stdio)) return spawn.call(this, options);
    const error = refusal(options.file, options.args);
    this.spawnfile = options.file;
    this.spawnargs = Array.isArray(options.args) ? options.args : [];
    if (this._handle) { this._handle.close(); this._handle = null; }
    // Standard streams that end at once, so code that attaches to them before the error still runs.
    const ended = () => { const stream = new Readable({ read() {} }); stream.push(null); return stream; };
    this.stdin = new Writable({ write(chunk, encoding, done) { done(); } });
    this.stdout = ended();
    this.stderr = ended();
    this.stdio = [this.stdin, this.stdout, this.stderr];
    this.connected = false;
    this.send = (message, ...rest) => {
      const done = rest.find(value => typeof value === 'function');
      if (done) process.nextTick(done, error);
      return false;
    };
    this.disconnect = () => {};
    process.nextTick(() => {
      this.emit('error', error);
      this.emit('close', null, null);
    });
    return undefined;
  };
  // What spawnSync returns, and execSync and execFileSync throw, for a child that could not start.
  const outcome = { pid: 0, output: null, stdout: null, stderr: null, status: null, signal: null };
  const spawnSync = childProcess.spawnSync, execSync = childProcess.execSync, execFileSync = childProcess.execFileSync;
  childProcess.spawnSync = function miniclawSpawnSync(file, args, options) {
    const chosen = optionsOf(args, options);
    if (!needsPipe(chosen?.stdio, chosen?.input)) return spawnSync.apply(this, arguments);
    return { ...outcome, error: refusal(file, Array.isArray(args) ? [file, ...args] : [file]) };
  };
  childProcess.execFileSync = function miniclawExecFileSync(file, args, options) {
    const chosen = optionsOf(args, options);
    if (!needsPipe(chosen?.stdio, chosen?.input)) return execFileSync.apply(this, arguments);
    throw Object.assign(refusal(file, Array.isArray(args) ? [file, ...args] : [file]), outcome);
  };
  childProcess.execSync = function miniclawExecSync(command, options) {
    if (!needsPipe(options?.stdio, options?.input)) return execSync.apply(this, arguments);
    throw Object.assign(refusal(String(command), []), outcome);
  };
  // ES modules that import node:child_process see the replaced functions too.
  require('node:module').syncBuiltinESMExports();
}

if (process.platform === 'win32' && !fixedLibuv(process.versions.uv)) install();

module.exports = { CODE, MESSAGE, fixedLibuv, needsPipe, install };
