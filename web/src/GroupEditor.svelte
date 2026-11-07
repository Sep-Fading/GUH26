<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Styled text fields from the shared component library.
  import { Input } from '$lib/components/ui/input/index.js';
  // Identifies saving and reloading group settings.
  import Save from '@lucide/svelte/icons/save';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Reuses confirmation styling and deletion errors.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Reads owned groups and their member skills.
  import type { Chat } from './chat.svelte';
  // Keeps editable membership separate from registry state.
  import type { GroupDraft, SkillGroup } from './skill-groups.svelte';

  let { chat, group, onselect }: { chat: Chat; group?: SkillGroup;
    onselect: (group?: SkillGroup) => void } = $props();
  let draft = $state<GroupDraft>({ name: '', active: false, members: [] });
  let saved = $state(false);
  $effect(() => { draft = group ? { name: group.name, active: group.active,
    revision: group.revision, members: group.members.map(m => ({ ...m })) }
    : { name: '', active: false, members: [] }; });

  // Adds or removes membership without touching the skill's individual activation.
  function include(skillId: string, checked: boolean) {
    const others = draft.members.filter(m => m.skillId !== skillId);
    draft.members = checked ? [...others, { skillId, enabled: true }] : others;
    saved = false;
  }

  // Saves the current group and switches the editor to its acknowledged revision.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    const result = await chat.groups.save(group?.id, $state.snapshot(draft));
    if (result) { onselect(result); saved = true; }
  }

  // Reloads the observed group after a conflict, replacing its unsaved draft explicitly.
  async function reload() {
    await chat.groups.refresh();
    const current = chat.groups.items.find(g => g.id === group?.id);
    if (!chat.groups.error) { onselect(current); saved = false; }
  }
</script>

<form class="min-w-0 space-y-4 p-5" onsubmit={save}>
  <label class="block text-sm">Group name
    <Input class="mt-1" bind:value={draft.name} maxlength={80} required
      disabled={chat.groups.saving} oninput={() => saved = false} />
  </label>
  <label class="flex items-center gap-2 text-sm">
    <input type="checkbox" aria-label="Group active" bind:checked={draft.active}
      disabled={chat.groups.saving} onchange={() => saved = false} />Active
  </label>
  <fieldset disabled={chat.groups.saving || chat.skills.loading}>
    <legend class="mb-2 text-sm font-medium">Skills</legend>
    <div class="flex justify-between border-b border-border pb-2 text-xs text-muted-foreground">
      <span>Skill</span><span>Enabled</span>
    </div>
    {#each chat.skills.items as skill (skill.id)}
      {@const member = draft.members.find(m => m.skillId === skill.id)}
      <!-- One box: ticked puts the skill in the group and on; unticked takes it out. -->
      <label class="flex min-h-10 items-center justify-between gap-3 border-b border-border/50 py-2 text-sm">
        <span class="min-w-0 break-words">{skill.name}</span>
        <input type="checkbox" class="shrink-0" aria-label={`Enable ${skill.name} in this group`}
          checked={member?.enabled ?? false} onchange={event => include(skill.id, event.currentTarget.checked)} />
      </label>
    {/each}
    {#each draft.members.filter(m => !chat.skills.items.some(s => s.id === m.skillId)) as member}
      <label class="flex items-center gap-2 py-2 text-sm text-muted-foreground"><input type="checkbox" checked
        onchange={() => include(member.skillId, false)} />Deleted skill</label>
    {/each}
    {#if !chat.skills.items.length}<p class="py-3 text-sm text-muted-foreground">No skills</p>{/if}
  </fieldset>
  {#if chat.groups.error || chat.skills.error}
    <p role="alert" class="break-words text-sm text-destructive">{chat.groups.error || chat.skills.error}</p>
  {/if}
  <div class="flex flex-wrap items-center gap-2">
    {#if group}
      <ConfirmDelete label="Delete group" actionLabel="Delete group" triggerText="Delete"
        disabled={chat.groups.saving}
        description={`Delete ${group.name}? Its skills remain. Conversations stop using this group on their next message.`}
        action={async () => { await chat.groups.remove(group!.id, group!.revision); onselect(); }} />
    {/if}
    <button type="button" class="icon-button" aria-label="Reload group"
      disabled={chat.groups.loading || chat.groups.saving} onclick={reload}><RefreshCw size={16} aria-hidden="true" /></button>
    <span role="status" class="min-w-0 flex-1 text-xs text-success">{saved ? 'Saved' : ''}</span>
    <Button type="submit" 
   disabled={!chat.ready || chat.groups.saving || !draft.name.trim()}>
      <Save size={16} aria-hidden="true" />Save group
    </Button>
  </div>
</form>
