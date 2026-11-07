<script lang="ts" module>
  // The open swarm survives switching to a chat and back.
  export const remembered: { snapshot: any } = { snapshot: null };
</script>

<script lang="ts">
  // The Swarm page. With no swarm chosen it starts one: a shared goal, the teams and their models.
  // A chosen swarm is a project whose work is its issues, so the page shows Adam's Board and Issues
  // for it; the agents, their links and what happened last are in the running box over the page.
  // The backend runs the swarm (server/src/api/swarm.rs).
  import type { Chat } from './chat.svelte';
  // MiniClaw's own compact dropdown, as in the composer.
  import Choice from './Choice.svelte';
  // The project's issues, by stage and by owner, updated live.
  import BoardPanel from './BoardPanel.svelte';
  import IssuesPanel from './IssuesPanel.svelte';
  import { watchIssues, type ReviewLevel } from './project-state.svelte';
  // Icons in MiniClaw's set.
  import Users from '@lucide/svelte/icons/users';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import Square from '@lucide/svelte/icons/square';
  import ShieldOff from '@lucide/svelte/icons/shield-off';
  import SquareKanban from '@lucide/svelte/icons/square-kanban';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import MessageSquare from '@lucide/svelte/icons/message-square';

  type Snapshot = { id: string; goal: string; workspace: string; unsandboxed: boolean; projectId: string | null; leadThread?: string | null; status: string;
    created: number; finished: number | null; counts: { done: number; total: number } };

  // `target` is the swarm the sidebar chose; none means a new one. `onshow` tells the sidebar which
  // swarm is on screen; `onagents` opens the Agents page.
  // `onlead` opens the swarm's Lead conversation, where the user talks to the Lead.
  let { chat, target = null, onshow = () => {}, onagents, onlead }: { chat: Chat; target?: string | null;
    onshow?: (id: string | null) => void; onagents?: () => void; onlead?: (thread: string) => void } = $props();

  const EXAMPLES = [
    'Build a URL shortener in Python with a small HTTP API, a web page and tests',
    'Write a command-line todo app in Python that saves to a JSON file, with unit tests',
    'Create a Markdown-to-HTML converter for headings, lists and links, with tests',
    'Write a rate limiter module with a token bucket and a sliding window, with tests',
  ];
  const LEVELS: { value: ReviewLevel; label: string }[] = [
    { value: 'none', label: 'You review every issue' },
    { value: 'low', label: 'Agents close low severity' },
    { value: 'medium', label: 'Agents close up to medium' },
    { value: 'high', label: 'Agents close everything' },
  ];

  let text = $state('');
  let devCount = $state('3');
  let reviewCount = $state('1');
  let testCount = $state('2');
  let devModel = $state('');
  let reviewModel = $state('');
  let testModel = $state('');
  // Off by default: when on, agents' commands run as the user, outside the sandbox, until the swarm ends.
  let unsandboxed = $state(false);
  let level = $state<ReviewLevel>('medium');
  let swarm = $state<Snapshot | null>(remembered.snapshot);
  let error = $state('');
  let busy = $state(false);
  let tab = $state<'board' | 'issues'>('board');
  // Bumped when the project's issues change, so the Board and Issues reload at once.
  let live = $state(0);
  let openIssue = $state<number | null>(null);
  let openSeq = $state(0);

  const modelOptions = $derived(chat.models.map(model => ({ value: model.model, label: model.displayName })));
  // Defaults: Codex's default model builds; a Claude model, when one is signed in, reviews and tests.
  $effect(() => {
    if (!modelOptions.length) return;
    const claude = (chat.models.find(model => model.provider === 'claude') ?? chat.models[0]).model;
    if (!devModel) devModel = (chat.models.find(model => model.isDefault) ?? chat.models[0]).model;
    if (!reviewModel) reviewModel = claude;
    if (!testModel) testModel = claude;
  });

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
  // Right after a launch, the Lead's conversation opens once it exists, so the user sees it plan.
  let openLead = false;
  $effect(() => {
    const thread = swarm?.leadThread;
    if (openLead && thread) { openLead = false; void chat.refresh().then(() => onlead?.(thread)); }
  });
  const show = (value: Snapshot | null) => { swarm = remembered.snapshot = value; };

  // Tells the sidebar which swarm is on screen, so it stays highlighted and open there.
  $effect(() => { onshow(swarm?.id ?? null); });

  // Opens the swarm the sidebar chose.
  $effect(() => {
    const id = target;
    if (!id || id === swarm?.id) return;
    void api<Snapshot>(`/api/swarms/${id}`).then(value => { show(value); error = ''; }).catch(failure => { error = (failure as Error).message; });
  });

  // With nothing open, shows the newest swarm still running (after a reload, or one started through the API).
  let autoload = true;
  $effect(() => {
    if (swarm || !autoload) return;
    autoload = false;
    void api<{ swarms: { id: string; status: string }[] }>('/api/swarms').then(async ({ swarms }) => {
      const running = swarms.find(item => !final(item.status));
      if (running && !swarm) show(await api<Snapshot>(`/api/swarms/${running.id}`));
    }).catch(() => {});
  });

  // A running swarm's status and counts, every two seconds and once more after it ends.
  $effect(() => {
    const id = swarm?.id;
    if (!id || final(swarm?.status)) return;
    const timer = setInterval(async () => {
      try { show(await api<Snapshot>(`/api/swarms/${id}`)); } catch { /* the next reading retries */ }
    }, 2000);
    return () => clearInterval(timer);
  });

  // The project's issues, live: the Board and Issues reload as agents move them.
  $effect(() => {
    const project = swarm?.projectId;
    if (!project) return;
    return watchIssues(project, () => { live++; });
  });

  // The project's review level, read when a swarm opens; changing it applies to the next finished issue.
  $effect(() => {
    const project = swarm?.projectId;
    if (!project) return;
    void chat.projects.reviewLevel(project).then(value => { level = value; }).catch(() => {});
  });

  async function saveLevel(value: string) {
    const previous = level;
    level = value as ReviewLevel;
    if (!swarm?.projectId) return;
    try { await chat.projects.saveReviewLevel(swarm.projectId, level); error = ''; }
    catch (failure) { level = previous; error = (failure as Error).message; }
  }

  async function launch() {
    const goal = text.trim();
    if (!goal || busy) return;
    busy = true; error = '';
    try {
      show(await api<Snapshot>('/api/swarms', { goal, leadModel: devModel, dev: { count: Number(devCount), model: devModel },
        review: { count: Number(reviewCount), model: reviewModel }, test: { count: Number(testCount), model: testModel },
        unsandboxed, reviewLevel: level }));
      text = ''; tab = 'board'; openLead = true;
      // The new swarm's project appears in the sidebar's list and the agents' conversations under it.
      void chat.projects.refresh();
    } catch (failure) { error = (failure as Error).message; }
    finally { busy = false; }
  }

  async function file() {
    const words = text.trim();
    if (!words || busy || !swarm) return;
    busy = true; error = '';
    try { show(await api<Snapshot>(`/api/swarms/${swarm.id}/tasks`, { text: words })); text = ''; }
    catch (failure) { error = (failure as Error).message; }
    finally { busy = false; }
  }

  async function stop() {
    if (!swarm) return;
    try { show(await api<Snapshot>(`/api/swarms/${swarm.id}/stop`, {})); } catch (failure) { error = (failure as Error).message; }
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void (swarm ? file() : launch()); }
  }

  // Called by the sidebar's "New swarm".
  export function reset() { show(null); text = ''; error = ''; }

  const running = $derived(!!swarm && !final(swarm.status));
  const statusWord = $derived(!swarm ? '' : ({ planning: 'Planning', running: 'Running', done: 'Finished', stopped: 'Stopped',
    failed: 'Finished with failures', interrupted: 'Interrupted: the backend restarted', idle: 'Waiting for new stories' } as Record<string, string>)[swarm.status] ?? swarm.status);
  const total = $derived(Number(devCount) + Number(reviewCount) + Number(testCount) + 1);
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <header class="flex min-h-14 items-center justify-between gap-3 border-b border-border px-2 py-1.5 sm:min-h-16 sm:px-6 sm:py-2">
    {#if swarm}
      <div class="flex min-w-0 items-center gap-2 px-2">
        <Users size={16} class="shrink-0 text-brand" aria-hidden="true" />
        <h1 class="min-w-0 truncate text-sm font-medium" title={swarm.goal}>{swarm.goal}</h1>
        {#if swarm.unsandboxed}<span class="flex shrink-0 items-center gap-1 rounded-full bg-destructive/12 px-2 py-0.5 text-xs text-destructive"><ShieldOff size={12} aria-hidden="true" />Unsandboxed</span>{/if}
      </div>
      <div class="flex shrink-0 items-center gap-3">
        <span role="status" class="text-xs text-muted-foreground">{statusWord} · {swarm.counts.done} of {swarm.counts.total} issues done</span>
        {#if swarm.leadThread && onlead}
          <button class="flex h-8 items-center gap-1.5 rounded-md bg-brand px-2.5 text-xs font-medium text-brand-foreground hover:brightness-110"
            onclick={() => swarm?.leadThread && onlead?.(swarm.leadThread)}>
            <MessageSquare size={12} aria-hidden="true" />Chat with the Lead
          </button>
        {/if}
        {#if running}
          <button class="flex h-8 items-center gap-1.5 rounded-md border border-border px-2.5 text-xs hover:bg-accent" onclick={stop}>
            <Square size={12} aria-hidden="true" />Stop swarm
          </button>
        {/if}
      </div>
    {:else}
      <span class="flex h-9 items-center gap-2 px-2 text-xs text-muted-foreground"><Users size={14} aria-hidden="true" />Swarm</span>
      <span role="status" class="text-xs text-muted-foreground">{total} agents with the lead</span>
    {/if}
  </header>

  <div class="relative flex min-h-0 flex-1 flex-col">
    {#if !swarm}
      <div class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-6">
        <div class="max-w-md text-center">
          <Users size={36} stroke-width={1.5} class="mx-auto mb-5 text-brand" aria-hidden="true" />
          <h1 class="text-2xl font-medium">Give the swarm one goal</h1>
          <p class="mt-3 text-sm text-muted-foreground">The lead files it as issues. Agents take the issues their role works on, and keep going if their link drops.</p>
          <div class="mt-6 flex flex-col gap-2">
            {#each EXAMPLES as example (example)}
              <button class="w-full rounded-xl border border-border px-3.5 py-2.5 text-left text-sm hover:bg-muted" onclick={() => text = example}>{example}</button>
            {/each}
          </div>
        </div>
      </div>
    {:else}
      <div class="flex flex-wrap items-center gap-1 border-b border-border px-4 py-1.5 sm:px-6" role="tablist" aria-label="Swarm views">
        <button role="tab" aria-selected={tab === 'board'} class="workspace-tab" onclick={() => tab = 'board'}><SquareKanban size={14} aria-hidden="true" />Board</button>
        <button role="tab" aria-selected={tab === 'issues'} class="workspace-tab" onclick={() => tab = 'issues'}><CircleDot size={14} aria-hidden="true" />Issues</button>
        <span class="ml-auto flex min-w-0 items-center gap-1 truncate text-xs text-muted-foreground" title={swarm.workspace}>
          <FolderOpen size={12} class="shrink-0" aria-hidden="true" /><span class="truncate">{swarm.workspace}</span>
        </span>
        <Choice label="What agents may close without you" value={level} placeholder="Review level" options={LEVELS} onchange={saveLevel} />
      </div>
      <div class="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto">
        {#if !swarm.projectId}
          <p class="p-6 text-sm text-muted-foreground">This swarm was started before swarms kept their work as issues.</p>
        {:else if tab === 'board'}
          {#key swarm.projectId}<BoardPanel {chat} projectId={swarm.projectId} {live}
            onopen={number => { openIssue = number; openSeq++; tab = 'issues'; }} />{/key}
        {:else}
          {#key `${swarm.projectId}:${openSeq}`}<IssuesPanel {chat} projectId={swarm.projectId} {live} open={openIssue}
            onopened={() => { openIssue = null; }} />{/key}
        {/if}
      </div>

    {/if}
  </div>

  {#if !swarm}
    <div class="px-4 pb-4 pt-3 sm:px-[44px]">
      <div class="mx-auto max-w-2xl">
        {#if error}<p role="alert" class="mb-2 text-xs text-destructive">{error}</p>{/if}
        <div class="composer" role="group" aria-label="Message composer">
          <textarea aria-label={swarm ? 'File a feature for the swarm' : 'Shared goal'} rows="1" bind:value={text} onkeydown={keydown}
            placeholder={swarm ? 'File a feature for the swarm' : 'Describe the shared goal'}
            class="composer-input placeholder:text-muted-foreground focus:outline-none"></textarea>
          <div class="mt-2 flex items-center gap-2">
            <div class="flex min-w-0 flex-wrap items-center gap-0.5">
              {#if !swarm}
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
                <Choice label="What agents may close without you" value={level} placeholder="Review level" options={LEVELS} onchange={value => level = value as ReviewLevel} />
                <button class={['flex h-9 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs transition-colors',
                  unsandboxed ? 'bg-destructive/12 text-destructive' : 'text-muted-foreground hover:bg-border']}
                  aria-pressed={unsandboxed} onclick={() => unsandboxed = !unsandboxed}>
                  <ShieldOff size={13} aria-hidden="true" />Unsandboxed shell
                </button>
              {:else}
                <span class="px-2 text-xs text-muted-foreground">Filed as a feature in the Backlog; the next free development agent takes it.</span>
              {/if}
            </div>
            <button class="send-button ml-auto shrink-0" aria-label={swarm ? 'File the feature' : 'Launch the swarm'}
              disabled={busy || !text.trim() || (!swarm && (!devModel || (testCount !== '0' && !testModel) || (reviewCount !== '0' && !reviewModel)))}
              onclick={() => swarm ? file() : launch()}>
              <ArrowUp size={19} aria-hidden="true" />
            </button>
          </div>
        </div>
        {#if !swarm && unsandboxed}
          <p role="alert" class="mt-3 text-xs text-destructive">Unsandboxed shell: agents' commands run as you, outside MiniClaw's sandbox, with your files and network, until the swarm ends.</p>
        {:else if !swarm}
          <p class="mt-3 text-xs text-muted-foreground">{total} agents with the lead · each role has Anthropic skills · heartbeat every 5 s · lease 40 s</p>
        {/if}
      </div>
    </div>
  {/if}
</div>
