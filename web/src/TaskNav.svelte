<script lang="ts">
  // Provides accessible expandable projects and tooltips.
  import { Collapsible, Tooltip } from 'bits-ui';
  // Identifies folders, expansion, and conversation creation.
  import Folder from '@lucide/svelte/icons/folder';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Plus from '@lucide/svelte/icons/plus';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Marks conversations whose agents are working.
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  // Identifies handles for moving agent sections.
  import GripVertical from '@lucide/svelte/icons/grip-vertical';
  // Opens settings for new or saved projects.
  import ProjectSettings from './ProjectSettings.svelte';
  // Confirms deletion before removing a conversation.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Words the deletion dialog for this computer.
  import { words } from './platform.svelte';
  // Lets the user name a conversation.
  import RenameChat from './RenameChat.svelte';
  // Reveals truncated names without widening rows.
  import ScrollingName from './ScrollingName.svelte';
  // Shares task selection with the conversation interface.
  import type { Chat, Task } from './chat.svelte';

  let { chat, onselect = () => {}, onswarm = () => {}, activeSwarm = null, ondeleted = () => {} }: { chat: Chat; onselect?: () => void;
    onswarm?: (id: string) => void; activeSwarm?: string | null; ondeleted?: (id: string) => void } = $props();

  // The swarms of this backend and earlier runs, newest first, refreshed every few seconds.
  type SwarmItem = { id: string; goal: string; status: string; projectId?: string | null; counts?: { done: number; total: number } };
  // Each swarm shows its Lead; its other agents' conversations fold into one group, closed at first.
  let openAgents = $state<Record<string, boolean>>({});
  const leadChat = (item: SwarmItem) => agentChats(item).find(task => task.name === 'Lead');
  const otherChats = (item: SwarmItem) => agentChats(item).filter(task => task.name !== 'Lead');

  // Deletes a swarm with its project, conversations and stories; its files stay in its folder.
  async function deleteSwarm(item: SwarmItem) {
    const response = await fetch(`/api/swarms/${item.id}/delete`, { method: 'POST', cache: 'no-store',
      headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }, body: '{}' });
    if (!response.ok) {
      const value = await response.json().catch(() => ({}));
      throw new Error(value.error?.message ?? 'The swarm could not be deleted.');
    }
    swarms = swarms.filter(other => other.id !== item.id);
    ondeleted(item.id);
    void chat.refresh();
  }
  // Which swarms show their agents' conversations; a running swarm starts open.
  let openSwarms = $state<Record<string, boolean>>({});
  const swarmOpen = (item: SwarmItem) => openSwarms[item.id] ?? (['running', 'planning', 'idle'].includes(item.status) || item.id === activeSwarm);
  const agentChats = (item: SwarmItem) => item.projectId ? chat.tasks.filter(task => task.projectId === item.projectId) : [];
  let swarms = $state<SwarmItem[]>([]);
  $effect(() => {
    let ticks = 0;
    const read = () => fetch('/api/swarms', { cache: 'no-store' }).then(response => response.ok ? response.json() : null)
      .then(value => {
        if (!Array.isArray(value?.swarms)) return;
        swarms = value.swarms;
        // While a swarm runs, its agents open new conversations; the list catches up every 6 s.
        if (swarms.some(item => item.status === 'running' || item.status === 'planning') && ++ticks % 2 === 0) void chat.refresh();
      }).catch(() => {});
    void read();
    const timer = setInterval(read, 3000);
    return () => clearInterval(timer);
  });
  const swarmDot = (status: string) => status === 'running' || status === 'planning' ? 'bg-brand'
    : status === 'done' || status === 'idle' ? 'bg-success' : status === 'failed' ? 'bg-destructive' : 'bg-[#c7c7cc]';

  // Opens an existing conversation and dismisses mobile navigation.
  function select(task: Task) { void chat.select(task); onselect(); }
  // Starts an empty conversation under the chosen project.
  function create(id: string) { void chat.newChat(id); onselect(); }

  // Groups a project's chats by agent, with the most recently active chats first.
  function projectGroups(projectId: string) {
    const groups = new Map<string, { id: string; name: string; tasks: Task[] }>();
    for (const task of chat.tasks.filter(task => task.projectId === projectId)) {
      const id = task.agentId ?? '';
      const name = id ? chat.agents.find(agent => agent.id === id)?.name ?? task.agentName ?? 'Agent' : 'Main agent';
      if (!groups.has(id)) groups.set(id, { id, name, tasks: [] });
      groups.get(id)!.tasks.push(task);
    }
    for (const group of groups.values()) {
      group.tasks.sort((a, b) => (b.updatedAt ?? b.createdAt ?? 0) - (a.updatedAt ?? a.createdAt ?? 0)
        || a.id.localeCompare(b.id));
    }
    const saved = chat.projects.groupOrder[projectId] ?? [];
    const rank = (id: string) => saved.includes(id) ? saved.indexOf(id) : saved.length;
    return [...groups.values()].sort((a, b) => rank(a.id) - rank(b.id)
      || a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  }

  let drag = $state<{ pointer: number; project: string; id: string; y: number; before: string | null; moved: boolean } | null>(null);

  // Moves one group before another, or to the end, and saves only the finished order.
  function reorder(project: string, id: string, before: string | null) {
    if (id === before) return;
    const ids = projectGroups(project).map(group => group.id).filter(item => item !== id);
    const index = before === null ? ids.length : ids.indexOf(before);
    if (index < 0) return;
    ids.splice(index, 0, id);
    chat.projects.orderGroups(project, ids);
  }

  // Starts a pointer gesture on the handle without toggling the section.
  function startGroup(event: PointerEvent, project: string, id: string) {
    if (event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.focus(); handle.setPointerCapture(event.pointerId);
    drag = { pointer: event.pointerId, project, id, y: event.clientY, before: id, moved: false };
  }

  // Finds the drop position among this project's headings and scrolls near the list edges.
  function moveGroup(event: PointerEvent) {
    if (!drag || drag.pointer !== event.pointerId) return;
    if (!drag.moved && Math.abs(event.clientY - drag.y) < 5) return;
    const handle = event.currentTarget as HTMLElement;
    const project = handle.closest('[data-agent-project]');
    const nav = handle.closest('nav');
    if (!project || !nav) return;
    const bounds = nav.getBoundingClientRect();
    if (event.clientY < bounds.top + 32) nav.scrollTop -= 16;
    if (event.clientY > bounds.bottom - 32) nav.scrollTop += 16;
    const headings = [...project.querySelectorAll<HTMLElement>('[data-agent-heading]')];
    const target = headings.find(heading => {
      const box = heading.getBoundingClientRect();
      return event.clientY < box.top + box.height / 2;
    });
    drag = { ...drag, moved: true, before: target?.dataset.agentHeading ?? null };
  }

  // Commits a drop; cancelled or lost pointer capture leaves the saved order alone.
  function finishGroup(event: PointerEvent) {
    if (!drag || drag.pointer !== event.pointerId) return;
    const finished = drag;
    drag = null;
    if (event.type === 'pointerup' && finished.moved) reorder(finished.project, finished.id, finished.before);
    const handle = event.currentTarget as HTMLElement;
    if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
  }

  // Gives keyboard users the same ordering control with the up and down arrows.
  function groupKey(event: KeyboardEvent, project: string, id: string) {
    if (event.key === 'Escape') { drag = null; return; }
    if (!['ArrowUp', 'ArrowDown'].includes(event.key)) return;
    event.preventDefault();
    const ids = projectGroups(project).map(group => group.id);
    const index = ids.indexOf(id), next = index + (event.key === 'ArrowUp' ? -1 : 1);
    if (next < 0 || next >= ids.length) return;
    reorder(project, id, event.key === 'ArrowUp' ? ids[next] : ids[next + 1] ?? null);
  }
</script>

{#snippet row(task: Task)}
  <div class={`group flex min-w-0 items-center rounded-md ${chat.active?.id === task.id
    ? 'bg-foreground/8 font-medium' : 'hover:bg-foreground/5'}`}>
    <button class="min-w-0 flex-1 px-2 py-2 text-left text-sm disabled:opacity-50"
      aria-current={chat.active?.id === task.id ? 'page' : undefined}
      disabled={chat.working || chat.switching || chat.deleting} onclick={() => select(task)}>
      <ScrollingName text={task.name} />
    </button>
    {#if chat.isWorking(task.id)}
      <span role="status" aria-label={`Working on ${task.name}`} title="Working"
        class="mr-1 flex shrink-0 items-center text-muted-foreground">
        <LoaderCircle size={13} class="motion-safe:animate-spin" aria-hidden="true" />
      </span>
    {:else if chat.hasReadyReply(task.id)}
      <span role="status" aria-label={`Response ready in ${task.name}`} title="Response ready"
        class="mr-2 size-2 shrink-0 rounded-full bg-brand"></span>
    {/if}
    <span class="nav-actions flex shrink-0 items-center">
      <RenameChat {chat} {task} disabled={chat.working || chat.switching || chat.deleting || !chat.ready} />
      <ConfirmDelete label={`Delete chat ${task.name}`} actionLabel="Delete chat"
        description={task.folder
          ? `Permanently delete ${task.name} and its messages? This cannot be undone. Its folder is kept unless you tick the box.`
          : `Permanently delete ${task.name} and its messages? This cannot be undone.`}
        option={task.folder ? { label: `Also move this conversation's folder to ${words.folderTrash}`, detail: task.folder } : undefined}
        disabled={chat.working || chat.switching || chat.deleting || !chat.ready}
        action={ticked => chat.deleteChat(task, ticked)} />
    </span>
  </div>
{/snippet}

<nav class="min-h-0 flex-1 overflow-y-auto px-3 py-5" aria-label="Tasks">
  <!-- In this fork each swarm is a project; the swarms replace the Projects list. -->
  <h2 class="mb-2 pl-2 text-xs font-medium text-muted-foreground">Swarms</h2>
  {#each swarms as item (item.id)}
    <div class={['flex min-w-0 items-center rounded-md', activeSwarm === item.id ? 'bg-sidebar-accent' : 'hover:bg-foreground/5']}>
      <button class="flex size-7 shrink-0 items-center justify-center rounded text-muted-foreground"
        aria-label={`${swarmOpen(item) ? 'Hide' : 'Show'} the agents' conversations of ${item.goal}`} aria-expanded={swarmOpen(item)}
        onclick={() => openSwarms[item.id] = !swarmOpen(item)}>
        <ChevronRight size={13} class={swarmOpen(item) ? 'rotate-90' : ''} aria-hidden="true" />
      </button>
      <button class="flex min-w-0 flex-1 items-center gap-2 py-2 pr-1 text-left text-sm" onclick={() => onswarm(item.id)}
        aria-current={activeSwarm === item.id ? 'true' : undefined}>
        <span class={['size-2 shrink-0 rounded-full', swarmDot(item.status)]} aria-hidden="true"></span>
        <span class="min-w-0 flex-1 truncate">{item.goal}</span>
        <span class="shrink-0 text-xs tabular-nums text-muted-foreground">{item.counts?.done ?? 0}/{item.counts?.total ?? 0}</span>
      </button>
      <span class="nav-actions flex shrink-0 items-center">
        <ConfirmDelete label={`Delete swarm ${item.goal}`} actionLabel="Delete swarm"
          description={`Permanently delete this swarm, its stories and every agent's conversation? This cannot be undone. The files its agents made stay in its folder.`}
          disabled={!chat.ready} action={() => deleteSwarm(item)} />
      </span>
    </div>
    {#if swarmOpen(item)}
      <div class="mb-1 ml-4 border-l border-border pl-1" role="group" aria-label={`Conversations of ${item.goal}`}>
        {#if leadChat(item)}{@render row(leadChat(item)!)}{:else}<p class="px-2 py-1.5 text-xs text-muted-foreground">The Lead starts in a moment</p>{/if}
        {#if otherChats(item).length}
          <button class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-foreground/5"
            aria-expanded={openAgents[item.id] ?? false} onclick={() => openAgents[item.id] = !(openAgents[item.id] ?? false)}>
            <ChevronRight size={12} class={openAgents[item.id] ? 'rotate-90' : ''} aria-hidden="true" />
            Agents <span class="tabular-nums">{otherChats(item).length}</span>
          </button>
          {#if openAgents[item.id]}
            <div class="ml-3" role="group" aria-label={`Agents' conversations of ${item.goal}`}>
              {#each otherChats(item) as task (task.id)}{@render row(task)}{/each}
            </div>
          {/if}
        {/if}
      </div>
    {/if}
  {:else}<p class="px-2 py-2 text-xs text-muted-foreground">No swarms yet</p>{/each}
  <h2 class="mb-2 mt-6 px-2 text-xs font-medium text-muted-foreground">Conversations</h2>
  {#each chat.tasks.filter(t => !t.projectId) as task (task.id)}
    {@render row(task)}
  {:else}<p class="px-2 py-2 text-xs text-muted-foreground">No conversations</p>{/each}
</nav>
