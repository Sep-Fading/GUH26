<script lang="ts">
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  // Marks the commands of one queued message and of the queue.
  import Pencil from '@lucide/svelte/icons/pencil';
  import X from '@lucide/svelte/icons/x';
  import CornerDownRight from '@lucide/svelte/icons/corner-down-right';
  import Play from '@lucide/svelte/icons/play';
  import Pause from '@lucide/svelte/icons/pause';
  // Reads the queue and changes it through the backend.
  import type { Chat } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
  let editing = $state<string>(), words = $state('');

  // Why a queue is paused, in words.
  const reasons: Record<string, string> = {
    failed: 'the last response failed.', stopped: 'you stopped the last response.',
    unknown: 'the outcome of the last response is unknown.', restart: 'MiniClaw was restarted.',
    refused: 'the next message could not be sent.', user: 'you paused it.',
  };

  async function save(id: string) {
    if (!words.trim()) return;
    await chat.queueAction(id, { text: words });
    editing = undefined;
  }
</script>

{#if chat.queue.items.length}
  <section aria-label="Queued messages" class="mb-2 rounded-2xl border border-border bg-muted/50 p-2 text-sm">
    <div class="flex flex-wrap items-center gap-2 px-1 pb-1.5 text-xs text-muted-foreground">
      <span class="font-medium text-foreground">{chat.queue.items.length} queued</span>
      {#if chat.queue.paused}
        <span role="status" class="text-destructive">Paused: {reasons[chat.queue.reason ?? ''] ?? 'it is waiting for you.'}</span>
      {:else}<span>Sent one at a time, after each response.</span>{/if}
      <span class="ml-auto flex items-center gap-1">
        {#if chat.queue.paused}
          <Button size="sm" aria-label="Resume queue" onclick={() => chat.queueAction('resume')}><Play size={13} aria-hidden="true" />Resume</Button>
        {:else}
          <Button variant="ghost" size="sm" aria-label="Pause queue" onclick={() => chat.queueAction('pause')}><Pause size={13} aria-hidden="true" />Pause</Button>
        {/if}
        <Button variant="ghost" size="sm" aria-label="Clear queue" onclick={() => chat.queueAction('clear')}>Clear</Button>
      </span>
    </div>
    <ol class="max-h-44 space-y-1 overflow-y-auto">
      {#each chat.queue.items as item, index (item.id)}
        <li class="flex items-start gap-2 rounded-xl bg-background px-3 py-2">
          <span class="mt-0.5 w-4 shrink-0 text-right text-xs tabular-nums text-muted-foreground" aria-hidden="true">{index + 1}</span>
          {#if editing === item.id}
            <div class="min-w-0 flex-1 space-y-2">
              <Textarea aria-label="Queued message" rows={2} bind:value={words} maxlength={32768} />
              <div class="flex justify-end gap-2">
                <Button variant="outline" size="sm" onclick={() => { editing = undefined; }}>Cancel</Button>
                <Button size="sm" disabled={!words.trim()} onclick={() => save(item.id)}>Save</Button>
              </div>
            </div>
          {:else}
            <div class="min-w-0 flex-1">
              <p class="line-clamp-2 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">{item.text}</p>
              {#if item.attachments?.length}
                <p class="truncate text-xs text-muted-foreground">Files: {item.attachments.map(file => file.name).join(', ')}</p>
              {/if}
              {#if item.model}<p class="mt-0.5 text-xs text-muted-foreground">{item.model}{item.effort ? ` · ${item.effort}` : ''}</p>{/if}
            </div>
            <div class="flex shrink-0 items-center">
              <button type="button" class="icon-button" aria-label="Send now" title="Stop the current response and send this now"
                onclick={() => chat.queueAction(`${item.id}/send-now`)}><CornerDownRight size={15} aria-hidden="true" /></button>
              <button type="button" class="icon-button" aria-label="Edit queued message"
                onclick={() => { editing = item.id; words = item.text; }}><Pencil size={15} aria-hidden="true" /></button>
              <button type="button" class="icon-button" aria-label="Remove queued message"
                onclick={() => chat.queueAction(`${item.id}/delete`)}><X size={15} aria-hidden="true" /></button>
            </div>
          {/if}
        </li>
      {/each}
    </ol>
  </section>
{/if}
