<script lang="ts">
  // A floating box over the chat that appears while any agent is working, anywhere, with the same kill switch
  // as the Agents page: stop one response, one agent, or every agent. It can be folded to a small pill.
  import { onMount } from 'svelte';
  import OctagonX from '@lucide/svelte/icons/octagon-x';
  import Square from '@lucide/svelte/icons/square';
  import Wifi from '@lucide/svelte/icons/wifi';
  import WifiOff from '@lucide/svelte/icons/wifi-off';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import type { Chat } from './chat.svelte';
  import { duration, MAIN_AGENT, type ActiveTurn, type LiveSwarm, type SwarmMember } from './agent-stats';

  // `docked` places the box in the page's flow, beside the Board, instead of floating over the chat.
  let { chat, onopen, every = 3000, docked = false }: {
    chat: Pick<Chat, 'activeTurns' | 'interruptTurn'> & Partial<Pick<Chat, 'swarmLink'>>;
    // Opens the Agents page.
    onopen?: () => void;
    every?: number;
    docked?: boolean;
  } = $props();

  let active = $state<ActiveTurn[]>([]);
  // Running swarms: their agents are shown under their swarm, with the link switch, not in the list above.
  let swarms = $state<LiveSwarm[]>([]);
  let folded = $state(false);
  let now = $state(Date.now());
  let stopping = $state(new Set<string>());
  let error = $state('');
  let reading = false;

  async function read() {
    if (reading) return;
    reading = true;
    try {
      const reading = await chat.activeTurns<ActiveTurn>();
      active = reading.active;
      swarms = reading.swarms ?? [];
      const running = new Set(active.map(turn => turn.turnId));
      stopping = new Set([...stopping].filter(id => running.has(id)));
      if (!active.length) error = '';
    } catch { /* signed out or the backend restarting: keep the last reading */ }
    finally { reading = false; }
  }

  onMount(() => {
    void read();
    let last = Date.now();
    const tick = setInterval(() => {
      now = Date.now();
      // A hidden tab does not need to ask.
      if (document.visibilityState !== 'hidden' && now - last >= every) { last = now; void read(); }
    }, 1000);
    return () => clearInterval(tick);
  });

  const isStopping = (turn: ActiveTurn) => turn.stopping || stopping.has(turn.turnId);

  async function stop(turns: ActiveTurn[]) {
    const fresh = turns.filter(turn => !isStopping(turn));
    if (!fresh.length) return;
    stopping = new Set([...stopping, ...fresh.map(turn => turn.turnId)]);
    const results = await Promise.allSettled(fresh.map(turn => chat.interruptTurn(turn.threadId, turn.turnId)));
    const failed = results.filter(result => result.status === 'rejected').length;
    error = failed ? `${failed} of ${fresh.length} could not be stopped. They may have finished already.` : '';
    void read();
  }

  // A swarm agent's running response, so its row can offer Stop.
  const swarmThreads = $derived(new Set(swarms.flatMap(swarm => swarm.members.map(member => member.thread).filter(Boolean))));
  const turnOf = (member: SwarmMember) => active.find(turn => turn.threadId === member.thread);
  let linking = $state(new Set<string>());

  // Cuts or restores one swarm agent's link, then reads again.
  async function toggleLink(swarm: LiveSwarm, member: SwarmMember) {
    if (!chat.swarmLink) return;
    const key = `${swarm.id}:${member.name}`;
    linking = new Set([...linking, key]);
    try { await chat.swarmLink(swarm.id, member.name, !member.linkUp); error = ''; }
    catch (failure) { error = (failure as Error).message; }
    finally { linking = new Set([...linking].filter(item => item !== key)); void read(); }
  }
  const swarmDot = (member: SwarmMember) => !member.linkUp ? 'bg-[#b85400]' : member.issue !== null ? 'bg-sky-500 animate-pulse'
    : member.status === 'Done' ? 'bg-success' : 'bg-[#c7c7cc]';
  const working = $derived(swarms.reduce((sum, swarm) => sum + swarm.members.filter(member => member.issue !== null).length, 0));

  // One group per agent, the main agent first, each in the order its responses started.
  const groups = $derived.by(() => {
    const byAgent = new Map<string, ActiveTurn[]>();
    for (const turn of active.filter(turn => !swarmThreads.has(turn.threadId))) byAgent.set(turn.agent, [...(byAgent.get(turn.agent) ?? []), turn]);
    return [...byAgent.entries()].sort(([a], [b]) => (a === '' ? -1 : b === '' ? 1 : a.localeCompare(b)))
      .map(([agent, turns]) => ({ name: agent || MAIN_AGENT, turns }));
  });
</script>

