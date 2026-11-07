<script lang="ts">
  // Marks a file that is not a picture, and the remove command.
  import FileText from '@lucide/svelte/icons/file-text';
  import X from '@lucide/svelte/icons/x';
  // Reads and changes the files of the draft.
  import type { Chat } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
</script>

{#if chat.files.length}
  <ul aria-label="Files to send" class="mb-2 flex flex-wrap gap-2">
    {#each chat.files as item (item.key)}
      <li class="relative flex max-w-56 items-center gap-2 rounded-lg border border-border bg-card py-1 pl-1 pr-7 text-sm">
        {#if item.preview}
          <img src={item.preview} alt="" class="size-9 shrink-0 rounded-md object-cover" />
        {:else}
          <span class="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted">
            <FileText size={16} class="text-muted-foreground" aria-hidden="true" />
          </span>
        {/if}
        <span class="min-w-0 truncate">{item.file.name || 'Pasted picture'}</span>
        <button type="button" aria-label={`Remove ${item.file.name || 'pasted picture'}`} disabled={chat.working}
          class="absolute right-1 top-1/2 flex size-5 -translate-y-1/2 items-center justify-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
          onclick={() => chat.removeFile(item.key)}>
          <X size={13} aria-hidden="true" />
        </button>
      </li>
    {/each}
  </ul>
{/if}
