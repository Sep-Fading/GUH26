// Starts only the installed private network helper.
import { spawn } from 'node:child_process';

// Waits for explicit readiness and owns helper shutdown, including failed startup.
export async function startNetwork(binary, root, { signal, onOutput = () => {} } = {}) {
  signal?.throwIfAborted();
  const child = spawn(binary, [root], { stdio: ['ignore', 'pipe', 'pipe'] });
  let closed = false, closing;
  const done = new Promise(resolve => {
    child.once('error', () => { closed = true; resolve(1); });
    child.once('close', code => { closed = true; resolve(code ?? 1); });
  });
  // Reaps only this helper; VM shutdown remains the paired supervisor's responsibility.
  function close() {
    closing ??= (async () => {
      if (!closed) child.kill('SIGTERM');
      const kill = setTimeout(() => { if (!closed) child.kill('SIGKILL'); }, 1000);
      try { return await done; } finally { clearTimeout(kill); }
    })();
    return closing;
  }
  let timer, abort, line = '';
  try {
    await new Promise((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('Network helper readiness timed out')), 5000);
      abort = () => reject(new Error('Network helper startup cancelled'));
      signal?.addEventListener('abort', abort, { once: true });
      done.then(() => reject(new Error('Network helper exited before readiness')));
      child.stdout.on('data', chunk => {
        line += chunk.toString();
        if (line.length > 1024) { reject(new Error('Invalid network helper status')); return; }
        if (line.trim() === '{"status":"network_ready"}') resolve();
      });
      child.stderr.on('data', chunk => onOutput(chunk.toString()));
      if (signal?.aborted) abort();
    });
    return { done, close };
  } catch (error) {
    await close();
    throw error;
  } finally {
    clearTimeout(timer);
    if (abort) signal?.removeEventListener('abort', abort);
  }
}
