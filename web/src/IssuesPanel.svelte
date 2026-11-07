<script lang="ts">
  // The Issues tab: a project's issues grouped by owner, and one issue with its history,
  // where the user closes what waits for review, gives it to an agent and adds notes.
  // Waits for the DOM before moving focus.
  import { tick } from 'svelte';
  // Styled buttons from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Icons for search, back, status, history and sending.
  import Search from '@lucide/svelte/icons/search';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  // The chat holds the project store and the agents.
  import type { Chat } from './chat.svelte';
  import { activeStage, stageLabel, type IssueEvent, type IssueSeverity, type ProjectIssue, type ReviewLevel } from './project-state.svelte';

  // `live` changes whenever the project's issues change; `open` is an issue to show first, chosen on the Board.
  let { chat, projectId, oncount, live = 0, open = null, onopened }: { chat: Chat; projectId: string; oncount?: (count: number) => void;
    live?: number; open?: number | null; onopened?: () => void } = $props();
  type View = 'open' | 'review' | 'closed';

  const views: { id: View; label: string }[] = [
    { id: 'open', label: 'Open' }, { id: 'review', label: 'Needs you' }, { id: 'closed', label: 'Closed' }];
  const words: Record<string, string> = { none: 'None', low: 'Low', medium: 'Medium', high: 'High' };
  const tones: Record<IssueSeverity, string> = { low: 'text-muted-foreground',
    medium: 'text-amber-700 dark:text-amber-400', high: 'text-destructive' };
  const verbs: Record<string, string> = { filed: 'Filed', claim: 'Claimed', handover: 'Released', done: 'Marked done',
    closed: 'Closed', comment: 'Noted', severity: 'Severity', owner: 'Owner', reopened: 'Reopened', stage: 'Moved to' };

  let view = $state<View>('open');
  let query = $state('');
  let severity = $state<IssueSeverity | 'all'>('all');
  let rows = $state<ProjectIssue[]>([]);
  let all = $state<ProjectIssue[]>([]);
  let level = $state<ReviewLevel>('low');
  let selected = $state<number | null>(null);
  let current = $state<ProjectIssue | null>(null);
  // A failed read is replaced by the next good one; a refused change stays until the user acts again.
  let loadError = $state('');
  let actionError = $state('');
  const error = $derived(actionError || loadError);
  let busy = $state(false);
  let draft = $state('');
  let historyOpen = $state(true);
  // Each load takes a ticket; a reply for an older ticket or another project is dropped.
  let ticket = 0;
  // The panel's root, for finding the control that should take focus.
  let root = $state<HTMLElement>();

  const project = $derived(chat.projects.items.find(item => item.id === projectId));
  // The project's agents by name, for the owner choice.
  const agents = $derived(chat.agents.filter(agent => project?.agentIds.includes(agent.id))
    .sort((a, b) => a.name.localeCompare(b.name)));
  // Which view an issue belongs to: every working stage counts as open.
  const viewOf = (issue: ProjectIssue): View => activeStage(issue.status) ? 'open' : issue.status as View;
  const counts = $derived(Object.fromEntries(views.map(item => [item.id, all.filter(issue => viewOf(issue) === item.id).length])));
  // Owners alphabetically, with unowned issues last.
  const groups = $derived.by(() => {
    const shown = rows.filter(issue => viewOf(issue) === view && (severity === 'all' || issue.severity === severity));
    const owners = new Map<string, { name: string; issues: ProjectIssue[] }>();
    for (const issue of shown) {
      const key = issue.ownerId ?? '';
      if (!owners.has(key)) owners.set(key, { name: issue.owner ?? 'Unclaimed', issues: [] });
      owners.get(key)!.issues.push(issue);
    }
    return [...owners.entries()].sort(([a, x], [b, y]) => (a === '') !== (b === '') ? (a === '' ? 1 : -1) : x.name.localeCompare(y.name))
      .map(([key, group]) => ({ key, ...group }));
  });

  // Turns an error into the sentence shown above the list or issue.
  function reason(cause: unknown, fallback: string) { return cause instanceof Error ? cause.message : fallback; }

  // Loads the list for the current words and view, the counts, and the open issue.
  async function refresh() {
    const mine = ++ticket, id = projectId, number = selected;
    try {
      const [list, everything, issue] = await Promise.all([chat.projects.issues(id, query, view === 'open' ? undefined : view), chat.projects.issues(id),
        number === null ? Promise.resolve(null) : chat.projects.issue(id, number)]);
      if (mine !== ticket || id !== projectId) return;
      rows = list.issues; all = everything.issues; level = everything.closeWithoutReview;
      if (number !== null && number === selected) current = issue;
      loadError = '';
      oncount?.(all.filter(item => item.status === 'review').length);
    } catch (cause) {
      if (mine === ticket && id === projectId) loadError = reason(cause, 'The issues could not be read.');
    }
  }

  // Runs one change to the open issue, then reloads; it says whether the change was accepted.
  async function act(work: () => Promise<unknown>) {
    busy = true; actionError = '';
    let done = true;
    try { await work(); } catch (cause) { actionError = reason(cause, 'The issue could not be changed.'); done = false; }
    finally { busy = false; }
    await refresh();
    return done;
  }

  // Focuses the first element in the panel that matches, once the DOM has caught up.
  async function focus(...selectors: string[]) {
    await tick();
    for (const selector of selectors) {
      const target = root?.querySelector<HTMLElement>(selector);
      if (target) { target.focus(); return; }
    }
  }

  // Opens one issue and focuses its title, or returns to the list and focuses the row it came from.
  async function show(number: number | null) {
    const previous = selected;
    selected = number; current = null; draft = ''; actionError = '';
    if (number !== null) { await refresh(); await focus('#issue-title'); }
    else await focus(`[data-issue="${previous}"]`, 'input[type="search"]');
  }

  // Closes or reopens the issue; the buttons that did it are gone, so focus goes to Reopen or the title.
  async function settle(work: () => Promise<unknown>) {
    await act(work);
    await focus('[data-focus="reopen"]', '#issue-title');
  }

  // Applies an owner or severity choice; a refused or ignored one puts the select back.
  async function choose(select: HTMLSelectElement, was: string, work: () => Promise<unknown>) {
    if (busy || !(await act(work))) select.value = was;
  }

  // Adds the user's note to the open issue's history.
  function send(event: SubmitEvent) {
    event.preventDefault();
    const text = draft.trim(), number = selected;
    if (!text || number === null || busy) return;
    void act(async () => { await chat.projects.commentIssue(projectId, number, text); draft = ''; });
  }

  // A short age, such as "now", "5 min", "3 h" or "2 d".
  function ago(at: number) {
    const minutes = Math.floor((Date.now() - at) / 60000);
    return minutes < 1 ? 'now' : minutes < 60 ? `${minutes} min` : minutes < 1440 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 1440)} d`;
  }

  // What a history row says after its verb, without repeating it.
  function summary(event: IssueEvent, issue: ProjectIssue) {
    if (event.kind === 'filed') return `this ${issue.kind}`;
    if (event.kind === 'handover') return 'with a handover';
    if (event.kind === 'done' || event.kind === 'closed') return event.text ? 'with a note' : `#${issue.number}`;
    if (event.kind === 'comment') return event.text.split('\n')[0];
    return event.text || `#${issue.number}`;
  }

  // Whether a history row shows its text in full below it.
  const expanded = (event: IssueEvent) => ['handover', 'done', 'comment', 'closed'].includes(event.kind) && !!event.text;

  // Fresh words, view or project load again after a short pause for typing.
  $effect(() => {
    void query; void view; void projectId;
    const timer = setTimeout(() => void refresh(), 150);
    return () => clearTimeout(timer);
  });
  // Agents change issues in the background; the workspace hears it live and bumps `live`.
  let heard: number | undefined;
  $effect(() => { const now = live; if (heard !== undefined && now !== heard) void refresh(); heard = now; });
  // An issue chosen on the Board opens at once.
  $effect(() => { if (open !== null) { const number = open; onopened?.(); void show(number); } });
  // Another project's issue is not shown.
  let shownProject: string | undefined;
  $effect(() => {
    const id = projectId;
    if (shownProject !== undefined && shownProject !== id) { selected = null; current = null; actionError = ''; }
    shownProject = id;
  });
