// MiniClaw's sitecustomize for sandboxed Windows Python (scripts/setup/python-sandbox): a no-op
// elsewhere, chains to the sitecustomize it hides, patches asyncio only after asyncio is imported,
// and on Windows gives asyncio a wake-up pipe and subprocess pipes that need no socket.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { delimiter, join, resolve } from 'node:path';

const hook = resolve('scripts/setup/python-sandbox');
const python = process.platform === 'win32' ? 'python' : 'python3';
const windows = process.platform === 'win32';

// Runs Python code with the given extra environment and returns its JSON report.
function run(code, env = {}, args = []) {
  const result = spawnSync(python, [...args, '-c', code], { encoding: 'utf8', timeout: 60000,
    env: { ...process.env, PYTHONPATH: '', MINICLAW_PYTHON_SANDBOX: '', PYTHONDONTWRITEBYTECODE: '1', ...env } });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout.trim().split('\n').at(-1));
}

async function folder(t) {
  const path = await mkdtemp(join(tmpdir(), 'mc-pyhook-'));
  t.after(() => rm(path, { recursive: true, force: true }));
  return path;
}

const REPORT = `import json, sys
hook, site = sys.modules.get("miniclaw_sitecustomize"), sys.modules.get("sitecustomize")
print(json.dumps({"file": getattr(hook, "__file__", None), "patcher": any(type(f).__name__ == "Patcher" for f in sys.meta_path),
  "site": getattr(site, "__file__", None), "user": getattr(site, "USER", None)}))`;
// Loads the hook as an ordinary module, whatever sitecustomize the interpreter already ran.
const LOAD = `import importlib.util, sys
spec = importlib.util.spec_from_file_location("miniclaw_hook_under_test", ${JSON.stringify(join(hook, 'sitecustomize.py'))})
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)`;

test('the hook does nothing without the marker or off Windows, and the startup import still works', () => {
  const plain = run(REPORT, { PYTHONPATH: hook });
  assert.equal(plain.file, join(hook, 'sitecustomize.py'));
  assert.equal(plain.patcher, false);
  const marked = run(REPORT, { PYTHONPATH: hook, MINICLAW_PYTHON_SANDBOX: '1' });
  assert.equal(marked.patcher, windows, 'patches only on Windows');
  // Only the exact marker value counts.
  assert.equal(run(REPORT, { PYTHONPATH: hook, MINICLAW_PYTHON_SANDBOX: 'yes' }).patcher, false);
  const unit = run(`${LOAD}
import json
before = len(sys.meta_path)
print(json.dumps([m.install(platform="darwin", environ={"MINICLAW_PYTHON_SANDBOX": "1"}), m.install(platform="win32", environ={}),
  m.install(platform="win32", environ={"MINICLAW_PYTHON_SANDBOX": "0"}), len(sys.meta_path) - before]))`);
  assert.deepEqual(unit, [false, false, false, 0]);
});

test('a sitecustomize the hook hides still runs, once', async t => {
  const user = join(await folder(t), 'user');
  await mkdir(user);
  await writeFile(join(user, 'sitecustomize.py'), 'import os\nUSER = os.environ.get("RUNS", "") + "ran"\nos.environ["RUNS"] = USER\n');
  for (const marker of ['', '1']) {
    const report = run(REPORT, { PYTHONPATH: [hook, user].join(delimiter), MINICLAW_PYTHON_SANDBOX: marker });
    assert.equal(report.file, join(hook, 'sitecustomize.py'));
    assert.equal(report.site, join(user, 'sitecustomize.py'));
    assert.equal(report.user, 'ran');
  }
  // Python in isolated mode ignores PYTHONPATH, so neither runs: a known gap, kept visible here.
  const isolated = run(REPORT, { PYTHONPATH: [hook, user].join(delimiter), MINICLAW_PYTHON_SANDBOX: '1' }, ['-I']);
  assert.equal(isolated.file, null);
  assert.notEqual(isolated.site, join(user, 'sitecustomize.py'));
});

test('patches apply when their module is first imported, not before', () => {
  const report = run(`${LOAD}
import json
m.PATCHES = {"calendar": lambda module: setattr(module, "PATCHED_BY_MINICLAW", True)}
sys.modules.pop("calendar", None)
installed = m.install(platform="win32", environ={"MINICLAW_PYTHON_SANDBOX": "1"})
early = "calendar" in sys.modules
import calendar
print(json.dumps([installed, early, getattr(calendar, "PATCHED_BY_MINICLAW", False), "socket" in sys.modules]))`);
  assert.deepEqual(report, [true, false, true, false]);
});

test('asyncio runs on the patched pipes on Windows', { skip: !windows && 'Windows pipes' }, () => {
  const report = run(`import asyncio, json, subprocess, sys, threading, time
import asyncio.proactor_events as pe, asyncio.windows_utils as wu
async def main():
    loop = asyncio.get_running_loop()
    future = loop.create_future()
    threading.Thread(target=lambda: (time.sleep(0.05), loop.call_soon_threadsafe(future.set_result, "woke"))).start()
    woke = await asyncio.wait_for(future, 5)
    threads = [threading.Thread(target=lambda: [loop.call_soon_threadsafe(lambda: None) for _ in range(20000)]) for _ in range(2)]
    [thread.start() for thread in threads]
    [thread.join(20) for thread in threads]
    child = await asyncio.create_subprocess_exec(sys.executable, "-c", "import sys; print(sys.stdin.read().upper())",
        stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    output, _ = await asyncio.wait_for(child.communicate(b"hello"), 30)
    return [woke, sum(thread.is_alive() for thread in threads), output.decode().strip(), type(loop._ssock).__name__]
print(json.dumps([pe.BaseProactorEventLoop._make_self_pipe.__module__, wu.pipe.__module__, asyncio.run(main())]))`,
  { PYTHONPATH: hook, MINICLAW_PYTHON_SANDBOX: '1' });
  assert.deepEqual(report, ['sitecustomize', 'sitecustomize', ['woke', 0, 'HELLO', 'PipeHandle']]);
});

test('the pipe pair is connected, local and closed once', { skip: !windows && 'Windows pipes' }, () => {
  const report = run(`${LOAD}
import json, _winapi
reader, writer = m.pipe(overlapped=(False, False))
wake = m.WakeEnd(writer)
sent = wake.send(b"ab")
data = _winapi.ReadFile(reader, 2)[0]
full = sum(wake.send(b"x" * 4096) for _ in range(8))
wake.close(); wake.close()
try:
    wake.send(b"x"); after = "sent"
except OSError:
    after = "refused"
_winapi.CloseHandle(reader)
print(json.dumps([m.PIPE_PREFIX, sent, data.decode(), full < 8 * 4096, after, wake.fileno()]))`);
  assert.deepEqual(report, ['\\\\.\\pipe\\LOCAL\\python-pipe-', 2, 'ab', true, 'refused', -1]);
});
