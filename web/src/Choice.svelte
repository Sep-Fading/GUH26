<script lang="ts">
  // Provides keyboard and focus handling for dropdowns.
  import { Select } from 'bits-ui';
  // Marks dropdown controls and selected options.
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Check from '@lucide/svelte/icons/check';
  // Lets a caller put an icon before the chosen text.
  import type { Component } from 'svelte';
  const listId = $props.id();

  let { label, value, options, disabled = false, placeholder, onchange, icon: Icon, quiet = false }:
    // An option's `name` is the short form the closed control shows; the list shows its label.
    // `icon` goes before the chosen text; `quiet` hides that text on a phone, leaving the icon.
    { label: string; value: string; options: { value: string; label: string; name?: string }[];
      disabled?: boolean; placeholder: string; onchange: (value: string) => void;
      icon?: Component<{ size?: number; class?: string; 'aria-hidden'?: 'true' }>; quiet?: boolean } = $props();
  const chosen = $derived(options.find(option => option.value === value));
</script>

<Select.Root type="single" {value} {disabled} items={options} onValueChange={onchange}>
  <Select.Trigger role="combobox" aria-label={label} aria-controls={listId}
    class="flex h-9 min-w-0 w-auto max-w-56 items-center gap-2 rounded-md px-2 text-xs
      text-muted-foreground hover:bg-border disabled:opacity-50">
    {#if Icon}<Icon size={13} class="shrink-0" aria-hidden="true" />{/if}
    <span class={['min-w-0 flex-1 truncate text-left', quiet && 'hidden sm:inline']}>
      {chosen?.name ?? chosen?.label ?? placeholder}
    </span>
    <ChevronDown size={13} class="shrink-0" aria-hidden="true" />
  </Select.Trigger>
  <Select.Portal>
    <Select.Content id={listId} side="top" align="start" sideOffset={6}
      class="z-50 max-h-[min(320px,var(--bits-select-content-available-height))]
        w-64 max-w-[calc(100vw-32px)] overflow-y-auto rounded-md border border-border
        bg-background p-1 shadow-md">
      <Select.Viewport>
        {#each options as option (option.value)}
          <Select.Item value={option.value} label={option.label}
            class="flex min-h-9 items-center gap-2 rounded px-3 py-2 text-sm
              outline-none data-highlighted:bg-muted">
            {#snippet children({ selected })}
              <span class="min-w-0 flex-1 [overflow-wrap:anywhere]">{option.label}</span>
              {#if selected}<Check size={15} class="shrink-0 text-brand" aria-hidden="true" />{/if}
            {/snippet}
          </Select.Item>
        {/each}
      </Select.Viewport>
    </Select.Content>
  </Select.Portal>
</Select.Root>
