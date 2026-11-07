<script lang="ts">
  // The right-hand panel: the VM's screen, the working folder's files, the changes to review,
  // and what the agent did. On a wide window it is a side panel that can be dragged wider or
  // folded to a rail; on a narrow one it is a sheet opened from a button.
  import { Dialog } from 'bits-ui';
  import MonitorIcon from '@lucide/svelte/icons/monitor';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import GitCompare from '@lucide/svelte/icons/git-compare-arrows';
  import ListChecks from '@lucide/svelte/icons/list-checks';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import PanelRight from '@lucide/svelte/icons/panel-right';
  import X from '@lucide/svelte/icons/x';
  import DesktopView from './DesktopView.svelte';
  import FilesView from './FilesView.svelte';
  import ChangesView from './ChangesView.svelte';
  import ActivityList from './ActivityList.svelte';
  // The conversation's shared shell.
  import Terminal from './Terminal.svelte';
  import SquareTerminal from '@lucide/svelte/icons/square-terminal';
  // The agents this conversation handed tasks to, one card each.
  import AgentCards from './AgentCards.svelte';
  import Users from '@lucide/svelte/icons/users';
  // A project's issues, and the count waiting for review.
  import IssuesPanel from './IssuesPanel.svelte';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  // Every issue in its stage, and the live change signal.
  import BoardPanel from './BoardPanel.svelte';
  import SquareKanban from '@lucide/svelte/icons/square-kanban';
  import { watchIssues } from './project-state.svelte';
  import type { Chat } from './chat.svelte';

  type Tab = 'vm' | 'files' | 'changes' | 'terminal' | 'activity' | 'agents' | 'issues' | 'board';
  let { chat }: { chat: Chat } = $props();

  const vm = $derived(chat.machine === 'vm');
  // Only a project's conversation has issues.
  const project = $derived(chat.active?.projectId ?? '');
  const tabs = $derived([
    ...(vm ? [{ id: 'vm' as Tab, label: 'VM', icon: MonitorIcon }] : []),
    { id: 'files' as Tab, label: 'Files', icon: FolderTree },
    { id: 'changes' as Tab, label: 'Changes', icon: GitCompare },
    { id: 'terminal' as Tab, label: 'Terminal', icon: SquareTerminal },
    { id: 'activity' as Tab, label: 'Activity', icon: ListChecks },
    { id: 'agents' as Tab, label: 'Agents', icon: Users },
    ...(project ? [{ id: 'issues' as Tab, label: 'Issues', icon: CircleDot }, { id: 'board' as Tab, label: 'Board', icon: SquareKanban }] : []),
  ]);
  // How many delegated agents are working or waiting, shown on the tab.
  const working = $derived(chat.delegations.filter(run => run.status === 'running' || run.status === 'asking').length);

  // Width, tab and fold are reading preferences, so they are remembered for this browser.
  function recall<T>(key: string, fallback: T): T {
    try { const saved = localStorage.getItem(`miniclaw.workspace.${key}`); return saved === null ? fallback : JSON.parse(saved); }
    catch { return fallback; }
  }
  function keep(key: string, value: unknown) {
    try { localStorage.setItem(`miniclaw.workspace.${key}`, JSON.stringify(value)); } catch { /* Kept for this visit only. */ }
  }
  let width = $state(Math.min(900, Math.max(320, recall('width', 420))));
  let tab = $state<Tab | null>(recall('tab', null));
  // A VM conversation opens on its screen, as it always has; others start folded.
  let folded = $state<boolean | null>(recall('folded', null));
  let sheet = $state(false);
  let changeCount = $state<number | null>(null);
  let issueCount = $state(0);
  // Bumped whenever the project's issues change, so the Issues and Board tabs reload at once.
  let issuesLive = $state(0);
  // An issue chosen on the Board, opened in the Issues tab; the sequence remounts it for a repeat choice.
  let openIssue = $state<number | null>(null);
  let openSeq = $state(0);

  // Listens live to the project's issues and keeps the review count; an old project's reply is dropped.
  $effect(() => {
    const id = project;
    issueCount = 0;
    if (!id) return;
    let live = true;
    const count = () => chat.projects.issues(id, '', 'review')
      .then(reply => { if (live) issueCount = reply.issues.length; }, () => {});
    void count();
    const stop = watchIssues(id, () => { if (!live) return; issuesLive++; void count(); });
    return () => { live = false; stop(); };
  });

  // Opens one issue from the Board in the Issues tab.
  function openFromBoard(number: number) { openIssue = number; openSeq++; open('issues'); }

  const collapsed = $derived(folded ?? !vm);
  // With nothing remembered, a VM conversation shows its screen and any other its changes.
  const current = $derived<Tab>(tab && tabs.some(item => item.id === tab) ? tab : vm ? 'vm' : 'changes');

  // Only one layout is mounted at a time, so the VM's stream is opened once.
  let wide = $state(true);
  $effect(() => {
    if (typeof matchMedia !== 'function') return;
    const query = matchMedia('(min-width: 1024px)');
    const update = () => { wide = query.matches; };
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });

  function open(next: Tab) { tab = next; folded = false; keep('tab', next); keep('folded', false); }
  function fold() { folded = true; keep('folded', true); }
  function resize(next: number) { width = Math.min(900, Math.max(320, Math.round(next))); keep('width', width); }

  // Dragging the left edge resizes; the arrow keys do the same for keyboard users.
  function drag(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    const move = (moved: PointerEvent) => resize(window.innerWidth - moved.clientX);
    const stop = () => { handle.removeEventListener('pointermove', move); handle.removeEventListener('pointerup', stop); };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', stop);
  }
  function nudge(event: KeyboardEvent) {
    if (event.key === 'ArrowLeft') { event.preventDefault(); resize(width + 24); }
    if (event.key === 'ArrowRight') { event.preventDefault(); resize(width - 24); }
  }
