<script lang="ts">
  // Loads the VM's state and idle time when shown.
  import { onMount } from 'svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Marks the power and window commands.
  import Play from '@lucide/svelte/icons/play';
  import Power from '@lucide/svelte/icons/power';
  import AppWindow from '@lucide/svelte/icons/app-window';
  // Experimental folder backups for this VM.
  import VmBackups from './VmBackups.svelte';
  // Reaches the VM routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  let { chat, vmId, name, note = '', state: initial, children, onstate }: {
    chat: Chat; vmId: string; name: string; note?: string; state?: string;
    children?: import('svelte').Snippet<[string]>; onstate?: (state: string) => void;
  } = $props();
  let current = $state('unknown');
  // A fresh state from the parent replaces what these controls last saw.
  $effect(() => { if (initial) current = initial; });
  let minutes = $state(0), choices = $state<number[]>([0, 15, 30, 60, 120]);
  let busy = $state(false), error = $state(''), saved = $state(false);

  const words: Record<string, string> = { off: 'Off', running: 'Running', starting: 'Starting',
    stopping: 'Shutting down', failed: 'Failed', missing: 'Missing', unknown: 'Unknown' };
  const time = (value: number) => value === 0 ? 'Never' : value < 60 ? `After ${value} minutes`
    : `After ${value / 60} hour${value === 60 ? '' : 's'}`;

  // Reads the VM's state; a failure leaves the last known state in place.
  async function refresh() {
    try { current = (await chat.vmState(vmId)).state; onstate?.(current); }
    catch { /* The controls stay usable with the last known state. */ }
  }

  // Sends one power request, then follows the VM until it settles or a minute passes.
  async function power(action: 'start' | 'stop' | 'show') {
    busy = true; error = '';
    try {
      current = (await chat.vmPower(vmId, action)).state;
      for (let tries = 0; tries < 60 && ['starting', 'stopping'].includes(current); tries++) {
        await new Promise(resolve => setTimeout(resolve, tries ? 1000 : 50));
        await refresh();
      }
      onstate?.(current);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'The VM did not accept the request.';
    } finally { busy = false; }
  }

  // Saves the idle time at once; there is no separate save button.
  async function saveIdle(value: number) {
    error = ''; saved = false;
    try { minutes = (await chat.vmIdle(vmId, value)).minutes; saved = true; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The idle time was not saved.'; }
  }

  onMount(() => {
    if (!initial) void refresh();
    void chat.vmIdle(vmId).then(value => { minutes = value.minutes; choices = value.choices; }, () => {});
  });
</script>

<div role="group" aria-label={name} class="rounded-lg border border-border p-3 text-sm">
  <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
    <div class="min-w-0 flex-1">
      <p class="truncate font-medium">{name}</p>
      <p class="text-xs text-muted-foreground"><span>{words[current] ?? current}</span>{note ? ` · ${note}` : ''}</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button variant="secondary" size="sm" disabled={busy || !['off', 'failed'].includes(current)}
        onclick={() => power('start')}><Play size={14} aria-hidden="true" />Start</Button>
      <Button variant="secondary" size="sm" disabled={busy || current !== 'running'}
        onclick={() => power('show')}><AppWindow size={14} aria-hidden="true" />Open window</Button>
      <Button variant="secondary" size="sm" disabled={busy || current !== 'running'}
        onclick={() => power('stop')}><Power size={14} aria-hidden="true" />Shut down</Button>
      {@render children?.(current)}
    </div>
  </div>
  <label class="mt-3 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">Shut down when idle
    <select class="rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground"
      value={String(minutes)} onchange={event => saveIdle(Number(event.currentTarget.value))}>
      {#each choices as choice (choice)}<option value={String(choice)}>{time(choice)}</option>{/each}
    </select>
    <span role="status" class="text-success">{saved ? 'Saved' : ''}</span>
  </label>
  {#if error}<p role="alert" class="mt-2 text-xs text-destructive">{error}</p>{/if}
</div>
<div class="mt-2"><VmBackups {chat} {vmId} running={current === 'running'} /></div>
