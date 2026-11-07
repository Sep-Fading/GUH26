<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words as platformWords } from './platform.svelte';
  // Loads the schedules when Settings shows this panel.
  import { onMount } from 'svelte';
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  // Marks the commands of this panel.
  import Plus from '@lucide/svelte/icons/plus';
  import Play from '@lucide/svelte/icons/play';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Confirms deletion of a schedule.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Reaches the schedule routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  type Schedule = { id: string; name: string; agentId: string; projectId: string | null; prompt: string;
    every: 'hourly' | 'daily' | 'weekly'; enabled: boolean; approvalMinutes: number;
    lastRun: number | null; lastStatus: string | null; lastResult: string | null; lastThread: string | null };
  type Draft = Pick<Schedule, 'name' | 'agentId' | 'projectId' | 'prompt' | 'every' | 'enabled' | 'approvalMinutes'>;

  let { chat, onopen }: { chat: Chat; onopen?: (thread: string) => void } = $props();
  let items = $state<Schedule[]>([]);
  let draft = $state<Draft | null>(null);
  let editing = $state<string>();
  let busy = $state(false), error = $state('');

  const words: Record<string, string> = { running: 'Running', done: 'Finished', failed: 'Failed', terminated: 'Terminated' };
  const every = { hourly: 'Every hour', daily: 'Every day', weekly: 'Every week' };
  const agentName = (id: string) => chat.agents.find(agent => agent.id === id)?.name ?? 'Deleted agent';
  const definition = (item: Schedule): Draft => ({ name: item.name, agentId: item.agentId, projectId: item.projectId,
    prompt: item.prompt, every: item.every, enabled: item.enabled, approvalMinutes: item.approvalMinutes });

  // Runs one request, then reloads the list; a failure is shown in words.
  async function attempt(work: () => Promise<unknown>) {
    busy = true; error = '';
    try { await work(); items = (await chat.schedules<{ schedules: Schedule[] }>()).schedules; return true; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The request failed.'; return false; }
    finally { busy = false; }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (draft && await attempt(() => chat.schedules(editing ?? '', $state.snapshot(draft)))) { draft = null; editing = undefined; }
  }

  onMount(() => { void attempt(async () => {}); });
</script>

<section aria-label="Schedules" class="flex min-h-0 flex-1 flex-col overflow-y-auto">
  <!-- The right padding keeps the button clear of the sheet's own close button. -->
  <div class="sticky top-0 z-[5] flex h-12 shrink-0 items-center justify-between border-b border-border bg-background pl-5 pr-14">
    <h2 class="text-base font-semibold">Schedules</h2>
    <button class="icon-button" aria-label="Reload schedules" disabled={busy} onclick={() => attempt(async () => {})}>
      <RefreshCw size={16} aria-hidden="true" /></button>
  </div>
  <div class="space-y-4 p-5 text-sm">
    <p class="text-xs text-muted-foreground">A schedule sends a saved prompt to one agent by itself, while MiniClaw is running.
      Every schedule starts switched off. Nobody is there to approve, so a run that needs an approval waits for the time
      you set and is then terminated; add a permission rule for the agent to allow it.</p>
    {#each items as item (item.id)}
      <div role="group" aria-label={item.name} class="rounded-lg border border-border p-3">
        <div class="flex flex-wrap items-center gap-3">
          <div class="min-w-0 flex-1">
            <p class="truncate font-medium">{item.name}</p>
            <p class="text-xs text-muted-foreground">{agentName(item.agentId)} · {every[item.every]} · {item.enabled ? 'On' : 'Off'}</p>
          </div>
          <Switch class="data-[state=checked]:bg-success" aria-label={`Run ${item.name} on schedule`} checked={item.enabled}
            disabled={busy} onCheckedChange={enabled => attempt(() => chat.schedules(item.id, { ...definition(item), enabled }))} />
          <Button variant="secondary" size="sm" disabled={busy || item.lastStatus === 'running'}
            onclick={() => attempt(() => chat.schedules(`${item.id}/run`, {}))}><Play size={14} aria-hidden="true" />Run now</Button>
          <Button variant="ghost" size="sm" disabled={busy} onclick={() => { draft = definition(item); editing = item.id; }}>Edit</Button>
          <ConfirmDelete label="Delete schedule" actionLabel="Delete schedule" disabled={busy}
            description={`Delete ${item.name}? The conversations of its earlier runs stay.`}
            action={async () => { await chat.schedules(`${item.id}/delete`, {}); await attempt(async () => {}); }} />
        </div>
        <p class="mt-2 text-xs" class:text-destructive={['failed', 'terminated'].includes(item.lastStatus ?? '')}
          class:text-muted-foreground={!['failed', 'terminated'].includes(item.lastStatus ?? '')}>
          {#if item.lastStatus}
            <span class="font-medium">{words[item.lastStatus] ?? item.lastStatus}</span>{item.lastRun
              ? ` · ${new Date(item.lastRun * 1000).toLocaleString()}` : ''}{item.lastResult ? ` · ${item.lastResult}` : ''}
          {:else}Never run{/if}
        </p>
        {#if item.lastThread && onopen}
          <button class="mt-1 text-xs text-brand underline" onclick={() => onopen(item.lastThread!)}>Open the last run's conversation</button>
        {/if}
      </div>
    {:else}
      {#if !draft}<p class="text-muted-foreground">No schedules yet.</p>{/if}
    {/each}
    {#if draft}
      <form class="space-y-3 rounded-lg border border-border p-3" onsubmit={save}>
        <label class="block">Name<Input class="mt-1" bind:value={draft.name} maxlength={80} required /></label>
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-col gap-1">Agent
            <select aria-label="Agent" class="rounded-md border border-border bg-background px-2 py-1.5" bind:value={draft.agentId} required>
              <option value="" disabled>Choose an agent</option>
              {#each chat.agents.filter(agent => agent.active) as agent (agent.id)}<option value={agent.id}>{agent.name}</option>{/each}
            </select></label>
          <label class="flex flex-col gap-1">Project
            <select aria-label="Project" class="rounded-md border border-border bg-background px-2 py-1.5"
              value={draft.projectId ?? ''} onchange={event => { draft!.projectId = event.currentTarget.value || null; }}>
              <option value="">None ({platformWords.machine})</option>
              {#each chat.projects.items as project (project.id)}<option value={project.id}>{project.name}</option>{/each}
            </select></label>
          <label class="flex flex-col gap-1">Runs
            <select aria-label="Runs" class="rounded-md border border-border bg-background px-2 py-1.5" bind:value={draft.every}>
              <option value="hourly">Every hour</option><option value="daily">Every day</option><option value="weekly">Every week</option>
            </select></label>
          <label class="flex flex-col gap-1">Approval wait
            <select aria-label="Approval wait" class="rounded-md border border-border bg-background px-2 py-1.5"
              value={String(draft.approvalMinutes)} onchange={event => { draft!.approvalMinutes = Number(event.currentTarget.value); }}>
              {#each [1, 2, 5, 15, 30, 60] as minutes (minutes)}<option value={String(minutes)}>{minutes} min, then terminate</option>{/each}
            </select></label>
        </div>
        <label class="block">Prompt<Textarea class="mt-1" rows={4} bind:value={draft.prompt} maxlength={8000} required /></label>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" onclick={() => { draft = null; editing = undefined; }}>Cancel</Button>
          <Button type="submit" disabled={busy || !draft.name.trim() || !draft.agentId || !draft.prompt.trim()}>Save schedule</Button>
        </div>
      </form>
    {:else}
      <Button variant="secondary" size="sm" onclick={() => { draft = { name: '', agentId: '', projectId: null, prompt: '',
        every: 'daily', enabled: false, approvalMinutes: 5 }; editing = undefined; }}><Plus size={14} aria-hidden="true" />New schedule</Button>
    {/if}
    {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  </div>
</section>
