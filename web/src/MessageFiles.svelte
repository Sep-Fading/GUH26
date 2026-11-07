<script lang="ts">
  // Marks a file that is not a picture.
  import FileText from '@lucide/svelte/icons/file-text';
  // The uploads a sent message carried.
  import type { Attachment } from './chat.svelte';

  // `compact` lays the files out from the left with small pictures, for a popup or a card.
  let { thread, files, compact = false, label = 'Attached files' }:
    { thread: string; files: Pick<Attachment, 'id' | 'name' | 'kind' | 'size'>[]; compact?: boolean; label?: string } = $props();

  // Writes a size the way Finder does, roughly.
  const size = (bytes: number) => bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB`
    : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
</script>

<ul aria-label={label} class={['mb-2 flex flex-wrap gap-2', compact ? 'justify-start' : 'justify-end']}>
  {#each files as file (file.id)}
    {@const href = `/api/threads/${thread}/attachments/${file.id}`}
    <li>
      {#if file.kind === 'image'}
        <a {href} target="_blank" rel="noreferrer" aria-label={`Open ${file.name}`}
          class="block overflow-hidden rounded-lg border border-border focus-visible:outline-2 focus-visible:outline-ring">
          <img src={href} alt={file.name} loading="lazy" class={['min-h-12 min-w-12 bg-muted object-cover', compact ? 'max-h-20 max-w-32' : 'max-h-48 max-w-64']} />
        </a>
      {:else}
        <a {href} download={file.name} aria-label={`Download ${file.name}`}
          class="flex max-w-64 items-center gap-2 rounded-lg border border-border bg-card px-3 py-2 text-sm hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring">
          <FileText size={16} class="shrink-0 text-muted-foreground" aria-hidden="true" />
          <span class="min-w-0 truncate">{file.name}</span>
          <span class="shrink-0 text-xs text-muted-foreground">{size(file.size)}</span>
        </a>
      {/if}
    </li>
  {/each}
</ul>
