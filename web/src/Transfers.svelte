<script lang="ts">
  // Keeps dialog focus and dismissal accessible.
  import { Dialog } from 'bits-ui';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Marks the trigger, folders, files and the way back up.
  import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right';
  import Folder from '@lucide/svelte/icons/folder';
  import FileIcon from '@lucide/svelte/icons/file';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import X from '@lucide/svelte/icons/x';
  // Reaches the transfer routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  type Entry = { name: string; kind: 'file' | 'folder'; size: number | null };
  type View = { state: 'running' | 'conflict' | 'done' | 'failed' | 'cancelled'; totalFiles: number;
    doneFiles: number; totalBytes: number; doneBytes: number; skipped: number; current: string;
    conflict: string | null; error: string | null };

  let { chat, vmId, disabled = false }: { chat: Chat; vmId: string; disabled?: boolean } = $props();
  let open = $state(false);
  let tab = $state<'to_vm' | 'to_mac'>('to_vm');
  let selection = $state<{ selectionId: string; items: Entry[] } | null>(null);
  let folder = $state<'workspace' | 'transfers'>('workspace');
  let path = $state('');
  let entries = $state<Entry[]>([]);
  let chosen = $state<{ folder: string; path: string }[]>([]);
  let jobId = $state(''), view = $state<View | null>(null);
  let busy = $state(false), error = $state(''), applyAll = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  const size = (bytes: number | null) => bytes === null ? '' : bytes < 1024 ? `${bytes} B`
    : bytes < 1024 ** 2 ? `${(bytes / 1024).toFixed(0)} KB` : bytes < 1024 ** 3
      ? `${(bytes / 1024 ** 2).toFixed(1)} MB` : `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  const active = $derived(view?.state === 'running' || view?.state === 'conflict');
  const full = (name: string) => path ? `${path}/${name}` : name;

  // Runs one request and shows its failure in words.
  async function attempt<T>(work: () => Promise<T>) {
    busy = true; error = '';
    try { return await work(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The request failed.'; }
    finally { busy = false; }
  }

  // Asks the backend to open the macOS panel. Cancelling it changes nothing.
  async function pick(kind: 'files' | 'folders') {
    const reply = await attempt(() => chat.transferPick(vmId, kind));
    if (reply && 'selectionId' in reply) selection = reply;
  }

  // Lists one of the VM's two offered folders.
  async function browse(nextFolder = folder, nextPath = path) {
    const reply = await attempt(() => chat.transferFiles(vmId, nextFolder, nextPath));
    if (reply) { folder = nextFolder; path = nextPath; entries = reply.entries; }
  }

  // Follows the transfer until it is done, failed or cancelled.
  function follow(id: string) {
    jobId = id; view = null; applyAll = false;
    const read = async () => {
      try { view = await chat.transferView(id); } catch { /* A later poll tries again. */ }
      if (view && !['running', 'conflict'].includes(view.state)) { clearInterval(timer); selection = null; chosen = []; }
    };
    clearInterval(timer);
    timer = setInterval(read, 500);
    void read();
  }

  async function send() {
    if (!selection) return;
    const reply = await attempt(() => chat.transferStart(vmId, { direction: 'to_vm', selectionId: selection!.selectionId }));
    if (reply) follow(reply.jobId);
  }

  // The Mac folder is chosen last, in the macOS panel, and serves this one transfer.
  async function save() {
    const destination = await attempt(() => chat.transferPick(vmId, 'destination'));
    if (!destination || !('selectionId' in destination)) return;
    const reply = await attempt(() => chat.transferStart(vmId,
      { direction: 'to_mac', selectionId: destination.selectionId, sources: $state.snapshot(chosen) }));
    if (reply) follow(reply.jobId);
  }

  function toggle(name: string, checked: boolean) {
    const item = { folder, path: full(name) };
    chosen = checked ? [...chosen, item] : chosen.filter(one => one.folder !== item.folder || one.path !== item.path);
  }

  function opened(value: boolean) {
    if (!value && active) return;
    open = value; error = '';
    if (!value) { clearInterval(timer); view = null; jobId = ''; }
    else if (tab === 'to_mac') void browse();
  }

  $effect(() => () => clearInterval(timer));
</script>

<Dialog.Root {open} onOpenChange={opened}>
  <Dialog.Trigger class="icon-button hover:bg-border/60" title="Transfer files" aria-label="Transfer files" {disabled}>
    <ArrowLeftRight size={15} aria-hidden="true" />
  </Dialog.Trigger>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-1/2 z-50 flex max-h-[85dvh] w-[min(520px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-background shadow-lg">
      <div class="flex items-center justify-between border-b border-border px-5 py-3">
        <Dialog.Title class="text-base font-semibold">Transfer files</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close transfer files" disabled={active}><X size={18} aria-hidden="true" /></Dialog.Close>
      </div>
      <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-5 text-sm">
        {#if view}
          <div class="space-y-3">
            <progress aria-label="Transfer progress" class="h-2 w-full" max={Math.max(view.totalBytes, 1)} value={view.doneBytes}></progress>
            <p class="text-xs text-muted-foreground">{view.doneFiles} of {view.totalFiles} files ·
              {size(view.doneBytes)} of {size(view.totalBytes)}{active && view.current ? ` · ${view.current}` : ''}</p>
            {#if view.state === 'conflict'}
              <div role="group" aria-label="Name conflict" class="rounded-lg border border-border p-3">
                <p><span class="font-medium [overflow-wrap:anywhere]">{view.conflict}</span> already exists at the destination.</p>
                <label class="mt-2 flex items-center gap-2 text-xs"><input type="checkbox" bind:checked={applyAll} />Apply to all</label>
                <div class="mt-3 flex flex-wrap gap-2">
                  {#each [['replace', 'Replace'], ['keep_both', 'Keep both'], ['skip', 'Skip']] as [choice, label] (choice)}
                    <Button variant={choice === 'keep_both' ? 'default' : 'secondary'} size="sm" disabled={busy}
                      onclick={() => attempt(() => chat.transferResolve(jobId, choice, applyAll))}>{label}</Button>
                  {/each}
                </div>
              </div>
            {/if}
            {#if active}
              <Button variant="outline" size="sm" disabled={busy}
                onclick={() => attempt(() => chat.transferCancel(jobId))}>Cancel transfer</Button>
            {:else if view.state === 'done'}
              <p role="status" class="text-success">{view.doneFiles} file{view.doneFiles === 1 ? '' : 's'} transferred.{view.skipped
                ? ` ${view.skipped} skipped.` : ''}{tab === 'to_vm' ? ' They are in /Users/Shared/MiniClaw/Transfers in the VM.' : ''}</p>
            {:else if view.state === 'cancelled'}
              <p role="status">Cancelled. Files that arrived whole were kept; the unfinished file was removed.</p>
            {:else}
              <p role="alert" class="text-destructive">{view.error} {view.doneFiles} of {view.totalFiles} files arrived whole;
                the unfinished file was removed.</p>
            {/if}
            {#if !active}<Button variant="secondary" size="sm" onclick={() => { view = null; jobId = ''; if (tab === 'to_mac') void browse(); }}>New transfer</Button>{/if}
          </div>
        {:else}
          <div role="tablist" aria-label="Direction" class="flex gap-1 rounded-lg bg-muted p-1">
            {#each [['to_vm', 'To VM'], ['to_mac', 'From VM']] as [value, label] (value)}
              <button role="tab" aria-selected={tab === value} class="flex-1 rounded-md px-3 py-1.5 text-sm"
                class:bg-background={tab === value} class:shadow-sm={tab === value}
                onclick={() => { tab = value as typeof tab; error = ''; if (value === 'to_mac') void browse(); }}>{label}</button>
            {/each}
          </div>
          {#if tab === 'to_vm'}
            <p class="text-xs text-muted-foreground">Files land in <code class="rounded bg-muted px-1">/Users/Shared/MiniClaw/Transfers</code>
              in the VM, in a folder named after today. Links are not copied.</p>
            <div class="flex flex-wrap gap-2">
              <Button variant="secondary" size="sm" disabled={busy} onclick={() => pick('files')}>Choose files on this Mac</Button>
              <Button variant="secondary" size="sm" disabled={busy} onclick={() => pick('folders')}>Choose folders on this Mac</Button>
            </div>
            {#if busy}<p class="text-xs text-muted-foreground">Answer the macOS panel. It may open behind this window.</p>{/if}
            {#if selection}
              <ul class="divide-y divide-border rounded-lg border border-border">
                {#each selection.items as item (item.name)}
                  <li class="flex items-center gap-2 px-3 py-2">
                    {#if item.kind === 'folder'}<Folder size={15} aria-hidden="true" />{:else}<FileIcon size={15} aria-hidden="true" />{/if}
                    <span class="min-w-0 flex-1 truncate">{item.name}</span>
                    <span class="text-xs text-muted-foreground">{item.kind === 'folder' ? 'folder' : size(item.size)}</span>
                  </li>
                {/each}
              </ul>
            {/if}
            <Button disabled={busy || !selection} onclick={send}>Send to VM</Button>
          {:else}
            <div class="flex flex-wrap items-center gap-2">
              <select aria-label="VM folder" class="rounded-md border border-border bg-background px-2 py-1 text-sm"
                value={folder} onchange={event => browse(event.currentTarget.value as typeof folder, '')}>
                <option value="workspace">Workspace</option><option value="transfers">Transfers</option>
              </select>
              {#if path}
                <button class="flex items-center gap-1 text-xs text-brand" onclick={() => browse(folder, path.split('/').slice(0, -1).join('/'))}>
                  <ChevronLeft size={14} aria-hidden="true" />Up</button>
                <span class="min-w-0 truncate text-xs text-muted-foreground">{path}</span>
              {/if}
            </div>
            <ul class="max-h-64 divide-y divide-border overflow-y-auto rounded-lg border border-border">
              {#each entries as entry (entry.name)}
                <li class="flex items-center gap-2 px-3 py-2">
                  <input type="checkbox" aria-label={entry.name}
                    checked={chosen.some(one => one.folder === folder && one.path === full(entry.name))}
                    onchange={event => toggle(entry.name, event.currentTarget.checked)} />
                  {#if entry.kind === 'folder'}
                    <button class="flex min-w-0 flex-1 items-center gap-2 text-left" aria-label={`Open folder ${entry.name}`}
                      onclick={() => browse(folder, full(entry.name))}>
                      <Folder size={15} aria-hidden="true" /><span class="truncate">{entry.name}</span></button>
                  {:else}
                    <FileIcon size={15} aria-hidden="true" /><span class="min-w-0 flex-1 truncate">{entry.name}</span>
                    <span class="text-xs text-muted-foreground">{size(entry.size)}</span>
                  {/if}
                </li>
              {:else}
                <li class="px-3 py-4 text-xs text-muted-foreground">{busy ? 'Loading...' : 'This folder is empty.'}</li>
              {/each}
            </ul>
            <p class="text-xs text-muted-foreground">{chosen.length} selected. Only the VM's Workspace and Transfers folders are offered.</p>
            <Button disabled={busy || !chosen.length} onclick={save}>Save to this Mac</Button>
          {/if}
        {/if}
        {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
      </div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
