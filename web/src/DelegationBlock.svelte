<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Marks the run, its state and the expand arrow.
  import Users from '@lucide/svelte/icons/users';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  // Shows what the delegated agent did, and renders its answer.
  import ActivityList from './ActivityList.svelte';
  import MessageMarkdown from './MessageMarkdown.svelte';
  // Says where the run is, in the same words as its card in the Agents tab.
  import { runPhase } from './delegation-words';
  // Shows the pictures and files that went with the task.
  import MessageFiles from './MessageFiles.svelte';
  import type { Chat, DelegatedRun, ToolActivity } from './chat.svelte';

  let { chat, run, entries }: { chat: Chat; run: DelegatedRun; entries: ToolActivity[] } = $props();
  let open = $state(false), loading = $state(false), error = $state('');
  let opening = $state(false), waiting = $state('');
  let transcript = $state<{ id: string; role: string; text: string }[]>([]);

  const phase = $derived(runPhase(run));
  const took = $derived(run.ended ? `${Math.max(1, Math.round((run.ended - run.started) / 1000))} s` : '');

  // Refreshes an expanded run while it works; closing or switching runs discards late replies.
  $effect(() => {
    const thread = run.thread, running = run.status === 'running';
    if (!open || !thread || !chat.ready) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    loading = true;
    // Reads the current transcript and schedules one follow-up after the response arrives.
    async function refresh() {
      try {
        const result = await chat.runTranscript(thread!);
        if (cancelled) return;
        transcript = result.messages;
        waiting = result.scopedApproval ? 'Waiting for approval' : result.review ? 'Waiting for file review' : '';
        error = '';
      } catch (cause) {
        if (!cancelled) error = cause instanceof Error ? cause.message : 'The run could not be loaded.';
      } finally {
        if (!cancelled) {
          loading = false;
          if (running) timer = setTimeout(refresh, 1000);
        }
      }
    }
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  });

  // Opens the same conversation without duplicating the delegated task.
  async function openConversation() {
    if (opening) return;
    opening = true; error = '';
    try { await chat.continueWithRun(run); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The conversation could not be opened.'; }
    finally { opening = false; }
  }
</script>

<div role="group" aria-label={`Delegated to ${run.agent}`} data-run-id={run.id} class="mb-3 scroll-mt-4 rounded-lg border border-border text-sm">
  <div class="flex flex-wrap items-center gap-2 px-3 py-2">
    <Users size={15} class="shrink-0 text-brand" aria-hidden="true" />
    <span class="font-medium">Delegated to {run.agent}</span>
    <span class="flex items-center gap-1 text-xs text-muted-foreground">
      {#if run.status === 'running'}<LoaderCircle size={12} class="animate-spin" aria-hidden="true" />{/if}{phase}{took ? ` · ${took}` : ''}</span>
  </div>
  <p class="border-t border-border px-3 py-2 text-xs text-muted-foreground [overflow-wrap:anywhere]"><span class="font-medium">Task:</span> {run.task}</p>
  {#if run.attachments?.length}<div class="px-3"><MessageFiles thread={run.parent} files={run.attachments} compact label="Files handed over" /></div>{/if}
  {#if entries.length}<div class="px-3"><ActivityList {entries} /></div>{/if}
  {#if run.result}
    <div class="border-t border-border px-3 py-3">
      <!-- The name says who is speaking: the delegated agent, never the conversation's own agent. -->
      <p class="mb-1 text-xs font-medium text-brand">{run.agent}{run.status === 'done' ? '' : ' (no answer)'}</p>
      <article aria-label={`Answer from ${run.agent}`} class="break-words leading-7 [overflow-wrap:anywhere]">
        {#if run.status === 'done'}<MessageMarkdown text={run.result} />{:else}<p class="text-muted-foreground">{run.result}</p>{/if}
      </article>
    </div>
  {/if}
  {#if run.thread}
    <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
      <button class="flex items-center gap-1 text-xs text-muted-foreground" aria-expanded={open} onclick={() => open = !open}>
        <ChevronRight size={12} class={open ? 'rotate-90' : ''} aria-hidden="true" />{open ? 'Hide the full run' : 'Show the full run'}</button>
      <Button variant="ghost" size="sm" class="ml-auto" disabled={opening} onclick={openConversation}>
        {run.status === 'running' || run.detached ? `Open conversation with ${run.agent}` : `Continue with ${run.agent}`}</Button>
    </div>
    {#if open}
      <div class="space-y-3 border-t border-border bg-muted/40 px-3 py-3 text-sm">
        {#if loading}<p class="text-xs text-muted-foreground">Loading...</p>{/if}
        {#if waiting}<p role="status" class="text-xs text-muted-foreground">{waiting}</p>{/if}
        {#each transcript as message (message.id)}
          <div><p class="text-xs font-medium text-muted-foreground">{message.role === 'user' ? 'Task' : run.agent}</p>
            <div class="[overflow-wrap:anywhere]">{#if message.role === 'assistant'}<MessageMarkdown text={message.text} />{:else}{message.text}{/if}</div></div>
        {/each}
      </div>
    {/if}
  {/if}
  {#if error}<p role="alert" class="border-t border-border px-3 py-2 text-xs text-destructive">{error}</p>{/if}
</div>
