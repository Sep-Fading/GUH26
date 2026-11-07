<script lang="ts">
  // Shortens native paths on either platform.
  import { lastParts } from './paths';
  // Marks each kind of tool call, its state and the expand arrow.
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Terminal from '@lucide/svelte/icons/terminal';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import FileText from '@lucide/svelte/icons/file-text';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import Search from '@lucide/svelte/icons/search';
  import Cable from '@lucide/svelte/icons/cable';
  import MessageCircleQuestion from '@lucide/svelte/icons/message-circle-question-mark';
  import Brain from '@lucide/svelte/icons/brain';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  // One recorded tool call, as the backend reports it.
  import type { ToolActivity } from './chat.svelte';

  let { entries, label = 'Activity' }: { entries: ToolActivity[]; label?: string } = $props();
  let open = $state<Record<string, boolean>>({});
  let folded = $state(false);

  const icons = { command: Terminal, edit: FilePen, write: FilePen, read: FileText, list: FolderOpen, search: Search, mcp: Cable,
    question: MessageCircleQuestion, memory: Brain, issue: CircleDot };
  const verbs: Record<string, string> = { command: 'Ran', edit: 'Edited', write: 'Wrote', read: 'Read',
    list: 'Listed', search: 'Searched for', mcp: 'Called', question: 'Asked',
    // A memory entry's title already says what happened: Remembered, Recalled or Forgot.
    memory: '',
    // An issue entry's title says it too: Filed, Claimed, Released or Closed.
    issue: '' };
  // A path shows its last two parts; the full path is in the tooltip and the detail.
  const short = (entry: ToolActivity) => ['command', 'search', 'mcp', 'question', 'memory', 'issue'].includes(entry.kind) ? entry.title
    : lastParts(entry.title, 2);
  const took = (entry: ToolActivity) => entry.endedAt === undefined ? '' : entry.endedAt - entry.startedAt < 1000
    ? `${entry.endedAt - entry.startedAt} ms` : `${((entry.endedAt - entry.startedAt) / 1000).toFixed(1)} s`;
  const counts = (diff: string) => diff.split('\n').reduce((sum, line) => ({
    added: sum.added + Number(line.startsWith('+') && !line.startsWith('+++')),
    removed: sum.removed + Number(line.startsWith('-') && !line.startsWith('---')) }), { added: 0, removed: 0 });
  const expandable = (entry: ToolActivity) => !!(entry.diff || entry.output || entry.detail || entry.refusedHosts?.length);
  const running = $derived(entries.filter(entry => entry.status === 'running').length);
  const lineClass = (line: string) => line.startsWith('@@') ? 'diff-hunk' : line.startsWith('+') ? 'diff-added'
    : line.startsWith('-') ? 'diff-removed' : line.startsWith('\\') ? 'diff-note' : 'diff-context';
</script>

