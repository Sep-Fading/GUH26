<script lang="ts">
  // Shows the message only on Windows, where the network service keeps these settings.
  import { words } from './platform.svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';

  // What the network service last said about its Windows access entries (GET /api/sandbox);
  // `unknown` while the service is stopped, with the reason the backend gives.
  type Access = { state: 'healthy' | 'repaired' | 'failed' | 'unknown'; time?: number; reason?: string };
  let access = $state<Access | null>(null);
  // Set while records of an earlier sandboxed run are unresolved: sandboxed work is paused for that reason.
  let paused = $state(''), retried = $state('');
  let busy = $state(false), done = $state(''), failure = $state(''), command = $state('');

  // Reads the sandbox report; a page that cannot reach the backend shows nothing here.
  async function load() {
    try {
      const response = await fetch('/api/sandbox', { cache: 'no-store' });
      if (response.ok) {
        const report = await response.json();
        access = report.access ?? null;
        paused = report.recovery ? report.reason ?? '' : '';
      }
    } catch { access = null; }
  }

  $effect(() => { if (words.windows) void load(); });

  // Asks the running service to set the entries up again; when it cannot, names the command
  // that does it with one administrator approval.
  async function repair() {
    busy = true; done = ''; failure = ''; command = '';
    try {
      const response = await fetch('/api/sandbox/repair', { method: 'POST', cache: 'no-store',
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }, body: '{}' });
      const body = await response.json();
      if (body.ok) {
        done = 'Network access for sandboxed tools is set up again.';
        access = body.report?.access?.state === 'failed' ? body.report.access : null;
      } else {
        failure = body.reason ?? body.error?.message ?? 'Network access could not be set up again.';
        command = body.command ?? '';
        access = body.report?.access ?? access;
      }
    } catch { failure = 'MiniClaw could not reach its backend.'; }
    finally { busy = false; }
  }

  // Runs MiniClaw's own recovery of the earlier run's records again (the same as at the next start).
  async function retry() {
    busy = true; retried = '';
    try {
      const response = await fetch('/api/sandbox/repair', { method: 'POST', cache: 'no-store',
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }, body: '{}' });
      const body = await response.json();
      paused = body.report?.recovery ? body.report.reason ?? body.reason ?? paused : '';
      if (!paused) retried = 'Sandboxed work is available again.';
    } catch { retried = 'MiniClaw could not reach its backend.'; }
    finally { busy = false; }
  }

  // Formats the service's time for a person.
  const when = (time?: number) => time ? ` on ${new Date(time * 1000).toLocaleString()}` : '';
</script>

{#if words.windows && paused}
  <div role="alert" aria-label="Sandbox recovery" class="space-y-2 rounded-md border border-destructive p-3 text-xs text-destructive">
    <p class="break-words">{paused}</p>
    <Button size="sm" variant="outline" onclick={retry} disabled={busy}>{busy ? 'Trying…' : 'Try again'}</Button>
  </div>
{:else if words.windows && retried}
  <p role="status" aria-label="Sandbox recovery" class="text-xs text-muted-foreground">{retried}</p>
{/if}
{#if words.windows && (done || (access && access.state !== 'healthy'))}
  <div role={access?.state === 'failed' || failure ? 'alert' : 'status'} aria-label="Sandbox network access"
    class="space-y-2 rounded-md border p-3 text-xs {access?.state === 'failed' || failure ? 'border-destructive text-destructive' : 'border-border text-muted-foreground'}">
    {#if access?.state === 'failed'}
      <p>Network access for sandboxed tools was turned off because Windows changed system settings.</p>
      {#if access.reason}<p class="break-words opacity-80">{access.reason}</p>{/if}
      <Button size="sm" variant="outline" onclick={repair} disabled={busy}>{busy ? 'Setting up…' : 'Set up again'}</Button>
    {:else if access?.state === 'repaired'}
      <p>Windows changed system settings{when(access.time)}. MiniClaw set up network access for sandboxed tools again.</p>
    {:else if access?.state === 'unknown'}
      <p>{access.reason ?? 'The MiniClaw network service is not running.'}</p>
    {/if}
    {#if done}<p class="text-foreground">{done}</p>{/if}
    {#if failure}
      <p>{failure}</p>
      {#if command}<p>Run <code class="font-mono">{command}</code> in a terminal. Windows asks for administrator approval once.</p>{/if}
    {/if}
  </div>
{/if}
