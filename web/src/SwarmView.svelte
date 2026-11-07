<script lang="ts">
  // Swarm mission control: one shared goal, a team of development and testing agents, and a live
  // view of the shared ledger they work from. The backend runs the swarm (server/src/api/swarm.rs);
  // this page starts it, polls its snapshot, cuts and restores agents' links and adds tasks.
  import type { Chat } from './chat.svelte';
  // MiniClaw's own compact dropdown, as in the composer.
  import Choice from './Choice.svelte';
  // Icons in MiniClaw's set.
  import Users from '@lucide/svelte/icons/users';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import Square from '@lucide/svelte/icons/square';
  import Wifi from '@lucide/svelte/icons/wifi';
  import WifiOff from '@lucide/svelte/icons/wifi-off';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import ShieldOff from '@lucide/svelte/icons/shield-off';

  type Member = { name: string; role: 'lead' | 'dev' | 'review' | 'test'; model: string; linkUp: boolean; reachable: boolean;
    status: string; task: number | null; queued: number; done: number; uptime: number };
  type Task = { id: number; title: string; kind: string; state: string; owner: string | null; result: string | null };
  type Event = { at: number; kind: string; text: string };
  type Snapshot = { id: string; goal: string; workspace: string; unsandboxed: boolean; status: string; created: number; finished: number | null;
    members: Member[]; tasks: Task[]; events: Event[]; counts: { done: number; total: number } };

  // `target` is the swarm the sidebar chose; none means a new one.
  let { chat, target = null }: { chat: Chat; target?: string | null } = $props();

  const EXAMPLES = [
    'Build a URL shortener in Python with a small HTTP API, a web page and tests',
    'Write a command-line todo app in Python that saves to a JSON file, with unit tests',
    'Create a Markdown-to-HTML converter for headings, lists and links, with tests',
    'Write a rate limiter module with a token bucket and a sliding window, with tests',
  ];

  let text = $state('');
  let devCount = $state('3');
  let testCount = $state('2');
  let reviewCount = $state('1');
  let reviewModel = $state('');
  let devModel = $state('');
  let testModel = $state('');
  // Off by default: when on, agents' commands run as the user, outside the sandbox, until the swarm ends.
  let unsandboxed = $state(false);
  let swarm = $state<Snapshot | null>(remembered.snapshot);
  let error = $state('');
  let busy = $state(false);
  let scroller = $state<HTMLElement>();

  const modelOptions = $derived(chat.models.map(model => ({ value: model.model, label: model.displayName })));
  // Defaults: Codex's default for building, a Claude model for testing when one is signed in.
  $effect(() => {
    if (!devModel && modelOptions.length) devModel = (chat.models.find(model => model.isDefault) ?? chat.models[0]).model;
    if (!testModel && modelOptions.length) testModel = (chat.models.find(model => model.provider === 'claude') ?? chat.models[0]).model;
    if (!reviewModel && modelOptions.length) reviewModel = (chat.models.find(model => model.provider === 'claude') ?? chat.models[0]).model;
  });
  const label = (id: string) => chat.models.find(model => model.model === id)?.displayName ?? id;

  // Sends one request the way the rest of the page does: JSON and MiniClaw's request header.
  async function api<T>(path: string, body?: unknown): Promise<T> {
    const response = await fetch(path, { method: body === undefined ? 'GET' : 'POST', cache: 'no-store',
      headers: body === undefined ? {} : { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
      body: body === undefined ? undefined : JSON.stringify(body) });
    const value = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(value.error?.message ?? 'The request failed.');
    return value as T;
  }

  const final = (status?: string) => status === 'done' || status === 'failed' || status === 'stopped' || status === 'interrupted';

  // Opens the swarm the sidebar chose.
  $effect(() => {
    const id = target;
    if (!id || id === swarm?.id) return;
    void api<Snapshot>(`/api/swarms/${id}`).then(value => { swarm = remembered.snapshot = value; error = ''; })
      .catch(failure => { error = (failure as Error).message; });
  });

  // With nothing open, shows the newest swarm that is still running (after a reload, or one started
  // through the API).
  let autoload = true;
  $effect(() => {
    if (swarm || !autoload) return;
    autoload = false;
    void api<{ swarms: { id: string; status: string }[] }>('/api/swarms').then(async ({ swarms }) => {
      const live = swarms.find(item => !final(item.status));
      if (live && !swarm) swarm = remembered.snapshot = await api<Snapshot>(`/api/swarms/${live.id}`);
    }).catch(() => {});
  });

  // Polls the running swarm once a second, and once more after it ends.
  $effect(() => {
    const id = swarm?.id;
    if (!id || final(swarm?.status)) return;
    const timer = setInterval(async () => {
      try { swarm = remembered.snapshot = await api<Snapshot>(`/api/swarms/${id}`); } catch { /* the next poll retries */ }
    }, 1000);
    return () => clearInterval(timer);
  });

  // Keeps the newest line in view as the run grows.
  $effect(() => {
    void swarm?.events.length;
    if (scroller) queueMicrotask(() => scroller && (scroller.scrollTop = scroller.scrollHeight));
  });

  async function submit() {
    const words = text.trim();
    if (!words || busy) return;
    busy = true; error = '';
    try {
      if (!swarm || final(swarm.status)) {
        swarm = remembered.snapshot = await api<Snapshot>('/api/swarms', { goal: words, leadModel: devModel,
          dev: { count: Number(devCount), model: devModel }, test: { count: Number(testCount), model: testModel },
          review: { count: Number(reviewCount), model: reviewModel }, unsandboxed });
      } else {
        swarm = remembered.snapshot = await api<Snapshot>(`/api/swarms/${swarm.id}/tasks`, { text: words });
      }
      text = '';
    } catch (failure) { error = (failure as Error).message; }
    finally { busy = false; }
  }

  async function toggleLink(member: Member) {
    if (!swarm) return;
    try { swarm = remembered.snapshot = await api<Snapshot>(`/api/swarms/${swarm.id}/link`, { agent: member.name, up: !member.linkUp }); }
    catch (failure) { error = (failure as Error).message; }
  }

  async function stop() {
    if (!swarm) return;
    try { swarm = remembered.snapshot = await api<Snapshot>(`/api/swarms/${swarm.id}/stop`, {}); }
    catch (failure) { error = (failure as Error).message; }
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void submit(); }
  }

  // Exported so the sidebar's "New swarm" can clear the view.
  export function reset() { swarm = remembered.snapshot = null; text = ''; error = ''; }

  const running = $derived(!!swarm && !final(swarm.status));
  const workers = $derived(swarm?.members.filter(member => member.role !== 'lead') ?? []);
  const offline = $derived(workers.filter(member => !member.linkUp).length);
  const elapsed = $derived.by(() => {
    if (!swarm) return '';
    void swarm.events.length;
    const seconds = Math.max(0, Math.round(((swarm.finished ?? Date.now()) - swarm.created) / 1000));
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  });
  const statusLine = $derived(!swarm ? '' : swarm.status === 'planning' ? `Planning · ${elapsed}`
    : swarm.status === 'running' ? `Running${offline ? ` · ${offline} agent${offline === 1 ? '' : 's'} offline` : ''} · ${elapsed}`
    : swarm.status === 'done' ? `Finished · ${elapsed}` : swarm.status === 'stopped' ? `Stopped · ${elapsed}`
    : swarm.status === 'interrupted' ? 'Interrupted: the backend restarted' : `Finished with failures · ${elapsed}`);
  const notable = $derived(swarm?.events.filter(event => event.kind !== 'info' || event.text.startsWith('You added')) ?? []);
  const progress = $derived(swarm && swarm.counts.total ? Math.round(100 * swarm.counts.done / swarm.counts.total) : 0);
  const dot = (member: Member) => !member.linkUp ? 'bg-[#b85400]' : member.status === 'Done' ? 'bg-success'
    : member.task !== null || member.role === 'lead' && swarm?.status === 'planning' ? 'bg-brand' : 'bg-[#c7c7cc]';
  const noticeColour = (kind: string) => kind === 'warn' || kind === 'fail' ? 'text-[#b85400]' : kind === 'ok' || kind === 'done' ? 'text-success' : 'text-muted-foreground';
