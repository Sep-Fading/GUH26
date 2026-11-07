// Waits between polls while respecting cancellation.
import { setTimeout as delay } from 'node:timers/promises';

// Signs in to Codex inside the VM through the Mac backend's VM routes. The guest's own
// worker runs the device flow, so its credentials stay in the guest and the Mac's
// sign-in is never touched.
export async function signInGuest(base, { signal, fetcher = fetch, vm = 'master',
  showCode = () => {}, wait = signal => delay(1000, undefined, { signal }),
} = {}) {
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(base)) throw new Error('Setup requires a loopback API');
  const pending = AbortSignal.any([AbortSignal.timeout(15 * 60_000), ...(signal ? [signal] : [])]);
  pending.throwIfAborted();
  // Adds browser-equivalent mutation checks and a deadline to each local request.
  async function request(path, body) {
    pending.throwIfAborted();
    const response = await fetcher(`${base}${path}`, {
      signal: AbortSignal.any([pending, AbortSignal.timeout(20_000)]), redirect: 'error',
      ...(body === undefined ? {} : { method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-MiniClaw-Request': '1', Origin: base,
      }, body: JSON.stringify(body) }),
    });
    if (!response.ok) throw new Error(`Guest sign-in request failed: HTTP ${response.status}`);
    return response.json();
  }
  if ((await request(`/api/vms/${vm}/status`)).signedIn === true) return;
  const result = await request(`/api/vms/${vm}/auth/start`, { method: 'device' });
  if (result.status !== 'pending'
    || result.verificationUrl !== 'https://auth.openai.com/codex/device'
    || !/^[A-Z0-9-]{4,32}$/.test(result.userCode ?? '')) {
    throw new Error('Invalid guest sign-in response');
  }
  await showCode({ url: result.verificationUrl, code: result.userCode });
  while (true) {
    await wait(pending);
    const status = await request(`/api/vms/${vm}/status`);
    if (status.signedIn === true) return;
    if (status.ready !== true) throw new Error('The guest worker disconnected during sign-in. Resume setup to retry.');
  }
}
