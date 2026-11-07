<script lang="ts">
  // The Agents page: one card per agent with its averages, tokens, health, running time and recent activity, and
  // a chart of each agent's share of the work. Figures come from /api/analytics/agents.
  import { onMount } from 'svelte';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Bot from '@lucide/svelte/icons/bot';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Choice from './Choice.svelte';
  import type { Chat } from './chat.svelte';
  import OctagonX from '@lucide/svelte/icons/octagon-x';
  import Square from '@lucide/svelte/icons/square';
  import Forward from '@lucide/svelte/icons/forward';
  import Reply from '@lucide/svelte/icons/reply';
  import Search from '@lucide/svelte/icons/search';
  // Stops and failures across the range, under the tiles.
  import AgentTimeline from './AgentTimeline.svelte';
  import { ago, duration, eventWords, names, percent, shares, short, stateWords, withKnownAgents, withLive,
    type ActiveTurn, type AgentReport, type AgentState, type AgentStats } from './agent-stats';

  let { chat, onclose }: { chat: Chat; onclose?: () => void } = $props();

  let report = $state<AgentReport>();
  let days = $state('30');
  let loading = $state(false), error = $state('');
  let shareOf = $state<'responses' | 'tasks'>('responses');
  // Ticks every second so a running agent's time counts up between readings.
  let now = $state(Date.now());

  async function load() {
    loading = true;
    try { report = await chat.agentAnalytics<AgentReport>(Number(days)); error = ''; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The figures could not be read.'; }
    finally { loading = false; }
  }

  onMount(() => {
    void load();
    let last = Date.now();
    // Health and running time change while the page is open: every 3 seconds while something runs, so a
    // stopped response leaves quickly, otherwise every 15 seconds.
    const tick = setInterval(() => {
      now = Date.now();
      const every = live.length || stopping.size ? 3000 : 15_000;
      if (!loading && now - last >= every) { last = now; void load(); }
    }, 1000);
    return () => clearInterval(tick);
  });

  const agents = $derived(report ? withKnownAgents(withLive(
    withKnownAgents(report.agents, chat.agents.map(a => ({ id: a.id, name: a.name }))), report.active ?? []), []) : []);
  const live = $derived(report?.active ?? []);

  // Turns a stop was asked for, until the backend stops listing them.
  let stopping = $state(new Set<string>());
  let stopError = $state('');
  $effect(() => {
    const running = new Set(live.map(turn => turn.turnId));
    if ([...stopping].some(id => !running.has(id))) stopping = new Set([...stopping].filter(id => running.has(id)));
  });

  // The kill switch: interrupts each response, as Stop does in its conversation. A delegated task whose
  // response is stopped ends as cancelled.
  async function stop(turns: ActiveTurn[]) {
    const fresh = turns.filter(turn => !turn.stopping && !stopping.has(turn.turnId));
    if (!fresh.length) return;
    stopping = new Set([...stopping, ...fresh.map(turn => turn.turnId)]);
    const results = await Promise.allSettled(fresh.map(turn => chat.interruptTurn(turn.threadId, turn.turnId)));
    const failed = results.filter(result => result.status === 'rejected').length;
    // A response that ended by itself meanwhile also refuses; the next reading tells them apart.
    stopError = failed ? `${failed} of ${fresh.length} could not be stopped. They may have finished already.` : '';
    void load();
  }
  const isStopping = (turn: ActiveTurn) => turn.stopping || stopping.has(turn.turnId);
  const parts = $derived(shares(agents, shareOf));
  const colors = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-5)', 'var(--chart-3)', 'var(--chart-4)'];
  const color = (index: number) => colors[index % colors.length];
  // The ring is one conic gradient: each agent's slice starts where the previous one ended.
  const ring = $derived.by(() => {
    let start = 0;
    const stops = parts.map((part, index) => {
      const end = start + part.share * 100;
      const stop = `${color(index)} ${start}% ${end}%`;
      start = end;
      return stop;
    });
    return stops.length ? `conic-gradient(${stops.join(', ')})` : 'conic-gradient(var(--muted) 0 100%)';
  });
  const running = $derived(agents.filter(agent => agent.state === 'running').length);

  const badge: Record<AgentState, string> = {
    running: 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
    healthy: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
    stopped: 'bg-amber-500/15 text-amber-700 dark:text-amber-300',
    failed: 'bg-destructive/15 text-destructive',
    idle: 'bg-muted text-muted-foreground',
  };
  // The tile's left edge, in the state's colour.
  const edge: Record<AgentState, string> = {
    running: 'border-l-sky-500', healthy: 'border-l-emerald-500', stopped: 'border-l-amber-500',
    failed: 'border-l-destructive', idle: 'border-l-border',
  };

  // How many recent events a tile lists.
  const RECENT = 3;

  // With many agents: show only those that need a look, and find one by name.
  type Filter = 'all' | 'running' | 'delegating' | 'attention' | 'idle';
  const filters: { id: Filter; label: string; test: (agent: AgentStats) => boolean }[] = [
    { id: 'all', label: 'All', test: () => true },
    { id: 'running', label: 'Running', test: agent => agent.state === 'running' },
    { id: 'delegating', label: 'Delegating', test: agent => !!agent.delegatingTo?.length },
    { id: 'attention', label: 'Stopped or failed', test: agent => agent.state === 'stopped' || agent.state === 'failed' },
    { id: 'idle', label: 'Idle', test: agent => agent.state === 'idle' },
  ];
  let filter = $state<Filter>('all');
  let query = $state('');
  const shown = $derived(agents.filter(filters.find(option => option.id === filter)!.test)
    .filter(agent => !query.trim() || agent.name.toLowerCase().includes(query.trim().toLowerCase())));

  const dot: Record<AgentState, string> = {
    running: 'bg-sky-500 animate-pulse', healthy: 'bg-emerald-500', stopped: 'bg-amber-500', failed: 'bg-destructive',
    idle: 'bg-muted-foreground/50',
  };
