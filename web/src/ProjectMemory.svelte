<script lang="ts">
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  // Marks the commands of one note.
  import Pencil from '@lucide/svelte/icons/pencil';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import Plus from '@lucide/svelte/icons/plus';
  // Reads and changes the project's notes through the backend.
  import type { Projects, MemoryNote } from './project-state.svelte';

  let { projects, id }: { projects: Projects; id: string } = $props();
  let notes = $state<MemoryNote[]>([]), error = $state(''), busy = $state(false);
  // The note being edited: its saved name, or '' for a new one; undefined when none is open.
  let editing = $state<string>(), draft = $state({ name: '', description: '', text: '' });

  // Runs one change and reloads the list; a failure stays beside the list.
  async function attempt(work: () => Promise<unknown>) {
    busy = true; error = '';
    try { await work(); notes = await projects.memory(id); return true; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The project\'s memory could not be changed.'; return false; }
    finally { busy = false; }
  }

  function open(note?: MemoryNote) {
    editing = note?.name ?? '';
    draft = note ? { name: note.name, description: note.description, text: note.text } : { name: '', description: '', text: '' };
    error = '';
  }

  async function save() {
    if (await attempt(() => projects.saveMemory(id, $state.snapshot(draft), editing || undefined))) editing = undefined;
  }

  $effect(() => { void id; void attempt(async () => {}); });
</script>

<section aria-label="Project memory" class="space-y-2 border-t border-border pt-4 text-sm">
  <div class="flex items-center justify-between gap-2">
    <h3 class="font-medium">Memory</h3>
    <Button type="button" size="sm" variant="outline" disabled={busy || editing !== undefined || notes.length >= 64} onclick={() => open()}>
      <Plus size={14} aria-hidden="true" />Add note
    </Button>
  </div>
  <p class="text-xs text-muted-foreground">Short notes every conversation of this project can read. The agent saves them as it works, and you can
    add, change or delete any of them. A conversation is told the names and descriptions once and reads a note only when it needs it.</p>
  {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  {#if editing !== undefined}
    <div role="group" aria-label={editing ? `Edit note ${editing}` : 'New note'} class="space-y-2 rounded-md border border-border p-3">
      <Input aria-label="Note name" placeholder="Name, a few words" maxlength={80} bind:value={draft.name} disabled={busy} />
      <Input aria-label="Note description" placeholder="One sentence: when is this worth reading?" maxlength={200} bind:value={draft.description} disabled={busy} />
      <Textarea aria-label="Note text" rows={4} maxlength={8192} bind:value={draft.text} disabled={busy} />
      <div class="flex justify-end gap-2">
        <Button type="button" size="sm" variant="ghost" disabled={busy} onclick={() => { editing = undefined; error = ''; }}>Cancel</Button>
        <Button type="button" size="sm" disabled={busy || !draft.name.trim() || !draft.description.trim() || !draft.text.trim()} onclick={save}>Save note</Button>
      </div>
    </div>
  {/if}
  <ul class="space-y-1">
    {#each notes as note (note.name)}
      <li class="flex items-start gap-2 rounded-md border border-border px-3 py-2">
        <div class="min-w-0 flex-1">
          <p class="break-words font-medium [overflow-wrap:anywhere]">{note.name}</p>
          <p class="break-words text-xs text-muted-foreground">{note.description}</p>
        </div>
        <button type="button" class="icon-button shrink-0" aria-label={`Edit note ${note.name}`} disabled={busy || editing !== undefined}
          onclick={() => open(note)}><Pencil size={14} aria-hidden="true" /></button>
        <button type="button" class="icon-button shrink-0 hover:text-destructive" aria-label={`Delete note ${note.name}`} disabled={busy || editing !== undefined}
          onclick={() => attempt(() => projects.removeMemory(id, note.name))}><Trash2 size={14} aria-hidden="true" /></button>
      </li>
    {:else}
      <li class="text-xs text-muted-foreground">Nothing remembered yet.</li>
    {/each}
  </ul>
</section>
