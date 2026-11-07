<script lang="ts">
  // Keeps focus and dismissal inside the dialog.
  import { Dialog } from 'bits-ui';
  // Provides the dialog close control icon.
  import X from '@lucide/svelte/icons/x';
  // Types the destination without loading conversations.
  import type { Task } from './chat.svelte';

  export type Hit = { thread: Task; messageId: string; snippet: string; approximate: boolean };
  type Results = { results: Hit[]; indexing: boolean; indexed: number; incomplete: boolean; limited: boolean };
  let { enabled, account, onselect, open = $bindable(false) }: { enabled: boolean; account: string; open?: boolean;
    onselect: (hit: Hit) => Promise<void> } = $props();
  let query = $state('');
  let results = $state<Results | null>(null);
  let error = $state('');
  let loading = $state(false);
  let choosing = $state(false);
  let selected = $state(0);
  let revision = $state(0);
  let list = $state<HTMLElement>();
  const searchable = $derived(query.trim().split(/\s+/).some(word => [...word].length >= 3));

  $effect(() => {
    void account;
    open = false; query = ''; results = null; error = '';
  });
  $effect(() => {
    const text = query;
    const version = revision;
    if (!open || !enabled) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    results = null; selected = 0; loading = true; error = '';
    // Polls only while indexing; stale queries and closed dialogs are cancelled.
    async function read() {
      try {
        const response = await fetch(`/api/search?q=${encodeURIComponent(text.trim())}`, { signal: controller.signal });
        if (!response.ok) throw new Error('Could not search conversations. Try again.');
        const value: Results = await response.json();
        if (controller.signal.aborted) return;
        results = value; selected = Math.min(selected, Math.max(0, value.results.length - 1)); loading = false;
        if (value.indexing) timer = setTimeout(read, 500);
      } catch (cause) {
        if (!controller.signal.aborted) { results = null; error = cause instanceof Error ? cause.message : 'Search failed.'; loading = false; }
      }
    }
    void version;
    timer = setTimeout(read, 120);
    return () => { controller.abort(); clearTimeout(timer); };
  });

  // Opens search from anywhere using the standard command palette shortcut.
  function shortcut(event: KeyboardEvent) {
    if (enabled && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault(); open = !open;
    }
  }

  // Selects a result with arrows and opens it with Enter, leaving Tab navigation intact.
  function keydown(event: KeyboardEvent) {
    const hits = results?.results ?? [];
    if (!hits.length || choosing) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      selected = (selected + (event.key === 'ArrowDown' ? 1 : hits.length - 1)) % hits.length;
      list?.querySelectorAll('button')[selected]?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter') { event.preventDefault(); void choose(hits[selected]); }
  }

  // Loads the exact destination and keeps errors visible if it no longer exists.
  async function choose(hit: Hit) {
    choosing = true; error = '';
    try { await onselect(hit); open = false; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Could not open this message.'; }
    finally { choosing = false; }
  }

  // Requests a fresh pass after new history or an unavailable provider returns.
  async function refresh() {
    try {
      const response = await fetch('/api/search/refresh', { method: 'POST', headers: { 'X-MiniClaw-Request': '1', 'Content-Type': 'application/json' }, body: '{}' });
      if (!response.ok) throw new Error();
      revision += 1;
    } catch { error = 'Could not refresh search. Try again.'; }
  }
</script>

<svelte:window onkeydown={shortcut} />
<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-[10dvh] z-50 flex max-h-[80dvh] w-[min(42rem,calc(100vw-2rem))] -translate-x-1/2 flex-col overflow-hidden rounded-xl border border-border bg-background shadow-xl">
      <div class="flex items-center justify-between px-5 pt-4">
        <Dialog.Title class="font-semibold">Search conversations</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close search"><X size={18} aria-hidden="true" /></Dialog.Close>
      </div>
      <Dialog.Description class="px-5 text-sm text-muted-foreground">Search messages across your conversations. Minor spelling mistakes are OK.</Dialog.Description>
      <div class="p-4">
        <input class="w-full rounded-md border border-border bg-background px-3 py-2 text-base outline-none focus:ring-2 focus:ring-ring"
          aria-label="Search all conversations" placeholder="Search for a phrase or a few words…" maxlength={200}
          bind:value={query} onkeydown={keydown} autocomplete="off" spellcheck="false" />
      </div>
      <div class="min-h-0 overflow-y-auto px-2" bind:this={list}>
        {#each results?.results ?? [] as hit, i (`${hit.thread.id}:${hit.messageId}`)}
          <button class="mb-1 w-full rounded-md px-3 py-3 text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
            class:bg-muted={i === selected} disabled={choosing} onclick={() => choose(hit)}>
            <span class="block truncate text-sm font-medium">{hit.thread.name}</span>
            <span class="mt-1 block break-words text-sm text-muted-foreground">{hit.snippet}</span>
            {#if hit.approximate}<span class="text-xs text-muted-foreground">Similar spelling</span>{/if}
          </button>
        {/each}
      </div>
      <div class="border-t border-border px-5 py-3 text-xs text-muted-foreground">
        <p role="status">{choosing ? 'Opening message…' : loading ? 'Searching…' : results?.indexing
          ? `Indexing conversations… ${results.indexed} ready.` : !searchable ? 'Type at least 3 letters in one word.'
          : !results?.results.length ? 'No matching messages.' : `${results.results.length} matching messages`}</p>
        {#if results?.incomplete}<p class="mt-1">Some history could not be indexed. Connect its provider or start its VM, then refresh.</p>{/if}
        {#if results?.limited}<p class="mt-1">Showing the best matches found. Add more words to narrow your search.</p>{/if}
        {#if error}<p role="alert" class="mt-1 text-destructive">{error}</p>{/if}
        <button class="mt-2 underline" disabled={results?.indexing || choosing} onclick={refresh}>Refresh search</button>
      </div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