</script>

<section aria-label="Agents" class="flex min-h-0 flex-1 flex-col">
  <header class="flex min-h-14 shrink-0 items-center gap-3 border-b border-border px-4 sm:min-h-16 sm:px-6">
    <h1 class="text-lg font-semibold">Agents</h1>
    {#if running}<span class="text-sm text-muted-foreground">{running} running now</span>{/if}
    <div class="ml-auto flex items-center gap-1.5">
      {#if live.length}
        <button class="flex items-center gap-1.5 rounded-md bg-destructive px-3 py-1.5 text-sm font-medium text-white
          hover:bg-destructive/90 disabled:opacity-50" disabled={live.every(isStopping)} onclick={() => stop(live)}>
          <OctagonX size={15} aria-hidden="true" />Stop all agents
        </button>
      {/if}
      <Choice label="Range" value={days} placeholder="Last 30 days" disabled={loading}
        options={[{ value: '1', label: 'Today' }, { value: '7', label: 'Last 7 days' }, { value: '30', label: 'Last 30 days' },
          { value: '90', label: 'Last 90 days' }, { value: '365', label: 'Last year' }]}
        onchange={value => { days = value; void load(); }} />
      <button class="icon-button" aria-label="Reload agent figures" disabled={loading} onclick={load}>
        <RefreshCw size={15} aria-hidden="true" class={loading ? 'animate-spin' : ''} />
      </button>
      {#if onclose}
        <button class="flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-sm hover:bg-accent"
          onclick={onclose}><MessageSquare size={15} aria-hidden="true" />Back to chat</button>
      {/if}
    </div>
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if stopError}
      <p role="alert" class="mx-5 mt-4 text-sm text-destructive">{stopError}</p>
    {/if}
    {#if error}
      <p role="alert" class="m-5 text-sm text-destructive">{error}</p>
    {:else if !report}
      <p class="m-5 text-sm text-muted-foreground">Reading what each agent has done...</p>
    {:else}
      <div class="mx-auto flex max-w-6xl flex-col gap-4 p-4 sm:p-6">

        <!-- Share of the work: a ring for the few-parts whole, with a legend that carries the numbers. -->
        <section aria-label="Share of work" class="rounded-lg border border-border p-4">
          <div class="flex flex-wrap items-baseline justify-between gap-2">
            <h2 class="text-sm font-medium">Share of work</h2>
            <div role="radiogroup" aria-label="Count" class="flex gap-1 rounded-md bg-muted p-0.5 text-xs">
              {#each [['responses', 'Responses'], ['tasks', 'Delegated tasks']] as [value, label] (value)}
                <button role="radio" aria-checked={shareOf === value} onclick={() => shareOf = value as typeof shareOf}
                  class={`rounded px-2.5 py-1 ${shareOf === value ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}`}>{label}</button>
              {/each}
            </div>
          </div>
          <p class="mb-3 mt-0.5 text-xs text-muted-foreground">
            {shareOf === 'responses' ? 'Of all responses in the range, the part each agent gave.'
              : 'Of all tasks delegated in the range, the part each agent was handed.'}
          </p>
          {#if parts.length}
            <div class="flex flex-col items-center gap-5 sm:flex-row sm:items-start">
              <div class="relative size-28 shrink-0 rounded-full" style:background={ring} role="img"
                aria-label={parts.map(part => `${part.name} ${percent(part.share)}`).join(', ')}>
                <div class="absolute inset-[20%] flex flex-col items-center justify-center rounded-full bg-background">
                  <span class="text-lg font-semibold tabular-nums">{parts.reduce((sum, part) => sum + part.count, 0)}</span>
                  <span class="text-[11px] text-muted-foreground">{shareOf === 'responses' ? 'responses' : 'tasks'}</span>
                </div>
              </div>
              <!-- Many agents flow into columns instead of one long list. -->
              <ul class="grid w-full flex-1 grid-cols-1 gap-x-6 gap-y-1.5 md:grid-cols-2 xl:grid-cols-3">
                {#each parts as part, index (part.name)}
                  <li class="grid grid-cols-[auto_minmax(0,1fr)_auto_3.5rem] items-center gap-x-2 text-sm">
                    <span class="size-2.5 rounded-sm" style:background={color(index)} aria-hidden="true"></span>
                    <span class="truncate" title={part.name}>{part.name}</span>
                    <span class="text-xs text-muted-foreground tabular-nums">{part.count}</span>
                    <span class="text-right font-medium tabular-nums">{percent(part.share)}</span>
                    <div class="col-span-4 col-start-1 mt-1 h-1 overflow-hidden rounded-full bg-muted">
                      <div class="h-full rounded-full" style:width={`${part.share * 100}%`} style:background={color(index)}></div>
                    </div>
                  </li>
                {/each}
              </ul>
            </div>
          {:else}
            <p class="py-6 text-center text-sm text-muted-foreground">
              {shareOf === 'responses' ? 'No responses in this range yet.' : 'No task was delegated in this range.'}
            </p>
          {/if}
        </section>

        <!-- Finding an agent among many: filters by what needs attention, with counts, and a name search. -->
        <div class="flex flex-wrap items-center gap-2">
          <div role="radiogroup" aria-label="Show" class="flex flex-wrap gap-1 rounded-md bg-muted p-0.5 text-xs">
            {#each filters as option (option.id)}
              <button role="radio" aria-checked={filter === option.id} onclick={() => filter = option.id}
                class={`flex items-center gap-1.5 rounded px-2.5 py-1 ${filter === option.id ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}>
                {option.label}<span class="tabular-nums text-muted-foreground">{agents.filter(option.test).length}</span>
              </button>
            {/each}
          </div>
          <label class="relative ml-auto w-full sm:w-56">
            <span class="sr-only">Find an agent</span>
            <Search size={14} class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <input type="search" placeholder="Find an agent" bind:value={query}
              class="w-full rounded-md border border-border bg-background py-1.5 pl-8 pr-2 text-sm" />
          </label>
        </div>

        <!-- Small tiles, as many across as fit. The coloured left edge is the state, so a column of tiles can be
             scanned for trouble without reading. -->
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
          {#each shown as agent (agent.main ? '' : agent.id ?? agent.name)}
            <article aria-label={agent.name}
              class={`flex min-w-0 flex-col gap-2.5 rounded-lg border border-l-[3px] border-border bg-background p-3 ${edge[agent.state]}`}>
              <div class="flex items-start gap-2">
                <div class="min-w-0 flex-1">
                  <h2 class="truncate text-sm font-semibold" title={agent.name}>{agent.name}{#if agent.main}<span class="ml-1 text-xs font-normal text-muted-foreground">main</span>{/if}</h2>
                  <p class="truncate text-xs text-muted-foreground">
                    {#if agent.state === 'running' && agent.runningSince}
                      <span>Running for <span class="tabular-nums">{duration((now - agent.runningSince) / 1000)}</span></span>
                      {#if agent.tasks.running > 1}· {agent.tasks.running} tasks{/if}
                    {:else}
                      Last active {ago(agent.lastActive, now)}
                    {/if}
                  </p>
                </div>
                <span role="status" class={`flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium ${badge[agent.state]}`}>
                  <span class={`size-1.5 rounded-full ${dot[agent.state]}`} aria-hidden="true"></span>{stateWords[agent.state]}
                </span>
                {#if agent.live?.length}
                  <button class="flex size-6 shrink-0 items-center justify-center rounded-md bg-destructive text-white hover:bg-destructive/90
                    disabled:opacity-50" disabled={agent.live.every(isStopping)} aria-label={`Stop ${agent.name}`}
                    title={`Stop ${agent.name}`} onclick={() => stop(agent.live ?? [])}>
                    <OctagonX size={13} aria-hidden="true" />
                  </button>
                {/if}
              </div>

              <!-- Shown beside the state, whatever it is: an agent can be Stopped and still have work out with others. -->
              {#if agent.delegatingTo?.length || agent.workingFor?.length}
                <div class="flex flex-wrap gap-1">
                  {#if agent.delegatingTo?.length}
                    <span class="flex max-w-full items-center gap-1 rounded-full bg-violet-500/15 px-2 py-0.5 text-[11px] font-medium
                      text-violet-700 dark:text-violet-300">
                      <Forward size={11} aria-hidden="true" />
                      <span class="truncate">Delegating to {names(agent.delegatingTo.map(task => task.agent))}</span>
                    </span>
                  {/if}
                  {#if agent.workingFor?.length}
                    <span class="flex max-w-full items-center gap-1 rounded-full border border-border px-2 py-0.5 text-[11px] text-muted-foreground">
                      <Reply size={11} aria-hidden="true" />
                      <span class="truncate">Working for {names(agent.workingFor)}</span>
                    </span>
                  {/if}
                </div>
              {/if}

              {#if agent.live?.length}
                <!-- The kill switch, one line per running response. -->
                <ul role="group" aria-label={`${agent.name} running now`} class="space-y-0.5 rounded-md bg-destructive/5 px-2 py-1.5 text-xs">
                  {#each agent.live as turn (turn.turnId)}
                    <li class="flex items-center justify-between gap-2">
                      <span class="min-w-0 truncate" title={turn.conversation}>{turn.conversation}</span>
                      <span class="flex shrink-0 items-center gap-1.5 text-muted-foreground">
                        {#if turn.startedAt}<span class="tabular-nums">{duration((now - turn.startedAt) / 1000)}</span>{/if}
                        {#if isStopping(turn)}
                          <span>Stopping...</span>
                        {:else}
                          <button class="flex items-center gap-1 rounded border border-border px-1 py-px hover:bg-accent hover:text-foreground"
                            aria-label={`Stop ${turn.conversation}`} onclick={() => stop([turn])}>
                            <Square size={9} aria-hidden="true" />Stop
                          </button>
                        {/if}
                      </span>
                    </li>
                  {/each}
                </ul>
              {/if}

              {#if agent.delegatingTo?.length}
                <ul class="space-y-0.5 rounded-md bg-violet-500/5 px-2 py-1.5 text-xs" aria-label={`${agent.name} is delegating`}>
                  {#each agent.delegatingTo as task, index (index)}
                    <li class="flex items-baseline justify-between gap-2">
                      <span class="min-w-0 truncate" title={task.summary}>
                        <span class="font-medium">{task.agent || 'MiniClaw'}</span>
                        <span class="text-muted-foreground">· {task.summary || 'a delegated task'}</span>
                      </span>
                      <span class="shrink-0 text-muted-foreground tabular-nums">
                        {task.status === 'asking' ? 'waiting for approval' : `${duration((now - task.started) / 1000)}${task.mode === 'background' ? ', background' : ''}`}
                      </span>
                    </li>
                  {/each}
                </ul>
              {/if}

              <!-- Four figures in one row; the rest of each is in its tooltip. -->
              <dl class="grid grid-cols-4 divide-x divide-border rounded-md bg-muted/50 py-1.5 text-center">
                <div class="min-w-0 px-1" title={`Average finished response. Average delegated task: ${duration(agent.tasks.averageSeconds)}`}>
                  <dt class="truncate text-[11px] text-muted-foreground">Avg reply</dt>
                  <dd class="text-sm font-semibold tabular-nums">{agent.responses.completed ? duration(agent.responses.averageSeconds) : '–'}</dd>
                </div>
                <div class="min-w-0 px-1" title={`${agent.tokens.total.toLocaleString()} tokens: ${agent.tokens.input.toLocaleString()} in, ${agent.tokens.output.toLocaleString()} out, ${short(agent.tokens.perResponse)} per response`}>
                  <dt class="truncate text-[11px] text-muted-foreground">Tokens</dt>
                  <dd class="text-sm font-semibold tabular-nums">{short(agent.tokens.total)}</dd>
                </div>
                <div class="min-w-0 px-1" title={`${agent.responses.count} responses (${agent.responses.interrupted} stopped, ${agent.responses.failed} failed); ${agent.tasks.count} delegated tasks (${agent.tasks.done} done)`}>
                  <dt class="truncate text-[11px] text-muted-foreground">Replies</dt>
                  <dd class="text-sm font-semibold tabular-nums">{agent.responses.count}</dd>
                </div>
                <div class="min-w-0 px-1" title={`Time working: ${duration(agent.responses.busySeconds)}`}>
                  <dt class="truncate text-[11px] text-muted-foreground">Share</dt>
                  <dd class="text-sm font-semibold tabular-nums">{percent(agent.responses.share)}</dd>
                </div>
              </dl>

              {#if agent.recent.length}
                <ul aria-label={`${agent.name} recent activity`} class="space-y-0.5 text-xs">
                  {#each agent.recent.slice(0, RECENT) as event, index (index)}
                    <li class="flex items-baseline justify-between gap-2">
                      <span class="min-w-0 truncate" title={eventWords(event)}>{eventWords(event)}</span>
                      <span class="shrink-0 text-muted-foreground">{ago(event.at, now)}</span>
                    </li>
                  {/each}
                </ul>
              {:else}
                <p class="text-xs text-muted-foreground">Nothing in this range.</p>
              {/if}
            </article>
          {:else}
            <p class="col-span-full py-8 text-center text-sm text-muted-foreground">No agent matches.</p>
          {/each}
        </div>

        <!-- Stops and failures over time, for the agents the filter shows, in the tiles' order. -->
        {#if report.timeline && shown.length}
          <AgentTimeline timeline={report.timeline} order={shown} days={report.days} />
        {/if}

        <p class="text-xs text-muted-foreground">
          A response is one answer from the agent; a delegated task is work another agent handed to it. Healthy means the last
          thing it did finished; Stopped means it was interrupted or cancelled. Averages count only finished work.
        </p>
      </div>
    {/if}
  </div>
</section>