{#if active.length || swarms.length}
  <div class={docked ? 'm-3 w-80 shrink-0 self-start rounded-lg border border-border bg-background'
    : 'pointer-events-auto absolute right-3 top-3 z-20 w-[min(320px,calc(100%-24px))] rounded-lg border border-destructive/40 bg-background/95 shadow-lg backdrop-blur'}
    role="region" aria-label="Running agents">
    <div class="flex items-center gap-2 px-3 py-2">
      <span class="size-2 shrink-0 animate-pulse rounded-full bg-sky-500" aria-hidden="true"></span>
      <button class="min-w-0 flex-1 truncate text-left text-sm font-medium hover:underline disabled:no-underline"
        disabled={!onopen} onclick={() => onopen?.()} title="Open the Agents page">
        {groups.length + working} {groups.length + working === 1 ? 'agent' : 'agents'} running
      </button>
      <button class="flex shrink-0 items-center gap-1 rounded-md bg-destructive px-2 py-1 text-xs font-medium text-white
        hover:bg-destructive/90 disabled:opacity-50" disabled={active.every(isStopping)} onclick={() => stop(active)}>
        <OctagonX size={13} aria-hidden="true" />Stop all agents
      </button>
      <button class="icon-button size-7" aria-label={folded ? 'Show running agents' : 'Fold running agents'}
        aria-expanded={!folded} onclick={() => folded = !folded}>
        {#if folded}<ChevronDown size={15} aria-hidden="true" />{:else}<ChevronUp size={15} aria-hidden="true" />{/if}
      </button>
    </div>

    {#if !folded}
      <div class="max-h-[50dvh] overflow-y-auto border-t border-border px-3 py-2">
        {#if error}<p role="alert" class="mb-2 text-xs text-destructive">{error}</p>{/if}
        {#each swarms as swarm (swarm.id)}
          <div class="border-b border-border pb-2 pt-1.5" role="group" aria-label={`Swarm: ${swarm.goal}`}>
            <div class="mb-1 flex items-baseline justify-between gap-2">
              <span class="min-w-0 truncate text-xs font-semibold" title={swarm.goal}>{swarm.goal}</span>
              <span class="shrink-0 text-xs tabular-nums text-muted-foreground">{swarm.counts.done}/{swarm.counts.total} done</span>
            </div>
            <ul class="space-y-1 text-sm">
              {#each swarm.members as member (member.name)}
                {@const turn = turnOf(member)}
                <li class="flex items-center gap-2">
                  <span class={['size-2 shrink-0 rounded-full', swarmDot(member)]} aria-hidden="true"></span>
                  <span class="w-16 shrink-0 truncate text-xs font-medium">{member.name}</span>
                  <span class={['min-w-0 flex-1 truncate text-xs', member.linkUp ? 'text-muted-foreground' : 'text-[#b85400]']} title={member.status}>{member.status}</span>
                  {#if turn && !isStopping(turn)}
                    <button class="icon-button size-6" aria-label={`Stop ${member.name}`} title="Stop" onclick={() => stop([turn])}>
                      <Square size={11} aria-hidden="true" />
                    </button>
                  {/if}
                  {#if member.role !== 'lead' && chat.swarmLink}
                    <button class={['flex shrink-0 items-center gap-1 rounded border px-1.5 py-0.5 text-xs disabled:opacity-50',
                      member.linkUp ? 'border-border text-muted-foreground hover:bg-accent hover:text-foreground' : 'border-[#b85400]/50 text-[#b85400] hover:bg-[#b85400]/10']}
                      disabled={linking.has(`${swarm.id}:${member.name}`)} aria-label={member.linkUp ? `Cut ${member.name}'s link` : `Restore ${member.name}'s link`}
                      onclick={() => toggleLink(swarm, member)}>
                      {#if member.linkUp}<WifiOff size={11} aria-hidden="true" />Cut{:else}<Wifi size={11} aria-hidden="true" />Restore{/if}
                    </button>
                  {/if}
                </li>
              {/each}
            </ul>
            {#if swarm.events.length}
              {@const last = swarm.events[swarm.events.length - 1]}
              <p class={['mt-1.5 text-xs', last.kind === 'warn' || last.kind === 'fail' ? 'text-[#b85400]' : 'text-muted-foreground']} title={last.text}>
                <span class="line-clamp-2">{last.text}</span>
              </p>
            {/if}
          </div>
        {/each}
        {#each groups as group (group.name)}
          <div class="py-1.5" role="group" aria-label={`${group.name} running now`}>
            <div class="mb-1 flex items-center justify-between gap-2">
              <span class="truncate text-xs font-semibold">{group.name}</span>
              <button class="flex shrink-0 items-center gap-1 rounded border border-destructive/50 px-1.5 py-0.5 text-xs
                text-destructive hover:bg-destructive/10 disabled:opacity-50" disabled={group.turns.every(isStopping)}
                aria-label={`Stop ${group.name}`} onclick={() => stop(group.turns)}>
                <OctagonX size={11} aria-hidden="true" />Stop agent
              </button>
            </div>
            <ul class="space-y-1 text-sm">
              {#each group.turns as turn (turn.turnId)}
                <li class="flex items-center justify-between gap-2">
                  <span class="min-w-0 truncate text-muted-foreground" title={turn.conversation}>{turn.conversation}</span>
                  <span class="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
                    {#if turn.startedAt}<span class="tabular-nums">{duration((now - turn.startedAt) / 1000)}</span>{/if}
                    {#if isStopping(turn)}
                      <span>Stopping...</span>
                    {:else}
                      <button class="flex items-center gap-1 rounded border border-border px-1.5 py-0.5 hover:bg-accent hover:text-foreground"
                        aria-label={`Stop ${turn.conversation}`} onclick={() => stop([turn])}>
                        <Square size={11} aria-hidden="true" />Stop
                      </button>
                    {/if}
                  </span>
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}
