<script lang="ts">
  // The Board tab: every issue of the project in its stage, Backlog to Closed, updated live.
  // Choosing a card opens it in the Issues tab.
  // Icons for search and the stage that needs the user.
  import Search from '@lucide/svelte/icons/search';
  // The chat holds the project store.
  import type { Chat } from './chat.svelte';
  // Stages in board order, and the issue shape.
  import { STAGES, type IssueSeverity, type ProjectIssue } from './project-state.svelte';

  let { chat, projectId, live = 0, onopen }: { chat: Chat; projectId: string; live?: number;
    onopen?: (number: number) => void } = $props();

  const words: Record<IssueSeverity, string> = { low: 'Low', medium: 'Medium', high: 'High' };
  const tones: Record<IssueSeverity, string> = { low: 'text-muted-foreground',
    medium: 'text-amber-700 dark:text-amber-400', high: 'text-destructive' };

  let issues = $state<ProjectIssue[]>([]);
  let query = $state('');
  let error = $state('');
  let loaded = $state(false);
  // Each load takes a ticket; a reply for an older ticket or another project is dropped.
  let ticket = 0;

  // The issues matching the search, in each stage, most recently changed first.
  const columns = $derived.by(() => {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const shown = issues.filter(issue => words.every(word =>
      `#${issue.number} ${issue.title} ${issue.tags.join(' ')} ${issue.owner ?? ''}`.toLowerCase().includes(word)));
    return STAGES.map(stage => ({ ...stage, issues: shown.filter(issue => issue.status === stage.id) }));
  });

  // Reads every issue of the project; a failure keeps the last board and says why.
  async function refresh() {
    const mine = ++ticket, id = projectId;
    try {
      const reply = await chat.projects.issues(id);
      if (mine !== ticket || id !== projectId) return;
      issues = reply.issues; error = ''; loaded = true;
    } catch (cause) {
      if (mine === ticket && id === projectId) error = cause instanceof Error ? cause.message : 'The board could not be read.';
    }
  }

  // Loads when shown and whenever the workspace hears that the issues changed.
  $effect(() => { void projectId; void live; void refresh(); });
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <div class="p-3">
    <label class="flex items-center gap-2 rounded-md border border-input bg-background px-2.5 focus-within:border-ring">
      <Search size={14} class="shrink-0 text-muted-foreground" aria-hidden="true" />
      <span class="sr-only">Search the board</span>
      <input type="search" placeholder="Search titles, tags and owners" bind:value={query}
        class="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none" />
    </label>
  </div>
  {#if error}<p role="alert" class="mx-3 mb-2 rounded-md border border-border bg-muted px-2 py-1.5 text-xs text-destructive">{error}</p>{/if}
  <div role="list" aria-label="Issue stages" class="flex min-h-0 flex-1 snap-x gap-2.5 overflow-x-auto px-3 pb-3">
    {#each columns as column (column.id)}
      <section role="listitem" aria-label={`${column.label}, ${column.issues.length}`}
        class={['flex min-w-44 flex-1 basis-0 snap-start flex-col rounded-lg border border-border',
          column.id === 'review' && column.issues.length ? 'bg-brand/5' : 'bg-muted/40']}>
        <h3 class="flex items-center gap-1.5 px-3 pb-1.5 pt-2.5 text-xs font-medium text-muted-foreground">
          {#if column.id === 'review'}<span class="size-2 shrink-0 rounded-full bg-brand" aria-hidden="true"></span>{/if}
          <span class={column.id === 'review' ? 'text-foreground' : ''}>{column.label}</span>
          <span class="font-normal">{column.issues.length}</span>
          {#if column.id === 'review' && column.issues.length}<span class="ml-auto font-medium text-brand">Needs you</span>{/if}
        </h3>
        <ul class="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto px-2 pb-2">
          {#each column.issues as issue (issue.number)}
            <li>
              <button class="flex w-full flex-col gap-1 rounded-md border border-border bg-background p-2.5 text-left shadow-xs transition-colors hover:bg-accent"
                data-card={issue.number} onclick={() => onopen?.(issue.number)}>
                <span class={['text-[13px] leading-snug [overflow-wrap:anywhere]', issue.status === 'closed' && 'text-muted-foreground']}>{issue.title}</span>
                <span class="flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
                  <span>#{issue.number}</span><span aria-hidden="true">·</span>
                  <span class={['font-medium', tones[issue.severity]]}>{words[issue.severity]}</span>
                  {#if issue.kind === 'feature'}<span aria-hidden="true">·</span><span>Feature</span>{/if}
                </span>
                <span class="flex flex-wrap items-center gap-1">
                  <span class={['rounded px-1 text-[10px]', issue.owner ? 'bg-muted text-foreground' : 'text-muted-foreground italic']}>{issue.owner ?? 'Unclaimed'}</span>
                  {#each issue.tags as tag (tag)}<span class="rounded border border-border px-1 text-[10px] text-muted-foreground">{tag}</span>{/each}
                </span>
              </button>
            </li>
          {:else}
            <li class="rounded-md border border-dashed border-border px-2 py-3 text-center text-xs text-muted-foreground">{loaded ? 'Nothing here' : '…'}</li>
          {/each}
        </ul>
      </section>
    {/each}
  </div>
</div>
