<script lang="ts">
  // Identifies groups alongside their selection checkboxes.
  import Folder from '@lucide/svelte/icons/folder';
  // Reads group names and activation states.
  import type { SkillGroups } from './skill-groups.svelte';

  let { registry, selected = $bindable([]), disabled = false, label = 'Skill groups', onchange = () => {} }:
    { registry: SkillGroups; selected?: string[]; disabled?: boolean; label?: string; onchange?: () => void } = $props();
</script>

<fieldset class="space-y-2 text-sm" disabled={disabled || registry.loading}>
  <legend class="mb-2 font-medium">{label}</legend>
  {#each registry.items as group (group.id)}
    <label class="flex items-center gap-2">
      <input type="checkbox" value={group.id} bind:group={selected} aria-label={group.name}
        disabled={!selected.includes(group.id) && selected.length >= 16} {onchange} />
      <Folder size={15} class="shrink-0 text-muted-foreground" aria-hidden="true" />
      <span class="min-w-0 break-words">{group.name}</span>
      {#if !group.active}<span class="text-xs text-muted-foreground">Inactive</span>{/if}
    </label>
  {/each}
  {#each selected.filter(id => !registry.items.some(g => g.id === id)) as id}
    <label class="flex items-center gap-2 text-muted-foreground"><input type="checkbox" value={id}
      bind:group={selected} {onchange} />Deleted group</label>
  {/each}
  {#if registry.error}<p role="alert" class="text-destructive">{registry.error}</p>
  {:else if !registry.items.length && !selected.length}<p class="text-muted-foreground">No groups</p>{/if}
</fieldset>
