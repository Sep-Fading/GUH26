<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Styled text fields from the shared component library.
  import { Input } from '$lib/components/ui/input/index.js';
  // Loads this panel's data when Settings shows it.
  import { onMount } from 'svelte';
  // Keeps modal focus and tooltips accessible.
  // Identifies skill management and editing commands.
  import BookOpen from '@lucide/svelte/icons/book-open';
  import Plus from '@lucide/svelte/icons/plus';
  import Save from '@lucide/svelte/icons/save';
  import Download from '@lucide/svelte/icons/download';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import X from '@lucide/svelte/icons/x';
  import Folder from '@lucide/svelte/icons/folder';
  // Edits group membership inside this manager.
  import GroupEditor from './GroupEditor.svelte';
  // Tracks the selected group's observed revision.
  import type { SkillGroup } from './skill-groups.svelte';
  // Uses the existing accessible selection control.
  import Choice from './Choice.svelte';
  // Shares exact-source Markdown editing with agents.
  import Instructions from './Instructions.svelte';
  // Confirms deletion of MiniClaw-owned skill copies.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Types owned documents and local form drafts.
  import type { Skill, SkillDraft } from './skill-state.svelte';
  // Uses the current account's skill registry.
  import type { Chat } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
  const registry = $derived(chat.skills);
  let open = $state(false);
  let id = $state<string>();
  let draft = $state<SkillDraft>(empty());
  let saved = $state(false);
  let importing = $state(false);
  let source = $state('');
  // Both providers keep skills as folders with a SKILL.md, so either can be the source.
  let provider = $state<'codex' | 'claude'>('codex');
  let grouping = $state(false);
  let group = $state<SkillGroup>();

  // Returns an inactive instruction document with no source dependency.
  function empty(): SkillDraft {
    return { name: '', description: '', instructions: '', active: false };
  }

  // The command name the backend derives from a skill's name: lowercase, other characters as one dash.
  const slug = (name: string) => name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40);

  // Copies one saved revision into the editor without modifying the registry.
  function edit(skill?: Skill) {
    grouping = false;
    id = skill?.id;
    if (skill) {
      const { id: _id, ...definition } = skill;
      draft = { ...definition };
    } else draft = empty();
    saved = false; registry.error = ''; proposed = '';
  }

  // Refreshes owned copies on opening; import discovery requires its own action.
  function opened(value: boolean) {
    if (chat.groups.saving || registry.saving) return;
    open = value;
    if (value) { edit(); importing = false; source = ''; void registry.refresh(); void chat.groups.refresh(); }
  }

  // Opens a group revision without changing individual skill settings.
  function editGroup(value?: SkillGroup) {
    grouping = true; group = value; chat.groups.error = '';
  }

  // Saves the current source and records its acknowledged revision.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    const skill = await registry.save(id, $state.snapshot(draft));
    if (skill) { edit(skill); saved = true; }
  }

  // Lists installed source names without importing or activating them.
  function discover(from = provider) {
    provider = from; importing = true; source = ''; void registry.discover(from);
  }

  // Creates an inactive copy of the selected source and opens it for review.
  async function importSkill() {
    const skill = await registry.import(source, provider);
    if (skill) { edit(skill); importing = false; saved = true; }
  }

  // A skill an agent wrote opens as a draft once the list is loaded. With `replaces` it opens the
  // named skill with the new text, keeping its revision and whether it is active; otherwise it is
  // a new, inactive skill. Nothing is saved from here.
  let mounted = $state(false), proposed = $state('');
  $effect(() => {
    const proposal = chat.skillProposal;
    if (!proposal || !mounted || registry.loading) return;
    chat.skillProposal = null;
    const old = proposal.replaces ? registry.items.find(skill => skill.name.toLowerCase() === proposal.replaces!.toLowerCase()) : undefined;
    edit(old);
    draft = { ...draft, name: old ? old.name : proposal.name, description: proposal.description, instructions: proposal.instructions,
      slash: proposal.slash };
    importing = false;
    proposed = old ? `This is a new version of ${old.name}, written by the agent. Saving replaces the current text.`
      : proposal.replaces ? `The agent meant to replace "${proposal.replaces}", which does not exist, so this is a new skill.`
      : 'Written by the agent. Review it, then save.';
  });

  onMount(() => { opened(true); mounted = true; });
</script>

