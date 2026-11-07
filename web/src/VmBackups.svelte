<script lang="ts">
  // Loads the VM's backup settings when shown.
  import { onMount } from 'svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Reaches the backup routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  type View = { experimental: boolean; settings: { schedule: 'off' | 'daily' | 'weekly'; keep: number;
    lastRun: number | null; lastError: string | null };
    backups: { id: string; created: number; files: number; bytes: number }[] };
  let { chat, vmId, running }: { chat: Chat; vmId: string; running: boolean } = $props();
  let view = $state<View | null>(null);
  let busy = $state(false), error = $state(''), note = $state('');

  const size = (bytes: number) => bytes < 1024 ** 2 ? `${Math.ceil(bytes / 1024)} KB` : `${(bytes / 1024 ** 2).toFixed(1)} MB`;

  // Runs one request and shows its failure in words.
  async function attempt(work: () => Promise<unknown>, done = '') {
    busy = true; error = ''; note = '';
    try { await work(); note = done; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The request failed.'; }
    finally { busy = false; }
  }
  const load = () => attempt(async () => { view = await chat.vmBackups<View>(vmId); });
  const save = (schedule: string, keep: number) =>
    attempt(async () => { view = await chat.vmBackups<View>(vmId, 'settings', { schedule, keep }); }, 'Saved');

  onMount(load);
</script>

{#if view}
  <div role="group" aria-label="Backups" class="rounded-lg border border-dashed border-border p-3 text-sm">
    <p class="font-medium">Backups <span class="ml-1 rounded bg-muted px-1.5 py-0.5 text-[11px] font-normal text-muted-foreground">Experimental</span></p>
    <p class="mt-1 text-xs text-muted-foreground">Copies this VM's Workspace folder to this Mac. Off until you choose a schedule
      or press Back up now. It never starts a VM and is not a whole-disk backup.</p>
    <div class="mt-3 flex flex-wrap items-center gap-3 text-xs">
      <label class="flex items-center gap-2">Schedule
        <select aria-label="Backup schedule" class="rounded-md border border-border bg-background px-2 py-1"
          value={view.settings.schedule} disabled={busy} onchange={event => save(event.currentTarget.value, view!.settings.keep)}>
          <option value="off">Off</option><option value="daily">Daily</option><option value="weekly">Weekly</option>
        </select></label>
      <label class="flex items-center gap-2">Keep
        <select aria-label="Backups to keep" class="rounded-md border border-border bg-background px-2 py-1"
          value={String(view.settings.keep)} disabled={busy} onchange={event => save(view!.settings.schedule, Number(event.currentTarget.value))}>
          {#each Array.from({ length: 10 }, (_, index) => index + 1) as count (count)}<option value={String(count)}>{count}</option>{/each}
        </select></label>
      <Button variant="secondary" size="sm" disabled={busy || !running}
        title={running ? '' : 'The VM must be running'} onclick={() => attempt(() => chat.vmBackups(vmId, 'run', {}),
          'Backup started. It appears here when every file has arrived.')}>Back up now</Button>
    </div>
    {#if view.settings.lastError}<p class="mt-2 text-xs text-destructive">Last backup failed: {view.settings.lastError}</p>{/if}
    {#if view.backups.length}
      <ul class="mt-3 divide-y divide-border text-xs">
        {#each view.backups as backup (backup.id)}
          <li class="flex items-center gap-2 py-1.5">
            <span class="min-w-0 flex-1">{new Date(backup.created * 1000).toLocaleString()} · {backup.files} files · {size(backup.bytes)}</span>
            <Button variant="ghost" size="sm" disabled={busy || !running}
              onclick={() => attempt(() => chat.vmBackups(vmId, `${backup.id}/restore`, {}),
                `Restoring into the VM's Transfers folder as restore-${backup.id}. The Workspace is not overwritten.`)}>Restore</Button>
          </li>
        {/each}
      </ul>
    {/if}
    <p role="status" class="mt-2 text-xs text-success">{note}</p>
    {#if error}<p role="alert" class="mt-1 text-xs text-destructive">{error}</p>{/if}
  </div>
{/if}
