<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Reaches the idle routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  type Warning = { vmId: string; name: string; secondsLeft: number };
  let { chat }: { chat: Chat } = $props();
  let warnings = $state<Warning[]>([]);

  // Asks which VMs are about to shut down; a failed read keeps the last answer.
  async function refresh() {
    try {
      const next = (await chat.idleWarnings()).warnings;
      if (Array.isArray(next)) warnings = next.slice(0, 8);
    } catch { /* A later poll tries again. */ }
  }

  // Answers one warning; the VM's idle period starts again.
  async function keep(vmId: string) {
    try { await chat.keepRunning(vmId); warnings = warnings.filter(item => item.vmId !== vmId); }
    catch { await refresh(); }
  }

  $effect(() => {
    if (!chat.ready) { warnings = []; return; }
    void refresh();
    // A warning lasts 60 s, so it must be seen well within that.
    const poll = setInterval(() => { if (!document.hidden) void refresh(); }, 10000);
    const count = setInterval(() => {
      warnings = warnings.map(item => ({ ...item, secondsLeft: Math.max(0, item.secondsLeft - 1) }));
    }, 1000);
    return () => { clearInterval(poll); clearInterval(count); };
  });
</script>

{#if warnings.length}
  <div class="pointer-events-none fixed inset-x-0 top-3 z-[80] flex flex-col items-center gap-2 px-3">
    {#each warnings as warning (warning.vmId)}
      <div role="alert" class="pointer-events-auto flex max-w-full flex-wrap items-center gap-3 rounded-lg border
        border-border bg-background px-4 py-2.5 text-sm shadow-lg">
        <p class="min-w-0"><span class="font-medium">{warning.name}</span> is idle and shuts down in {warning.secondsLeft} s.</p>
        <Button size="sm" onclick={() => keep(warning.vmId)}>Keep running</Button>
      </div>
    {/each}
  </div>
{/if}