</script>

{#snippet body()}
  <!-- Six tabs, or eight with a project's Issues and Board, do not fit a narrow panel with their words, so below 34rem
       (42rem with a project) each shows its icon; the word stays as the tab's name for screen readers and in its tooltip. -->
  <div role="tablist" aria-label="Workspace views" class="@container flex min-w-0 flex-1 gap-0.5">
    {#each tabs as item (item.id)}
      <button role="tab" aria-selected={current === item.id} class="workspace-tab" title={item.label} onclick={() => open(item.id)}>
        <item.icon size={14} aria-hidden="true" /><span class={project ? 'sr-only @[42rem]:not-sr-only' : 'sr-only @[34rem]:not-sr-only'}>{item.label}</span>{#if item.id === 'changes' && changeCount}<span class="workspace-count">{changeCount}</span>{/if}{#if item.id === 'agents' && working}<span class="workspace-count">{working}</span>{/if}{#if item.id === 'issues' && issueCount}<span class="workspace-count">{issueCount}</span>{/if}
      </button>
    {/each}
  </div>
{/snippet}

{#snippet view()}
  <div role="tabpanel" class="flex min-h-0 flex-1 flex-col overflow-hidden">
    {#if current === 'vm'}
      {#key chat.vmTarget}<DesktopView {chat} embedded />{/key}
    {:else if current === 'files'}
      {#key chat.active?.id}<FilesView {chat} onchanges={() => open('changes')} />{/key}
    {:else if current === 'changes'}
      {#key chat.active?.id}<ChangesView {chat} oncount={count => { changeCount = count; }} />{/key}
    {:else if current === 'terminal'}
      {#key chat.active?.id}<Terminal {chat} />{/key}
    {:else if current === 'issues'}
      {#key `${project}:${openSeq}`}<IssuesPanel {chat} projectId={project} live={issuesLive} open={openIssue} onopened={() => { openIssue = null; }}
        oncount={count => { issueCount = count; }} />{/key}
    {:else if current === 'board'}
      {#key project}<BoardPanel {chat} projectId={project} live={issuesLive} onopen={openFromBoard} />{/key}
    {:else if current === 'agents'}
      <AgentCards {chat} onshow={() => { sheet = false; }} />
    {:else}
      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        {#if chat.activities.length}<ActivityList entries={chat.activities} label="Activity in this conversation" />
        {:else}<p class="p-3 text-sm text-muted-foreground">Nothing yet. Commands, reads and edits the agent makes appear here as they happen.</p>{/if}
      </div>
    {/if}
  </div>
{/snippet}

{#if wide}
<!-- Wide windows: a side panel, or its rail. -->
<aside aria-label="Workspace" class="workspace flex shrink-0 border-l border-border bg-background"
  style:width={collapsed ? undefined : `${width}px`}>
  {#if collapsed}
    <div class="flex w-11 flex-col items-center gap-1 py-2">
      <button class="icon-button" aria-label="Expand workspace" title="Expand workspace" onclick={() => open(current)}>
        <PanelRight size={16} aria-hidden="true" /></button>
      {#each tabs as item (item.id)}
        <button class="icon-button relative" aria-label={`Open ${item.label}`} title={item.label} onclick={() => open(item.id)}>
          <item.icon size={16} aria-hidden="true" />
          {#if (item.id === 'changes' && changeCount) || (item.id === 'issues' && issueCount)}<span class="workspace-dot" aria-hidden="true"></span>{/if}
        </button>
      {/each}
    </div>
  {:else}
    <!-- A focusable separator with a value is the ARIA window-splitter pattern. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div role="separator" aria-orientation="vertical" aria-label="Resize workspace" aria-valuenow={width} aria-valuemin={320}
      aria-valuemax={900} tabindex="0" class="workspace-handle" onpointerdown={drag} onkeydown={nudge}></div>
    <div class="flex min-w-0 flex-1 flex-col">
      <div class="flex items-center gap-1 border-b border-border px-2 py-1.5">
        {@render body()}
        <button class="icon-button" aria-label="Collapse workspace" title="Collapse" onclick={fold}><PanelRightClose size={16} aria-hidden="true" /></button>
      </div>
      {@render view()}
    </div>
  {/if}
</aside>

{:else}
<!-- Narrow windows: the VM keeps its place above the chat; the rest opens as a sheet. -->
<div class="order-first">
  {#if vm}{#key chat.vmTarget}<DesktopView {chat} />{/key}{/if}
  <Dialog.Root open={sheet} onOpenChange={value => { sheet = value; }}>
    <Dialog.Trigger class="workspace-open" aria-label="Open workspace"><PanelRight size={16} aria-hidden="true" />Workspace</Dialog.Trigger>
    <Dialog.Portal>
      <Dialog.Overlay class="fixed inset-0 z-40 bg-black/40" />
      <Dialog.Content aria-label="Workspace" class="fixed inset-x-0 bottom-0 top-10 z-50 flex flex-col rounded-t-xl border border-border bg-background shadow-xl">
        <div class="flex items-center gap-1 border-b border-border px-2 py-1.5">
          <Dialog.Title class="sr-only">Workspace</Dialog.Title>
          {@render body()}
          <Dialog.Close class="icon-button" aria-label="Close workspace"><X size={16} aria-hidden="true" /></Dialog.Close>
        </div>
        {#if sheet}{@render view()}{/if}
      </Dialog.Content>
    </Dialog.Portal>
  </Dialog.Root>
</div>
{/if}
