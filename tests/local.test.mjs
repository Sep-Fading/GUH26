import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import { dirname, join, win32 } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { eventually, freePort, json, peerFixture, stopChild } from './support/harness.mjs';

const exec = promisify(execFile);
const root = new URL('../', import.meta.url);
const windows = process.platform === 'win32';

// Windows has npm only as npm.cmd, which needs cmd.exe; Node runs npm's own entry script instead,
// with fixed arguments and no shell.
function npm(args) {
  if (!windows) return ['npm', args];
  const cli = [process.env.npm_execpath, join(dirname(process.execPath), 'node_modules', 'npm', 'bin', 'npm-cli.js')]
    .find(path => path && /npm-cli\.js$/i.test(path) && existsSync(path));
  assert.ok(cli, 'npm-cli.js was not found beside Node');
  return [process.execPath, [cli, ...args]];
}

// Windows has no process groups, lsof or ps: PowerShell reads the launcher's process tree and the
// TCP listeners it owns, printed in lsof's -Fn shape. The process id travels in the environment.
const TREE = '$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId; '
  + '$set = [System.Collections.Generic.HashSet[int]]::new(); [void]$set.Add([int]$env:MC_ROOT_PID); '
  + 'do { $added = $false; foreach ($p in $all) { if ($set.Contains([int]$p.ParentProcessId) -and $set.Add([int]$p.ProcessId)) { $added = $true } } } while ($added); ';
async function powershell(script, pid) {
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await exec(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script],
    { env: { ...process.env, MC_ROOT_PID: String(pid) }, timeout: 60000, windowsHide: true });
  return stdout.split(/\r?\n/).filter(Boolean);
}
const alive = pid => { try { process.kill(Number(pid), 0); return true; } catch { return false; } };

async function group(pid) {
  // A live process tree on Windows; processes whose launcher already exited are not in it.
  if (windows) return powershell(`${TREE}$set | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }`, pid);
  const { stdout } = await exec('ps', ['-eo', 'pid=,pgid=']);
  return stdout.trim().split('\n').map(line => line.trim().split(/\s+/))
    .filter(([, parent]) => Number(parent) === pid).map(([id]) => id);
}

async function listeners(pid) {
  const pids = await group(pid);
  assert.ok(pids.length, 'Expected the launcher process group');
  if (windows) {
    return powershell(`${TREE}Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | `
      + 'Where-Object { $set.Contains([int]$_.OwningProcess) } | '
      + 'ForEach-Object { "p$($_.OwningProcess)"; "n$($_.LocalAddress):$($_.LocalPort)" }', pid);
  }
  // Linux without lsof: ss names each listener's owning processes; printed in lsof's -Fn shape.
  if (process.platform === 'linux') {
    const { stdout } = await exec('ss', ['-ltnpH']);
    const mine = new Set(pids.map(String));
    return stdout.split('\n').filter(line => [...line.matchAll(/pid=(\d+)/g)].some(([, id]) => mine.has(id)))
      .flatMap(line => [`p${[...line.matchAll(/pid=(\d+)/g)].map(([, id]) => id).find(id => mine.has(id))}`,
        `n${line.trim().split(/\s+/)[3]}`]);
  }
  // lsof exits 1 when a listed process has already exited (the group includes short-lived
  // helpers) while still printing every listener it found. Any other failure is real.
  const { stdout } = await exec('lsof', [
    '-nP', '-a', '-p', pids.join(','), '-iTCP', '-sTCP:LISTEN', '-Fn',
  ]).catch(error => {
    if (error.code !== 1 || error.stderr || typeof error.stdout !== 'string') throw error;
    return error;
  });
  return stdout.split('\n');
}

