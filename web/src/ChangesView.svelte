<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // The changes to review: the git working tree, or, where the folder is no repository, what
  // the agent changed in this conversation.
  import { onMount } from 'svelte';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import { AlertDialog } from 'bits-ui';
  import { Button } from '$lib/components/ui/button/index.js';
  import DiffView from './DiffView.svelte';
  import type { Chat } from './chat.svelte';

  type Changed = { path: string; status: 'modified' | 'new' | 'deleted'; added: number; removed: number };
  let { chat, oncount }: { chat: Chat; oncount?: (count: number) => void } = $props();
  let recoveryPath = $state('');
  let source = $state<'git' | 'conversation'>('git');
  let files = $state<Changed[]>([]);
  let diffs = $state<Record<string, string>>({});
  let open = $state<Record<string, boolean>>({});
  let loading = $state(true), error = $state(''), confirm = $state<Changed | null>(null), discarding = $state(false);

  async function refresh() {
    loading = true;
    try {
      const next = await chat.workspace<{ source: typeof source; files: Changed[] }>('changes');
      source = next.source; files = next.files; error = ''; diffs = {};
      oncount?.(files.length);
      for (const file of files.filter(item => open[item.path])) void load(file.path);
    } catch (cause) { error = cause instanceof Error ? cause.message : 'The changes could not be read.'; oncount?.(0); }
    finally { loading = false; }
  }
  async function load(path: string) {
    try { diffs[path] = (await chat.workspace<{ diff: string }>(`diff?${new URLSearchParams({ path })}`)).diff; }
    catch { diffs[path] = ''; }
  }
  async function toggle(path: string) {
    open[path] = !open[path];
    if (open[path] && diffs[path] === undefined) await load(path);
  }
  async function discard() {
    if (!confirm) return;
    discarding = true;
    try { const result = await chat.workspace<{ recoveryPath?: string }>('discard', { path: confirm.path }); recoveryPath = result.recoveryPath ?? ''; confirm = null; await refresh(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The change could not be discarded.'; confirm = null; }
    finally { discarding = false; }
  }

  // The list follows the agent: it reloads when one of its tool calls finishes.
  const finished = $derived(chat.activities.filter(entry => entry.status !== 'running').length);
  let seen = -1;
  $effect(() => { if (finished !== seen) { const first = seen < 0; seen = finished; if (!first) void refresh(); } });
  // The list reloads after each tool call, and when the window is looked at again, which covers
  // the user's own edits made elsewhere.
  onMount(() => {
    void refresh();
    const focused = () => { void refresh(); };
    window.addEventListener('focus', focused);
    return () => window.removeEventListener('focus', focused);
  });
</script>

<div class="flex min-h-0 flex-1 flex-col">
  {#if recoveryPath}<p role="status" class="break-all border-b border-border px-3 py-2 text-xs">Discarded file saved for recovery: <span class="select-all">{recoveryPath}</span></p>{/if}
  <!-- With git the list speaks for itself. Without it, one line says why it shows something else. -->
  {#if source !== 'git' && !loading && !error}
    <p class="border-b border-border px-3 py-1.5 text-xs text-muted-foreground">No git repository here: showing what the agent changed in this conversation</p>
  {/if}
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if error}<p role="alert" class="p-4 text-sm text-muted-foreground">{error}</p>
    {:else if !files.length && !loading}<p class="p-4 text-sm text-muted-foreground">No changes. Edits the agent makes, and your own, appear here.</p>{/if}
    <ul>
      {#each files as file (file.path)}
        <li class="border-b border-border">
          <div class="flex items-center">
            <button class="changes-row" aria-expanded={!!open[file.path]} onclick={() => toggle(file.path)}>
              <ChevronRight size={12} class={`shrink-0 text-muted-foreground ${open[file.path] ? 'rotate-90' : ''}`} aria-hidden="true" />
              <span class="min-w-0 flex-1 truncate text-left font-mono text-[12.5px]" title={file.path}>{file.path}</span>
              {#if file.status !== 'modified'}<span class="changes-status {file.status}">{file.status}</span>{/if}
              <span class="shrink-0 font-mono text-[12px] font-semibold"><span class="changes-plus">+{file.added}</span> <span class="changes-minus">−{file.removed}</span></span>
            </button>
            {#if source === 'git'}
              <button class="icon-button mr-1 shrink-0" aria-label={`Discard changes to ${file.path}`} title="Discard" onclick={() => { confirm = file; }}>
                <Undo2 size={14} aria-hidden="true" /></button>
            {/if}
          </div>
          {#if open[file.path]}
            <div class="border-t border-border">
              {#if diffs[file.path] === undefined}<p class="p-3 text-xs text-muted-foreground">Loading...</p>
              {:else}<DiffView diff={diffs[file.path]} path={file.path} />{/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
</div>

<AlertDialog.Root open={!!confirm} onOpenChange={value => { if (!value && !discarding) confirm = null; }}>
  <AlertDialog.Portal>
    <AlertDialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <AlertDialog.Content class="fixed left-1/2 top-1/2 z-[70] w-[min(420px,calc(100%-24px))] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-background p-5 shadow-lg">
      <AlertDialog.Title class="text-base font-semibold">Discard changes?</AlertDialog.Title>
      <AlertDialog.Description class="mt-2 break-words text-sm text-muted-foreground">
        {#if confirm?.status === 'new'}{confirm.path} is new, so it is moved away where you can get it back: {words.recovery}.
        {:else}{confirm?.path} returns to the last commit. The changes to it are lost.{/if}
      </AlertDialog.Description>
      <div class="mt-5 flex justify-end gap-3">
        <AlertDialog.Cancel class="rounded-md border border-border px-3 py-2 text-sm" disabled={discarding}>Keep</AlertDialog.Cancel>
        <Button variant="destructive" disabled={discarding} onclick={discard}>Discard changes</Button>
      </div>
    </AlertDialog.Content>
  </AlertDialog.Portal>
</AlertDialog.Root>
