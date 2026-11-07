// Starts tracked build and backend processes.
import { spawn } from 'node:child_process';
// Reads Cargo output as separate JSON lines.
import { createInterface } from 'node:readline';
// Resolves project paths relative to this script.
import { fileURLToPath } from 'node:url';
// Builds frontend assets without opening another server.
import { build } from 'vite';

const root = fileURLToPath(new URL('../', import.meta.url));
const web = fileURLToPath(new URL('../web', import.meta.url));
let child;
let watcher;
let stopping = false;
let deadline;

// Requests child shutdown once, forcing its exit if it takes longer than five seconds.
function stop() {
  if (stopping) return;
  stopping = true;
  // A Windows backend has no SIGTERM: closing the stdin pipe it watches asks it to drain.
  if (child?.stdin) child.stdin.end();
  else child?.kill('SIGTERM');
  deadline = setTimeout(() => child?.kill('SIGKILL'), 5000);
  deadline.unref();
}

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, stop);

// Runs and tracks one child until exit, optionally passing parsed stdout lines to collect.
// `launcherStdin` gives the Windows backend a stdin pipe whose end (this launcher stopping or
// being ended) shuts it down, as start-host.mjs does for an installation.
// Throws on startup or exit failure unless the launcher is already stopping.
async function run(binary, args, collect, { launcherStdin = false } = {}) {
  child = spawn(binary, args, {
    cwd: root,
    stdio: [launcherStdin ? 'pipe' : 'inherit', collect ? 'pipe' : 'inherit', 'inherit'],
    ...(launcherStdin ? { env: { ...process.env, MINICLAW_LAUNCHER_STDIN: '1' } } : {}),
  });
  child.stdin?.on('error', () => {});
  const done = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', code => resolve(code));
  });
  const lines = collect ? createInterface({ input: child.stdout }) : null;
  lines?.on('line', line => {
    try { collect(JSON.parse(line)); }
    catch { console.error(line); }
  });
  try {
    const code = await done;
    if (!stopping && code !== 0) throw new Error(`${binary} exited with ${code}`);
  } finally {
    lines?.close();
    child = undefined;
  }
}

// Builds the UI and Rust backend, optionally starts an asset watcher, then runs the server.
// Returns when the backend exits or startup is cancelled; setup failures throw.
async function start() {
  const args = process.argv.slice(2);
  if (args.length && (args.length !== 1 || args[0] !== '--watch')) {
    throw new Error('Use npm start or npm run dev; set MINICLAW_PORT explicitly if needed.');
  }
  await build({ root: web });
  if (stopping) return;

  // Read the executable path from Cargo instead of assuming a target directory.
  let executable;
  await run('cargo', [
    'build', '--package', 'miniclaw-server', '--message-format=json-render-diagnostics',
  ], message => {
    if (message.reason === 'compiler-artifact'
        && message.target?.name === 'miniclaw-server' && message.executable) {
      executable = message.executable;
    }
  });
  if (stopping) return;
  if (!executable) throw new Error('Cargo did not produce the MiniClaw executable.');

  // Watch builds write assets to disk; Rust remains the only HTTP server.
  if (args[0] === '--watch') {
    watcher = await build({ root: web, build: { watch: {}, emptyOutDir: false } });
  }
  if (!stopping) await run(executable, [], undefined, { launcherStdin: process.platform === 'win32' });
}

try {
  await start();
} catch (error) {
  if (!stopping) {
    console.error(error.message);
    process.exitCode = 1;
  }
} finally {
  stop();
  await watcher?.close();
  clearTimeout(deadline);
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.off(signal, stop);
}