<div role="group" aria-label={label} class="activity mb-3 rounded-lg border border-border text-xs">
  <button class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-muted-foreground" aria-expanded={!folded}
    onclick={() => { folded = !folded; }}>
    <ChevronRight size={13} class={folded ? '' : 'rotate-90'} aria-hidden="true" />
    <span class="font-medium">{label}</span>
    <span>{entries.length} step{entries.length === 1 ? '' : 's'}{running ? `, ${running} running` : ''}</span>
  </button>
  {#if !folded}
    <ul class="divide-y divide-border border-t border-border">
      {#each entries as entry (entry.id)}
        {@const Icon = icons[entry.kind as keyof typeof icons] ?? Terminal}
        {@const change = entry.diff ? counts(entry.diff) : null}
        <li>
          <button class="flex w-full items-center gap-2 px-3 py-1.5 text-left disabled:cursor-default"
            aria-expanded={!!open[entry.id]} disabled={!expandable(entry)} title={entry.title}
            onclick={() => { open[entry.id] = !open[entry.id]; }}>
            <ChevronRight size={12} class={`shrink-0 ${open[entry.id] ? 'rotate-90' : ''} ${expandable(entry) ? '' : 'opacity-0'}`} aria-hidden="true" />
            <Icon size={13} class="shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="shrink-0 text-muted-foreground">{verbs[entry.kind] ?? 'Did'}</span>
            <span class={['min-w-0 flex-1 truncate', !['question', 'memory', 'issue'].includes(entry.kind) && 'font-mono']}>{short(entry)}</span>
            {#if entry.agent}<span class="shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground">{entry.agent}</span>{/if}
            {#if change}<span class="shrink-0 font-mono"><span class="text-success">+{change.added}</span>
              <span class="text-destructive">-{change.removed}</span></span>{/if}
            {#if entry.kind === 'command' && entry.exitCode !== undefined && entry.exitCode !== 0 && entry.status !== 'running'}
              <span class="shrink-0 text-destructive">exit {entry.exitCode ?? 'signal'}</span>{/if}
            {#if entry.status === 'running'}
              <span class="flex shrink-0 items-center gap-1 text-brand"><LoaderCircle size={12} class="animate-spin" aria-hidden="true" />Running</span>
            {:else if entry.status === 'refused'}<span class="shrink-0 text-destructive">{entry.kind === 'question' ? 'Not answered' : 'Refused'}</span>
            {:else if entry.status === 'failed'}<span class="shrink-0 text-destructive">{entry.kind === 'mcp' ? 'Failed' : 'No change'}</span>
            {:else}<span class="shrink-0 text-muted-foreground">{took(entry)}</span>{/if}
          </button>
          <!-- A refusal says why in view, so the user need not open the entry to learn what to change. -->
          {#if entry.status === 'refused' && entry.kind !== 'question' && entry.output && !open[entry.id]}
            <p role="note" class="px-3 pb-1.5 pl-9 text-destructive [overflow-wrap:anywhere]">{entry.output}</p>
          {/if}
          <!-- A tool's pictures, such as a Studio screenshot, show without expanding the entry. -->
          {#if entry.pictures?.length}
            <ul aria-label={`Pictures from ${short(entry)}`} class="flex flex-wrap gap-2 px-3 pb-2 pl-9">
              {#each entry.pictures as picture (picture.id)}
                <li><a href={picture.url} target="_blank" rel="noreferrer" aria-label={`Open ${picture.name}`}
                  class="block overflow-hidden rounded-md border border-border focus-visible:outline-2 focus-visible:outline-ring">
                  <img src={picture.url} alt={picture.name} loading="lazy" class="max-h-24 min-h-10 min-w-10 max-w-40 bg-muted object-cover" /></a></li>
              {/each}
            </ul>
          {/if}
          {#if open[entry.id]}
            <div class="space-y-2 border-t border-border bg-muted/40 px-3 py-2">
              {#if !['command', 'search', 'mcp', 'question', 'memory', 'issue'].includes(entry.kind)}<p class="break-all font-mono text-muted-foreground">{entry.title}</p>{/if}
              {#if entry.detail}<p class="break-all text-muted-foreground">{entry.kind === 'command' ? 'In' : entry.kind === 'mcp' ? 'Sent' : 'Under'} <span class="font-mono">{entry.detail}</span></p>{/if}
              {#if entry.diff}
                <pre class="diff overflow-x-auto rounded-md border border-border bg-background font-mono leading-5">{#each entry.diff.replace(/\n$/, '').split('\n') as line, index (index)}<span class={lineClass(line)}>{line || ' '}</span>{/each}</pre>
              {:else if entry.diff === ''}<p class="text-muted-foreground">The file already had this content.</p>{/if}
              {#if entry.output}<pre class="max-h-72 overflow-auto whitespace-pre-wrap rounded-md border border-border bg-background p-2 font-mono leading-5 [overflow-wrap:anywhere]">{entry.output}</pre>{/if}
              {#if entry.refusedHosts?.length}<p class="text-destructive">Refused connections: {entry.refusedHosts.join(', ')}</p>{/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
