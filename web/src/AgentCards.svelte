<script lang="ts">
  // The workspace's Agents tab: one card for each agent this conversation handed a task to, with
  // the agent's name, the summary its request gave, where it is, and a line that follows what it
  // is doing now. Running and waiting runs come first.
  import Users from '@lucide/svelte/icons/users';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
  // The conversation's runs and their live lines.
  import type { Chat, DelegatedRun } from './chat.svelte';
  // The words a run is shown with, shared with its block in the conversation.
  import { runLine, runPhase, runSummary } from './delegation-words';

  let { chat, onshow }: { chat: Chat; onshow?: () => void } = $props();

  const active = (run: DelegatedRun) => run.status === 'running' || run.status === 'asking';
  const runs = $derived([...chat.delegations].sort((a, b) => Number(active(b)) - Number(active(a)) || b.started - a.started));

  // Brings the run's block in the conversation into view.
  function show(run: DelegatedRun) {
    onshow?.();
    document.querySelector(`[data-run-id="${CSS.escape(run.id)}"]`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
</script>

<div class="min-h-0 flex-1 overflow-y-auto p-3">
  {#if runs.length}
    <ul aria-label="Agents in this conversation" class="space-y-2">
      {#each runs as run (run.id)}
        {@const summary = runSummary(run)}
        <li>
          <article aria-label={`${run.agent}: ${summary}`} class="rounded-lg border border-border bg-card p-3 text-sm">
            <div class="flex items-center gap-2">
              <Users size={14} class="shrink-0 text-brand" aria-hidden="true" />
              <span class="min-w-0 flex-1 truncate font-medium">{run.agent}</span>
              <span class={['flex shrink-0 items-center gap-1 text-xs', run.status === 'failed' ? 'text-destructive' : 'text-muted-foreground']}>
                {#if active(run)}<LoaderCircle size={12} class="animate-spin" aria-hidden="true" />{/if}{runPhase(run)}</span>
            </div>
            <p class="mt-1 [overflow-wrap:anywhere]">{summary}</p>
            <!-- The live line: it changes as the agent works, so it is announced politely. -->
            <p role="status" aria-live="polite" title={runLine(run)}
              class={['mt-2 truncate rounded-md bg-muted px-2 py-1 text-xs', active(run) ? 'font-mono text-foreground' : 'text-muted-foreground']}>{runLine(run)}</p>
            <button class="mt-2 flex items-center gap-1 text-xs text-brand underline-offset-2 hover:underline" onclick={() => show(run)}>
              <ArrowUpRight size={12} aria-hidden="true" />Show in conversation</button>
          </article>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="p-3 text-sm text-muted-foreground">No agents yet. When this conversation hands a task to another agent, its card appears here and shows what it is doing.</p>
  {/if}
</div>
