// Owns the VM session and its private links.
import { startPair } from '../vm/session.mjs';
// Keeps guest authentication inside this wizard.
import { signInGuest } from './auth.mjs';
// Reports unexpected VM exit during setup.
import { waitStopped } from '../vm/readiness.mjs';
// Waits between polls while respecting cancellation.
import { setTimeout as delay } from 'node:timers/promises';

// Reads one local API resource with a short deadline.
async function read(base, path, signal) {
  const response = await fetch(`${base}${path}`, { redirect: 'error', headers: { Origin: base },
    signal: AbortSignal.any([signal, AbortSignal.timeout(5000)]) });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

// Runs first-time VM setup for a backend on the Mac: open the session, start the Mac
// backend with its links, boot the VM once, wait for the guest worker, then sign in to
// Codex inside the guest. Returns the running pieces; `close` stops the backend first.
export async function connectSetup(pair, { signal, start = startPair, launch,
  guestSetup = () => {}, macSignIn = () => {}, showCode, onOutput, onStatus = () => {},
  authenticate = signInGuest, computerUse = false,
  account = (base, pending) => read(base, '/api/account', pending),
  status = (base, pending) => read(base, '/api/vms/master/status', pending),
  wait = pending => delay(1000, undefined, { signal: pending }),
} = {}) {
  signal?.throwIfAborted();
  if (!pair?.mac) throw new Error('Setup needs a Mac-backend pair');
  const session = await start(pair, { signal, onOutput, setup: true });
  const base = `http://127.0.0.1:${pair.port}`;
  const controller = new AbortController();
  const deadline = AbortSignal.any([controller.signal, AbortSignal.timeout(30 * 60_000),
    ...(signal ? [signal] : [])]);
  let backend;
  // Stops the backend before the VMs so no request reaches a closing worker.
  const close = async () => { await backend?.close(); return session.close(); };
  try {
    backend = await launch(session.linksPath);
    const stopped = waitStopped(session, deadline).then(() => { deadline.throwIfAborted(); });
    // Polls a condition once a second until it holds, the VM exits, or setup is cancelled.
    const until = async (probe, accept) => {
      while (true) {
        deadline.throwIfAborted();
        const value = await Promise.race([probe().catch(() => null), stopped]);
        if (value && accept(value)) return value;
        await wait(deadline);
      }
    };
    onStatus('backend');
    let signedIn = await until(() => account(base, deadline), () => true);
    if (signedIn.status !== 'signed_in') {
      // VM routes need one provider signed in on the Mac.
      await macSignIn(base);
      signedIn = await until(() => account(base, deadline), value => value.status === 'signed_in');
    }
    if (session.startMaster) await Promise.race([session.startMaster(), stopped]);
    if (await Promise.race([guestSetup(deadline), stopped]) === false) {
      throw new Error('Setup cancelled; guest files were retained');
    }
    onStatus('worker');
    const state = await until(() => status(base, deadline), value => {
      if (value.state === 'failed') throw new Error(value.reason ?? 'The VM failed to start.');
      return value.ready === true;
    });
    onStatus('ready');
    await Promise.race([authenticate(base, { signal: deadline, showCode }), stopped]);
    if (computerUse) {
      const helper = state.computerReady === true ? state : await status(base, deadline);
      if (helper.computerReady !== true) {
        throw new Error('Computer Use is not installed in the VM. Run the mounted installer, then resume setup.');
      }
    }
    session.check?.();
    return { session, backend, done: session.done, close };
  } catch (error) {
    controller.abort();
    await close();
    throw error;
  } finally { controller.abort(); }
}
