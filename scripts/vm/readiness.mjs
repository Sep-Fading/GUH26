// Delays health polling without blocking shutdown.
import { setTimeout as delay } from 'node:timers/promises';

// Checks the local API without following redirects or logging response bodies.
async function readHealth(base, signal) {
  const response = await fetch(`${base}/api/health`, {
    redirect: 'error', signal: AbortSignal.any([signal, AbortSignal.timeout(2000)]),
  });
  if (!response.ok) throw new Error(`Health check returned HTTP ${response.status}`);
  return response.json();
}

// Waits for the installed backend and worker; reports transitions and bounded failures.
export async function waitReady(base, session, { signal, timeoutMs = 126_000,
  onStatus = () => {}, health = readHealth,
  wait = signal => delay(1000, undefined, { signal }),
} = {}) {
  signal?.throwIfAborted();
  const controller = new AbortController();
  const pending = AbortSignal.any([controller.signal, ...(signal ? [signal] : [])]);
  let stage = 'backend';
  const timer = setTimeout(() => controller.abort(
    new Error(`VM startup timed out waiting for ${stage}. Check the Master login and guest installer.`),
  ), timeoutMs);
  // Uses one failure race, so a hung health request cannot outlive startup.
  const stopped = waitStopped(session, pending).then(() => { throw pending.reason; });
  try {
    onStatus(stage);
    await Promise.race([stopped, (async () => {
      while (true) {
        pending.throwIfAborted();
        session.check?.();
        const healthStatus = await health(base, pending).catch(() => null);
        pending.throwIfAborted();
        const next = healthStatus?.backend !== 'ok' ? 'backend'
          : healthStatus.codex === 'connected' ? 'ready' : 'worker';
        if (next !== stage) { stage = next; onStatus(stage); }
        if (stage === 'ready') return;
        await wait(pending);
      }
    })()]);
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
}

// Reports unexpected VM termination after startup.
export async function waitStopped(session, signal) {
  if (signal?.aborted) return;
  let stop;
  try {
    await Promise.race([
      session.done?.then(code => {
        if (!signal?.aborted) throw new Error(`VM stopped unexpectedly (exit ${code})`);
      }) ?? new Promise(() => {}),
      new Promise(resolve => {
        stop = resolve;
        signal?.addEventListener('abort', stop, { once: true });
      }),
    ]);
  } finally { signal?.removeEventListener('abort', stop); }
}