</script>

<div class="flex min-h-0 flex-1 flex-col" bind:this={root}>
  {#if error}<p role="alert" class="mx-3 mt-3 rounded-md border border-border bg-muted px-2 py-1.5 text-xs text-destructive">{error}</p>{/if}
  {#if selected === null}
    <div class="flex flex-col gap-2.5 p-3">
      <label class="flex items-center gap-2 rounded-md border border-input bg-background px-2.5 focus-within:border-ring">
        <Search size={14} class="shrink-0 text-muted-foreground" aria-hidden="true" />
        <span class="sr-only">Search issues</span>
        <input type="search" placeholder="Search issues and tags" bind:value={query}
          class="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none" />
      </label>
      <div class="flex items-center gap-1.5">
        <div role="group" aria-label="Issue status" class="flex min-w-0 flex-1 gap-0.5">
          {#each views as item (item.id)}
            <button aria-pressed={view === item.id} class="workspace-tab aria-pressed:bg-muted aria-pressed:font-medium aria-pressed:text-foreground"
              onclick={() => { view = item.id; actionError = ''; }}>
              {item.label}<span class="sr-only">,</span> <span class="font-normal text-muted-foreground">{counts[item.id]}</span>
            </button>
          {/each}
        </div>
        <label class="flex items-center">
          <span class="sr-only">Severity filter</span>
          <select bind:value={severity} class="rounded-md border border-input bg-background px-1.5 py-1 text-xs">
            <option value="all">Any severity</option>
            <option value="high">High</option>
            <option value="medium">Medium</option>
            <option value="low">Low</option>
          </select>
        </label>
      </div>
    </div>
    <div class="min-h-0 flex-1 overflow-y-auto px-3 pb-4">
      {#each groups as group (group.key)}
        <div role="group" aria-label={`${group.name} issues`} class="pb-1">
          <h3 class="mx-1 mb-1 mt-2 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <ChevronRight size={12} class="shrink-0 rotate-90" aria-hidden="true" />
            <span class="truncate">{group.name}</span>
            <span class="ml-1 h-px min-w-3 flex-1 bg-border" aria-hidden="true"></span>
          </h3>
          {#each group.issues as issue (issue.number)}
            <button class="flex w-full items-start gap-2 rounded-md p-2 text-left hover:bg-muted" data-issue={issue.number}
              onclick={() => show(issue.number)}>
              <span class="flex h-5 w-3.5 shrink-0 items-center justify-center text-muted-foreground" aria-hidden="true">
                {#if issue.status === 'review'}<span class="size-2 rounded-full bg-brand"></span>
                {:else if issue.status === 'closed'}<Check size={13} strokeWidth={2.4} />
                {:else if issue.ownerId}<span class="size-2 rounded-full bg-muted-foreground"></span>
                {:else}<span class="size-2 rounded-full border-[1.5px] border-muted-foreground"></span>{/if}
              </span>
              <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                <span class={['text-sm leading-5 [overflow-wrap:anywhere]', issue.status === 'closed' && 'text-muted-foreground']}>{issue.title}</span>
                <span class="flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
                  <span>#{issue.number}</span><span aria-hidden="true">·</span>
                  <span class={['font-medium', tones[issue.severity]]}>{words[issue.severity]}</span>
                  {#if activeStage(issue.status)}<span aria-hidden="true">·</span><span>{stageLabel(issue.status)}</span>{/if}
                  {#if issue.tags.length}<span aria-hidden="true">·</span><span>{issue.tags.join(', ')}</span>{/if}
                </span>
              </span>
              {#if issue.status === 'review'}<span class="shrink-0 text-xs font-medium leading-5 text-brand">Needs you</span>{/if}
            </button>
          {/each}
        </div>
      {:else}<p class="p-3 text-sm text-muted-foreground">{query.trim() || severity !== 'all' ? 'No issues match.'
        : view === 'review' ? 'Nothing waits for you.' : view === 'closed' ? 'No closed issues.' : 'No open issues. Agents file them as they work.'}</p>{/each}
      <p class="mx-1 mt-3 text-xs leading-normal text-muted-foreground">
        {level === 'none' ? 'Every finished issue waits for you.' : level === 'high' ? 'Agents close finished issues by themselves.'
          : `Agents close ${level === 'low' ? 'Low' : 'Low and Medium'} issues by themselves; ${level === 'low' ? 'Medium and High wait' : 'High waits'} for you.`}
        Change the level beside the Board and Issues tabs.</p>
    </div>
  {:else}
    <div class="flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto px-4 pb-4 pt-2">
      <div class="-ml-2 flex items-center gap-1">
        <button class="icon-button" aria-label="Back to issues" title="Back to issues" onclick={() => show(null)}>
          <ChevronLeft size={16} aria-hidden="true" /></button>
        <span class="text-xs text-muted-foreground">Issues</span>
        <span class="ml-auto text-xs text-muted-foreground">#{selected}</span>
      </div>
      {#if current}
        {@const issue = current}
        <div class="space-y-1">
          <h2 id="issue-title" tabindex="-1" class="text-base font-semibold leading-snug outline-none [overflow-wrap:anywhere]">{issue.title}</h2>
          <p class="text-xs text-muted-foreground">{issue.kind === 'bug' ? 'Bug' : 'Feature'} · {stageLabel(issue.status)} ·
            <span class={['font-medium', tones[issue.severity]]}>{words[issue.severity]}</span>
            {#if issue.tags.length}· {issue.tags.join(', ')}{/if} · filed by {issue.createdBy} {ago(issue.createdAt) === 'now' ? 'just now' : `${ago(issue.createdAt)} ago`}</p>
        </div>

        {#if issue.status === 'review'}
          <section aria-label="Close approval" class="rounded-lg border border-border bg-background p-3">
            {#if issue.owner}<p class="mb-2 text-xs text-muted-foreground">Requested by {issue.owner}</p>{/if}
            <p class="text-sm font-medium">Close #{issue.number} as done?</p>
            <p class="mt-1 text-xs text-muted-foreground">{words[issue.severity]} is above what this project closes without review
              ({words[level]}). The agent's note is the last entry below.</p>
            <div class="mt-3 flex flex-wrap justify-end gap-3">
              <Button variant="secondary" disabled={busy} onclick={() => settle(() => chat.projects.reopenIssue(projectId, issue.number))}>
                <X size={16} aria-hidden="true" />Not done</Button>
              <Button disabled={busy} onclick={() => settle(() => chat.projects.closeIssue(projectId, issue.number))}>
                <Check size={16} aria-hidden="true" />Close it</Button>
            </div>
          </section>
        {:else if issue.status === 'closed'}
          <div class="flex items-center gap-2 text-xs text-muted-foreground">
            <Check size={13} aria-hidden="true" /><span role="status">Closed.</span>
            <Button class="ml-auto" size="sm" variant="outline" disabled={busy} data-focus="reopen"
              onclick={() => settle(() => chat.projects.reopenIssue(projectId, issue.number))}><RotateCcw size={14} aria-hidden="true" />Reopen</Button>
          </div>
        {/if}

        <div class="rounded-lg border border-border text-xs">
          <div class="flex flex-wrap items-center gap-2 px-3 py-2">
            <span class="text-muted-foreground">Owner</span>
            <span class={['font-medium', issue.owner ? 'text-brand' : 'text-muted-foreground']}>{issue.owner ?? 'Nobody'}</span>
            <label class="ml-auto flex">
              <span class="sr-only">Change owner</span>
              <select value={issue.ownerId ?? ''} aria-busy={busy} class="rounded-md border border-input bg-background px-1.5 py-1 text-xs"
                onchange={event => { const owner = event.currentTarget.value || null;
                  void choose(event.currentTarget, issue.ownerId ?? '', () => chat.projects.assignIssue(projectId, issue.number, owner)); }}>
                {#each agents as agent (agent.id)}<option value={agent.id}>{agent.name}</option>{/each}
                {#if issue.ownerId && issue.ownerId !== 'main' && !agents.some(agent => agent.id === issue.ownerId)}
                  <option value={issue.ownerId}>{issue.owner}</option>{/if}
                <option value="main">Main agent</option>
                <option value="">Nobody</option>
              </select>
            </label>
          </div>
          <p class="border-t border-border px-3 py-2 leading-normal text-muted-foreground">{issue.owner
            ? `Only ${issue.owner} decides when this is done. It can hand parts to other agents, and has to leave a handover to let go.`
            : 'Unclaimed. The next agent that claims it owns it.'}</p>
        </div>

        <div class="flex items-center gap-2 text-xs">
          <label for="issue-severity" class="text-muted-foreground">Severity</label>
          <span class={['size-2 shrink-0 rounded-full bg-current', tones[issue.severity]]} aria-hidden="true"></span>
          <select id="issue-severity" value={issue.severity} aria-busy={busy} disabled={issue.status === 'closed'}
            class="rounded-md border border-input bg-background px-1.5 py-1 text-xs font-medium text-foreground"
            onchange={event => { const next = event.currentTarget.value as IssueSeverity;
              void choose(event.currentTarget, issue.severity, () => chat.projects.setIssueSeverity(projectId, issue.number, next)); }}>
            <option value="low" class="text-foreground">Low</option><option value="medium" class="text-foreground">Medium</option><option value="high" class="text-foreground">High</option>
          </select>
        </div>

        <p class="whitespace-pre-wrap text-sm leading-relaxed [overflow-wrap:anywhere]">{issue.text}</p>

        {@const events = issue.events ?? []}
        <div role="group" aria-label="History" class="rounded-lg border border-border text-xs">
          <button class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-muted-foreground" aria-expanded={historyOpen}
            onclick={() => { historyOpen = !historyOpen; }}>
            <ChevronRight size={13} class={historyOpen ? 'rotate-90' : ''} aria-hidden="true" />
            <span class="font-medium">History</span><span>{events.length} entr{events.length === 1 ? 'y' : 'ies'}</span>
          </button>
          {#if historyOpen && events.length}
            <ul class="divide-y divide-border border-t border-border">
              {#each events as event, index (index)}
                <li>
                  <div class="flex items-center gap-2 px-3 py-1.5">
                    <span class="shrink-0 text-muted-foreground">{verbs[event.kind] ?? 'Changed'}</span>
                    <span class="min-w-0 flex-1 truncate" title={event.text}>{summary(event, issue)}</span>
                    <span class="shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground">{event.author}</span>
                    {#if event.kind === 'done' && issue.status === 'review' && index === events.length - 1}
                      <span class="shrink-0 font-medium text-brand">Needs you</span>
                    {:else}<span class="shrink-0 text-muted-foreground">{ago(event.at)}</span>{/if}
                  </div>
                  {#if expanded(event)}
                    <div class="border-t border-border bg-muted/40 px-3 py-2.5">
                      <p class="mb-1 font-medium text-brand">{event.kind === 'handover' ? `Handover from ${event.author}` : event.author}</p>
                      <p class="whitespace-pre-line text-[13px] leading-relaxed text-foreground [overflow-wrap:anywhere]">{event.text}</p>
                    </div>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        <form class="composer flex items-end gap-2 !py-2.5 !pr-2.5" onsubmit={send}>
          <label class="flex flex-1">
            <span class="sr-only">Add a note to the history</span>
            <textarea rows="2" bind:value={draft} maxlength={8192} aria-busy={busy}
              placeholder="Add a note. Agents read the history before they claim or close."
              class="composer-input text-sm"
              onkeydown={event => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
                event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }}></textarea>
          </label>
          <button type="submit" class="send-button" aria-label="Add note" disabled={busy || !draft.trim()}>
            <ArrowUp size={16} aria-hidden="true" /></button>
        </form>
      {:else if !error}<p class="text-sm text-muted-foreground">Loading…</p>{/if}
    </div>
  {/if}
</div>