<section aria-label="Skills" class="flex min-h-0 flex-1 flex-col">
  <h2 class="flex h-12 shrink-0 items-center border-b border-border px-5 text-base font-semibold">Skills</h2>
  <div class="grid min-h-0 flex-1 grid-cols-1 overflow-y-auto sm:grid-cols-[180px_minmax(0,1fr)]">
    <nav class="border-b border-border p-3 sm:border-b-0 sm:border-r" aria-label="Skill copies">
      <button class="flex min-h-9 items-center gap-2 px-2 text-sm text-brand"
        disabled={registry.saving || chat.groups.saving} onclick={() => editGroup()}>
        <Plus size={16} aria-hidden="true" />New group</button>
      <div class="max-h-36 overflow-y-auto border-b border-border pb-2">
        {#each chat.groups.items as entry (entry.id)}
          <button class="flex min-h-9 w-full items-center gap-2 rounded px-2 py-2 text-left text-sm hover:bg-muted"
            class:bg-muted={grouping && group?.id === entry.id} disabled={registry.saving || chat.groups.saving}
            onclick={() => editGroup(entry)}>
            <Folder size={15} class="shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="min-w-0 flex-1 truncate">{entry.name}</span>
            <span class="size-1.5 shrink-0 rounded-full bg-border" class:bg-success={entry.active} aria-hidden="true"></span>
          </button>
        {/each}
      </div>
      <button class="flex min-h-9 items-center gap-2 px-2 text-sm text-brand"
        disabled={registry.saving || chat.groups.saving} onclick={() => edit()}><Plus size={16} aria-hidden="true" />New skill</button>
      <button class="flex min-h-9 items-center gap-2 px-2 text-left text-sm text-brand"
        disabled={registry.saving || registry.discovering || chat.groups.saving} onclick={() => { edit(); discover(); }}>
        <Download size={16} class="shrink-0" aria-hidden="true" />Import skills
      </button>
      <div class="max-h-28 overflow-y-auto sm:max-h-none">
        {#each registry.items as skill (skill.id)}
          <button class="flex min-h-9 w-full items-center gap-2 rounded px-2 py-2 text-left text-sm hover:bg-muted"
            class:bg-muted={!grouping && id === skill.id} disabled={registry.saving || chat.groups.saving} onclick={() => edit(skill)}>
            <span class="min-w-0 flex-1 truncate">{skill.name}</span>
            <span class="size-1.5 shrink-0 rounded-full bg-border" class:bg-success={skill.active} aria-hidden="true"></span>
          </button>
        {/each}
      </div>
    </nav>
    {#if grouping}
      <GroupEditor {chat} {group} onselect={editGroup} />
    {:else}
    <form class="min-w-0 space-y-4 p-5" onsubmit={save}>
      {#if proposed}<p role="note" class="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs">{proposed}</p>{/if}
      {#if importing}
        <section aria-label="Import skills" class="space-y-3 border-b border-border pb-4">
          <div role="radiogroup" aria-label="Import from" class="flex w-fit gap-1 rounded-lg bg-muted p-1 text-sm">
            {#each [['codex', 'Codex'], ['claude', 'Claude Code']] as [value, label] (value)}
              <button type="button" role="radio" aria-checked={provider === value} class="rounded-md px-3 py-1"
                class:bg-background={provider === value} class:shadow-sm={provider === value}
                disabled={registry.discovering || registry.saving} onclick={() => discover(value as typeof provider)}>{label}</button>
            {/each}
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <Choice label="Skill to import" value={source} placeholder="Select a skill"
              options={registry.sources.map(s => ({ value: s.id, label: s.name }))}
              disabled={registry.discovering || registry.saving} onchange={value => source = value} />
            <button type="button" class="flex items-center gap-2 text-sm text-brand disabled:opacity-40"
              disabled={!source || registry.saving || registry.discovering} onclick={importSkill}>
              <Download size={16} aria-hidden="true" />Import skill
            </button>
          </div>
          <p class="text-xs text-muted-foreground">Reads the skills installed for {provider === 'codex' ? 'Codex' : 'Claude Code'} on {words.machine}.
            SKILL.md only; scripts and supporting files are excluded, and the original stays unchanged. An imported skill works in
            conversations of either provider.</p>
          {#if registry.discovering}<p class="text-xs text-muted-foreground">Loading sources...</p>
          {:else if !registry.sources.length}<p class="text-xs text-muted-foreground">No importable skills found.</p>{/if}
        </section>
      {/if}
      <label class="block text-sm">Name
        <Input class="mt-1" bind:value={draft.name} maxlength={80} required
          disabled={registry.saving} oninput={() => saved = false} />
      </label>
      <label class="block text-sm">Description
        <Input class="mt-1" bind:value={draft.description} maxlength={500}
          disabled={registry.saving} oninput={() => saved = false} />
      </label>
      <Instructions bind:value={draft.instructions} disabled={registry.saving} onchange={() => saved = false} />
      <label class="flex items-center gap-2 text-sm">
        <input type="checkbox" bind:checked={draft.active} disabled={registry.saving}
          onchange={() => saved = false} />Active
      </label>
      <label class="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={draft.slash ?? false} disabled={registry.saving}
          onchange={event => { draft.slash = event.currentTarget.checked; saved = false; }} />Callable with a slash
      </label>
      {#if draft.slash}
        <p class="text-xs text-muted-foreground">While it is active, type <span class="font-mono">/{slug(draft.name) || 'its-name'}</span>
          in any conversation to use it for that one message, without attaching it.</p>
      {/if}
      {#if registry.error}
        <p role="alert" class="break-words text-sm text-destructive">{registry.error}</p>
      {/if}
      <div class="flex flex-wrap items-center gap-2">
        {#if id && draft.revision}
          <ConfirmDelete label="Delete skill" actionLabel="Delete skill" triggerText="Delete"
            disabled={registry.saving}
            description={`Delete ${draft.name}? Chats using this skill cannot continue. Their pinned instructions and the installed source remain.`}
            action={async () => { await registry.remove(id!, draft.revision!); edit(); }} />
        {/if}
        <button type="button" class="icon-button" aria-label="Reload skills"
          disabled={registry.saving || registry.loading} onclick={() => registry.refresh()}>
          <RefreshCw size={16} aria-hidden="true" />
        </button>
        <span role="status" class="min-w-0 flex-1 text-xs text-success">{saved ? 'Saved' : ''}</span>
        <Button type="submit" 
     disabled={registry.saving || !chat.ready || !draft.name.trim() || !draft.instructions.trim()}>
          <Save size={16} aria-hidden="true" />Save skill
        </Button>
      </div>
    </form>
    {/if}
  </div>
</section>
