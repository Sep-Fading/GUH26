<script lang="ts">
  // A read-only look at the conversation's working folder: a tree, and a viewer.
  import { onMount } from 'svelte';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Folder from '@lucide/svelte/icons/folder';
  import FileIcon from '@lucide/svelte/icons/file';
  import { highlightLines } from './message-markdown';
  import { languageOf } from './diff';
  import type { Chat } from './chat.svelte';

  type Entry = { name: string; kind: 'file' | 'folder'; size: number };
  let { chat, onchanges }: { chat: Chat; onchanges?: () => void } = $props();
  let root = $state(''), error = $state('');
  let folders = $state<Record<string, Entry[]>>({});
  let open = $state<Record<string, boolean>>({});
  let shown = $state<{ path: string; text: string | null; reason?: string } | null>(null);

  const join = (folder: string, name: string) => folder ? `${folder}/${name}` : name;
  const size = (bytes: number) => bytes < 1024 ? `${bytes} B` : bytes < 1024 ** 2 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 ** 2).toFixed(1)} MB`;

  async function load(path: string) {
    try {
      const listed = await chat.workspace<{ root: string; entries: Entry[] }>(`files?${new URLSearchParams({ path })}`);
      root = listed.root; folders[path] = listed.entries; error = '';
    } catch (cause) { error = cause instanceof Error ? cause.message : 'The folder could not be read.'; }
  }
  async function toggle(path: string) {
    open[path] = !open[path];
    if (open[path] && !folders[path]) await load(path);
  }
  async function show(path: string) {
    try { shown = await chat.workspace(`file?${new URLSearchParams({ path })}`); }
    catch (cause) { shown = { path, text: null, reason: cause instanceof Error ? cause.message : 'The file could not be read.' }; }
  }
  const lines = $derived(shown?.text == null ? [] : highlightLines(shown.text, languageOf(shown.path)));

  onMount(() => { void load(''); });
</script>

{#snippet tree(folder: string, depth: number)}
  <ul>
    {#each folders[folder] ?? [] as entry (entry.name)}
      {@const path = join(folder, entry.name)}
      <li>
        {#if entry.kind === 'folder'}
          <button class="files-row" style:padding-left={`${depth * 14 + 8}px`} aria-expanded={!!open[path]} onclick={() => toggle(path)}>
            <ChevronRight size={12} class={`shrink-0 text-muted-foreground ${open[path] ? 'rotate-90' : ''}`} aria-hidden="true" />
            <Folder size={14} class="shrink-0 text-muted-foreground" aria-hidden="true" /><span class="truncate">{entry.name}</span>
          </button>
          {#if open[path]}{@render tree(path, depth + 1)}{/if}
        {:else}
          <button class="files-row" class:files-current={shown?.path === path} style:padding-left={`${depth * 14 + 26}px`} onclick={() => show(path)}>
            <FileIcon size={14} class="shrink-0 text-muted-foreground" aria-hidden="true" /><span class="truncate">{entry.name}</span>
            <span class="ml-auto shrink-0 pl-2 text-[11px] text-muted-foreground">{size(entry.size)}</span>
          </button>
        {/if}
      </li>
    {:else}
      <li class="px-3 py-1 text-xs text-muted-foreground" style:padding-left={`${depth * 14 + 26}px`}>Empty</li>
    {/each}
  </ul>
{/snippet}

<div class="flex min-h-0 flex-1 flex-col">
  {#if error}
    <p role="alert" class="p-4 text-sm text-muted-foreground">{error}</p>
  {:else}
    <p class="truncate border-b border-border px-3 py-1.5 font-mono text-[11px] text-muted-foreground" title={root} style="direction: rtl; text-align: left">{root}</p>
    <nav aria-label="Files" class="max-h-[40%] min-h-24 shrink-0 overflow-y-auto border-b border-border py-1 text-[13px]">{@render tree('', 0)}</nav>
    {#if shown}
      <section aria-label={shown.path} class="flex min-h-0 flex-1 flex-col">
        <div class="flex items-center gap-2 border-b border-border px-3 py-1.5 text-xs">
          <span class="min-w-0 flex-1 truncate font-mono">{shown.path}</span>
          {#if onchanges}<button class="shrink-0 text-brand" onclick={onchanges}>See changes</button>{/if}
        </div>
        {#if shown.text === null}
          <p class="p-4 text-sm text-muted-foreground">{shown.reason}</p>
        {:else}
          <div class="min-h-0 flex-1 overflow-auto">
            <table class="fileview"><tbody>
              {#each lines as line, index (index)}
                <tr><td class="number">{index + 1}</td><td class="code">{@html line || ' '}</td></tr>
              {/each}
            </tbody></table>
          </div>
        {/if}
      </section>
    {:else}
      <p class="p-4 text-sm text-muted-foreground">Choose a file to read it. Nothing can be edited here.</p>
    {/if}
  {/if}
</div>