</script>

<script lang="ts" module>
  // The open swarm survives switching to a chat and back.
  export const remembered: { snapshot: any } = { snapshot: null };
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <header class="flex min-h-14 items-center justify-between gap-2 border-b border-border px-2 py-1.5 sm:min-h-16 sm:px-6 sm:py-2">
    <span class="flex h-9 items-center gap-2 px-2 text-xs text-muted-foreground"><Users size={14} aria-hidden="true" />Swarm</span>
    <span role="status" class="text-xs text-muted-foreground">{swarm ? statusLine : `${Number(devCount) + Number(reviewCount) + Number(testCount)} agents ready`}</span>
  </header>

  <div bind:this={scroller} class="conversation-scroll min-h-0 flex-1 overflow-y-auto py-8">
    {#if !swarm}
      <div class="flex min-h-full items-center justify-center">
        <div class="max-w-md text-center">
          <Users size={36} stroke-width={1.5} class="mx-auto mb-5 text-brand" aria-hidden="true" />
          <h1 class="text-2xl font-medium">Give the swarm one goal</h1>
          <p class="mt-3 text-sm text-muted-foreground">Agents split it into tasks, claim them from a shared ledger and keep working if their link drops.</p>
          <div class="mt-6 flex flex-col gap-2">
            {#each EXAMPLES as example (example)}
              <button class="w-full rounded-xl border border-border px-3.5 py-2.5 text-left text-sm hover:bg-muted" onclick={() => text = example}>{example}</button>
            {/each}
          </div>
        </div>
      </div>
    {:else}
      <div class="mx-auto max-w-2xl space-y-8">
        <article aria-label="Your message" class="bubble-user whitespace-pre-wrap break-words text-base leading-7">{swarm.goal}</article>

        <div class="space-y-3">
          <article aria-label="Assistant message" class="text-base leading-7">
            {#if swarm.status === 'planning'}The lead is splitting the goal into tasks…
            {:else}I split the goal into tasks on the shared ledger. Each agent claims its own task, so no one waits on another agent's link.{/if}
          </article>

          <div role="group" aria-label="Swarm" class="rounded-lg border border-border text-sm">
            <div class="flex flex-wrap items-center gap-2 px-3 py-2">
              <Users size={15} class="shrink-0 text-brand" aria-hidden="true" />
              <span class="font-medium">Swarm of {swarm.members.length} agents</span>
              <span class="text-xs text-muted-foreground">{statusLine}</span>
              {#if swarm.unsandboxed}<span class="flex items-center gap-1 rounded-full bg-destructive/12 px-2 py-0.5 text-xs text-destructive"><ShieldOff size={12} aria-hidden="true" />Unsandboxed shell</span>{/if}
            </div>
            <div class="h-[3px] bg-secondary" aria-hidden="true"><div class="h-full bg-brand transition-[width] duration-500" style:width={`${progress}%`}></div></div>
            {#each swarm.members as member (member.name)}
              <div class="flex items-center gap-3 border-t border-border px-3 py-2.5">
                <span class={['size-2 shrink-0 rounded-full', dot(member)]} aria-hidden="true"></span>
                <div class="flex w-36 shrink-0 flex-col">
                  <span class="font-medium">{member.name}</span>
                  <span class="truncate text-xs text-muted-foreground">{label(member.model)}</span>
                </div>
                <span class={['min-w-0 flex-1 text-[13px]', !member.linkUp ? 'text-[#b85400]' : 'text-foreground']}>{member.status}</span>
                {#if member.role !== 'lead' && running}
                  <button class="flex h-8 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                    aria-label={member.linkUp ? `Cut ${member.name}'s link` : `Restore ${member.name}'s link`} onclick={() => toggleLink(member)}>
                    {#if member.linkUp}<WifiOff size={13} aria-hidden="true" />Cut link{:else}<Wifi size={13} aria-hidden="true" />Restore{/if}
                  </button>
                {/if}
              </div>
            {/each}
            <details class="border-t border-border">
              <summary class="flex cursor-pointer items-center justify-between gap-2 px-3 py-2 text-xs text-muted-foreground">
                <span>{swarm.counts.done} of {swarm.counts.total} tasks done · ledger</span>
                <span class="flex min-w-0 items-center gap-1 truncate"><FolderOpen size={12} aria-hidden="true" />{swarm.workspace}</span>
              </summary>
              {#each swarm.tasks as task (task.id)}
                <div class="flex items-center gap-2 border-t border-border px-3 py-2 text-xs">
                  <span class="w-7 shrink-0 tabular-nums text-muted-foreground">#{task.id}</span>
                  <span class="min-w-0 flex-1 truncate" title={task.result ?? task.title}>{task.title}</span>
                  <span class="shrink-0 text-muted-foreground">{task.kind}{task.owner ? ` · ${task.owner}` : ''}</span>
                  <span class={['shrink-0 rounded-full px-2 py-0.5', task.state === 'done' ? 'bg-success/12 text-success'
                    : task.state === 'claimed' ? 'bg-brand/12 text-brand' : task.state === 'failed' ? 'bg-destructive/12 text-destructive' : 'bg-secondary text-muted-foreground']}>{task.state}</span>
                </div>
              {/each}
            </details>
          </div>
        </div>

        {#each notable as event, index (index)}
          {#if event.kind === 'done'}
            <article aria-label="Assistant message" class="text-base leading-7">{event.text}</article>
          {:else}
            <p role="note" class={['flex items-center justify-center gap-1.5 text-center text-xs', noticeColour(event.kind)]}>
              {#if event.kind === 'warn' || event.kind === 'fail'}<WifiOff size={12} class="shrink-0" aria-hidden="true" />{:else}<Wifi size={12} class="shrink-0" aria-hidden="true" />{/if}{event.text}
            </p>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <div class="px-4 pb-4 sm:px-[44px]">
    <div class="mx-auto max-w-2xl">
      {#if error}<p role="alert" class="mb-2 text-xs text-destructive">{error}</p>{/if}
      <div class="composer" role="group" aria-label="Message composer">
        <textarea aria-label={running ? 'Message the swarm' : 'Shared goal'} rows="1" bind:value={text} onkeydown={keydown}
          placeholder={running ? 'Add a task for the swarm' : 'Describe the shared goal'}
          class="composer-input placeholder:text-muted-foreground focus:outline-none"></textarea>
        <div class="mt-2 flex items-center gap-2">
          <div class="flex min-w-0 flex-wrap items-center gap-0.5">
            {#if !running}
              <Choice label="Development agents" value={devCount} placeholder="Dev agents" onchange={value => devCount = value}
                options={[1, 2, 3, 4, 5, 6].map(n => ({ value: String(n), label: `${n} dev` }))} />
              <Choice label="Model for development agents" value={devModel} placeholder={chat.modelsLoading ? 'Loading models...' : 'Model'}
                options={modelOptions} disabled={!modelOptions.length} onchange={value => devModel = value} />
              <span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>
              <Choice label="Code review agents" value={reviewCount} placeholder="Review agents" onchange={value => reviewCount = value}
                options={[0, 1, 2, 3, 4].map(n => ({ value: String(n), label: `${n} review` }))} />
              <Choice label="Model for code review agents" value={reviewModel} placeholder={chat.modelsLoading ? 'Loading models...' : 'Model'}
                options={modelOptions} disabled={!modelOptions.length || reviewCount === '0'} onchange={value => reviewModel = value} />
              <span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>
              <Choice label="Testing agents" value={testCount} placeholder="Test agents" onchange={value => testCount = value}
                options={[0, 1, 2, 3, 4].map(n => ({ value: String(n), label: `${n} test` }))} />
              <Choice label="Model for testing agents" value={testModel} placeholder={chat.modelsLoading ? 'Loading models...' : 'Model'}
                options={modelOptions} disabled={!modelOptions.length || testCount === '0'} onchange={value => testModel = value} />
              <span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>
              <button class={['flex h-9 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs transition-colors',
                unsandboxed ? 'bg-destructive/12 text-destructive' : 'text-muted-foreground hover:bg-border']}
                aria-pressed={unsandboxed} onclick={() => unsandboxed = !unsandboxed}>
                <ShieldOff size={13} aria-hidden="true" />Unsandboxed shell
              </button>
            {:else}
              <span class="px-2 text-xs text-muted-foreground">New tasks go on the ledger; the next free agent claims them.</span>
            {/if}
          </div>
          {#if running && !text.trim()}
            <button class="send-button ml-auto shrink-0" aria-label="Stop the swarm" onclick={stop}><Square size={16} aria-hidden="true" /></button>
          {:else}
            <button class="send-button ml-auto shrink-0" aria-label={running ? 'Add task' : 'Launch the swarm'}
              disabled={busy || !text.trim() || (!running && (!devModel || (testCount !== '0' && !testModel)))} onclick={submit}>
              <ArrowUp size={19} aria-hidden="true" />
            </button>
          {/if}
        </div>
      </div>
      {#if (swarm && running && swarm.unsandboxed) || (!running && unsandboxed)}
        <p role="alert" class="mt-3 text-xs text-destructive">Unsandboxed shell: agents' commands run as you, outside MiniClaw's sandbox, with your files and network, until the swarm ends.</p>
      {:else}
        <p class="mt-3 text-xs text-muted-foreground">
          {#if swarm && running}Agents work in {swarm.workspace} with MiniClaw's sandbox and no network.
          {:else}{Number(devCount) + Number(reviewCount) + Number(testCount) + 1} agents with the lead · each role has Anthropic skills · heartbeat every 5 s · lease 40 s{/if}
        </p>
      {/if}
    </div>
  </div>
</div>