test('direct Vite servers are disabled', async t => {
  for (const args of [[], ['preview']]) {
    const child = spawn(process.execPath, ['node_modules/vite/bin/vite.js', ...args, 'web'],
      { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
    t.after(() => stopChild(child));
    let logs = '';
    for (const stream of [child.stdout, child.stderr]) {
      stream.on('data', chunk => { logs = (logs + chunk).slice(-4000); });
    }
    // A cold Vite start on the Windows ARM64 guest (config bundling, Svelte and Tailwind native
    // modules, Defender scanning) took longer than the default 4 s bound in the native sweep.
    const code = await eventually(() => child.exitCode,
      value => value !== null, 'Vite refuses a separate server', windows ? 60000 : undefined);
    assert.notEqual(code, 0);
    assert.match(logs, /npm (start|run dev)/);
  }
});

for (const mode of ['dev', 'start']) {
  // Windows builds the page and the backend more slowly; the bounds there are wider.
  test(`${mode} uses one local listener`, { timeout: windows ? 300000 : 60000 }, async t => {
    const peer = await peerFixture();
    const port = await freePort();
    const url = `http://127.0.0.1:${port}`;
    const [binary, args] = npm(['run', mode]);
    const child = spawn(binary, args, {
      // A POSIX process group lets the test signal everything at once; Windows uses taskkill /T.
      cwd: root, detached: !windows, windowsHide: true,
      env: { ...process.env, MINICLAW_PORT: String(port),
        MINICLAW_DATA_DIR: peer.dir, MINICLAW_CHAT_DIR: peer.cwd,
        CODEX_HOME: peer.codexHome, CODEX_BIN: peer.binary,
        // Never the user's own Claude Code. On Windows its tool callback is a second, authenticated
        // loopback TCP listener (claude::tool_socket); on the Mac it is a Unix socket (VM2, 2026-10-04).
        CLAUDE_BIN: '', CLAUDE_CONFIG_DIR: join(peer.dir, 'claude-home') },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let logs = '';
    for (const stream of [child.stdout, child.stderr]) {
      stream.on('data', chunk => { logs = (logs + chunk).slice(-8000); });
    }
    const exited = once(child, 'exit');
    const signal = value => {
      try { process.kill(-child.pid, value); }
      catch (error) { if (error.code !== 'ESRCH') throw error; }
    };
    if (windows) t.after(async () => {
      // This tree contains only this test's npm, launcher, backend and fake Codex runtime.
      if (child.exitCode === null && child.signalCode === null) {
        await exec(win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'taskkill.exe'),
          ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true }).catch(() => {});
        await exited;
      }
      await peer.cleanup();
    });
    else t.after(async () => {
      // This group contains only this test's launcher and fake Codex runtime.
      signal('SIGTERM');
      const timer = setTimeout(() => signal('SIGKILL'), 1500);
      try { await exited; } finally { clearTimeout(timer); }
      signal('SIGKILL');
      await peer.cleanup();
    });
    await once(child, 'spawn');
    await eventually(async () => {
      assert.equal(child.exitCode, null, logs);
      try {
        const response = await fetch(`${url}/api/health`, {
          signal: AbortSignal.timeout(500),
        });
        return (await json(response)).backend;
      } catch { return null; }
    }, value => value === 'ok', `single-interface ${mode}: ${url}`, windows ? 240000 : 30000);

    const page = await fetch(url);
    assert.match(page.headers.get('content-type'), /text\/html/);
    const dom = new JSDOM(await page.text(), { url });
    const script = dom.window.document.querySelector('script[type="module"]');
    assert.ok(script, 'Expected the built frontend');
    assert.equal(new URL(script.src).origin, url);
    assert.equal((await fetch(script.src)).status, 200);
    dom.window.close();
    const sockets = await listeners(child.pid);
    assert.deepEqual(sockets.filter(line => line.startsWith('n')),
      [`n127.0.0.1:${port}`]);
    assert.equal((await json(await fetch(`${url}/api/account`))).status, 'signed_in');

    const server = sockets.find(line => line.startsWith('p')).slice(1);
    if (windows) {
      // Stopping the launcher is ending its process: Windows has no SIGTERM, so the backend must
      // notice that its launcher's stdin pipe closed, drain and exit, with its own children.
      const running = await group(child.pid);
      const [launcher] = await powershell('(Get-CimInstance Win32_Process -Filter "ProcessId=$env:MC_ROOT_PID").ParentProcessId', server);
      process.kill(Number(launcher), 'SIGTERM');
      await exited;
      await eventually(() => running.filter(alive), pids => pids.length === 0,
        'launcher and children exit', 15000);
      await assert.rejects(fetch(`${url}/api/health`));
      return;
    }
    const { stdout } = await exec('ps', ['-p', server, '-o', 'ppid=']);
    process.kill(Number(stdout.trim()), 'SIGTERM');
    await exited;
    await eventually(() => group(child.pid), pids => pids.length === 0,
      'launcher and children exit');
    await assert.rejects(fetch(`${url}/api/health`));
  });
}
