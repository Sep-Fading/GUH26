<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Keeps selection dialogs and tooltips accessible.
  import { Dialog, Tooltip } from 'bits-ui';
  // Identifies skill selection, saving, and closing.
  import BookOpen from '@lucide/svelte/icons/book-open';
  import Save from '@lucide/svelte/icons/save';
  import X from '@lucide/svelte/icons/x';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Shares group selection controls with projects.
  import GroupChoices from './GroupChoices.svelte';
  // Reads the current conversation and registries.
  import type { Chat } from './chat.svelte';
  // Preserves independent skill and group references.
  import type { SkillSelection } from './skill-groups.svelte';

  let { chat }: { chat: Chat } = $props();
  let open = $state(false), loading = $state(false), saving = $state(false);
  let loaded = $state(false);
  let error = $state('');
  let draft = $state<SkillSelection>({ skillIds: [], skillGroupIds: [], revision: 0 });
  let request = 0;
  const project = $derived(chat.projects.items.find(p => p.id === chat.projectId));
  const inherited = $derived(!chat.agentId ? project?.skillGroupIds ?? [] : []);

  // Loads explicit selections while keeping conversation content and draft unchanged.
  async function load() {
    const current = ++request;
    loading = true; loaded = false; error = '';
    void chat.skills.refresh(); void chat.groups.refresh();
    try {
      const value = chat.active ? await chat.groups.selection(chat.active.id) : $state.snapshot(chat.newSkillSelection);
      if (current === request) { draft = value; loaded = true; }
    } catch (failure) {
      if (current === request) error = failure instanceof Error ? failure.message : 'Could not load selection';
    } finally { if (current === request) loading = false; }
  }

  // Opens a fresh selection or closes only when no save is pending.
  function opened(value: boolean) {
    if (saving) return;
    open = value;
    if (value) void load(); else request++;
  }

  // Saves references for the next message without interrupting an active response.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!loaded || loading || saving || !chat.ready) return;
    saving = true; error = '';
    try {
      if (chat.active) await chat.groups.selection(chat.active.id, $state.snapshot(draft));
      else chat.newSkillSelection = $state.snapshot(draft);
      open = false;
    } catch (failure) { error = failure instanceof Error ? failure.message : 'Could not save selection'; }
    finally { saving = false; }
  }
</script>

<Dialog.Root {open} onOpenChange={opened}>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Dialog.Trigger {...props} class="icon-button shrink-0" aria-label="Conversation skills"
          disabled={!chat.ready || chat.working || chat.switching}>
          <BookOpen size={18} aria-hidden="true" />
        </Dialog.Trigger>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content class="tooltip" sideOffset={8}>Conversation skills</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
    <Dialog.Content escapeKeydownBehavior={saving ? 'ignore' : 'close'}
      class="fixed left-1/2 top-1/2 z-50 flex max-h-[90dvh] w-[min(440px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-background shadow-lg">
      <div class="flex items-center justify-between border-b border-border px-5 py-3">
        <Dialog.Title class="text-base font-semibold">Conversation skills</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close conversation skills" disabled={saving}>
          <X size={18} aria-hidden="true" />
        </Dialog.Close>
      </div>
      <form class="min-h-0 space-y-5 overflow-y-auto p-5" onsubmit={save}>
        <GroupChoices registry={chat.groups} bind:selected={draft.skillGroupIds} disabled={saving || loading} />
        {#if inherited.length}
          <section aria-label="Project skill groups" class="space-y-1 text-sm">
            <h3 class="font-medium">From {project?.name}</h3>
            {#each inherited as id}
              <p class="break-words text-muted-foreground">{chat.groups.items.find(g => g.id === id)?.name ?? 'Deleted group'}</p>
            {/each}
          </section>
        {/if}
        <fieldset class="space-y-2 text-sm" disabled={saving || loading || chat.skills.loading}>
          <legend class="mb-2 font-medium">Individual skills</legend>
          {#each chat.skills.items as skill (skill.id)}
            <label class="flex items-center gap-2">
              <input type="checkbox" value={skill.id} bind:group={draft.skillIds} aria-label={skill.name}
                disabled={!draft.skillIds.includes(skill.id) && (!skill.active || draft.skillIds.length >= 8)} />
              <span class="min-w-0 break-words">{skill.name}</span>
              {#if !skill.active}<span class="text-xs text-muted-foreground">Inactive</span>{/if}
            </label>
          {/each}
          {#each draft.skillIds.filter(id => !chat.skills.items.some(s => s.id === id)) as id}
            <label class="flex items-center gap-2 text-muted-foreground"><input type="checkbox" value={id}
              bind:group={draft.skillIds} />Deleted skill</label>
          {/each}
          {#if !chat.skills.items.length && !draft.skillIds.length}<p class="text-muted-foreground">No skills</p>{/if}
          {#if chat.skills.error}<p role="alert" class="text-destructive">{chat.skills.error}</p>{/if}
        </fieldset>
        {#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
        <div class="flex items-center justify-between gap-3">
          <button type="button" class="icon-button" aria-label="Reload selection" disabled={saving || loading} onclick={load}>
            <RefreshCw size={16} aria-hidden="true" />
          </button>
          <Button type="submit" 
      disabled={!chat.ready || saving || loading || !loaded || !!chat.groups.error || !!chat.skills.error}>
            <Save size={16} aria-hidden="true" />Save selection
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
