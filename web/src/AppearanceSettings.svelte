<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Reads and changes the saved display choices.
  import { appearance, type Appearance } from './appearance.svelte';
  // Styled on/off control from the shared component library.
  import { Switch } from '$lib/components/ui/switch/index.js';

  const themes: { value: Appearance['theme']; label: string }[] = $derived([
    { value: 'system', label: words.matchSystem }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }]);
  const sizes: { value: Appearance['textSize']; label: string }[] = [
    { value: 'small', label: 'Small' }, { value: 'default', label: 'Default' }, { value: 'large', label: 'Large' }];
  const accents: { value: Appearance['accent']; label: string; colour: string }[] = [
    { value: 'blue', label: 'Blue', colour: '#0071e3' }, { value: 'green', label: 'Green', colour: '#1d7a35' },
    { value: 'purple', label: 'Purple', colour: '#7d3fc4' }, { value: 'orange', label: 'Orange', colour: '#b85400' },
    { value: 'pink', label: 'Pink', colour: '#c2255c' }, { value: 'graphite', label: 'Graphite', colour: '#55555a' },
    { value: 'white', label: 'White', colour: '#ffffff' }];
</script>

<section aria-label="Appearance" class="flex min-h-0 flex-1 flex-col overflow-y-auto">
  <h2 class="sticky top-0 z-[5] flex h-12 shrink-0 items-center border-b border-border bg-background px-5 text-base font-semibold">Appearance</h2>
  <div class="space-y-6 p-5 text-sm">
    <fieldset>
      <legend class="font-medium">Theme</legend>
      <div class="mt-2 inline-flex rounded-lg bg-muted p-1" role="radiogroup" aria-label="Theme">
        {#each themes as theme (theme.value)}
          <button type="button" role="radio" aria-checked={appearance.value.theme === theme.value}
            class={`rounded-md px-3 py-1.5 ${appearance.value.theme === theme.value
              ? 'bg-background font-medium shadow-xs' : 'text-muted-foreground hover:text-foreground'}`}
            onclick={() => appearance.set({ theme: theme.value })}>{theme.label}</button>
        {/each}
      </div>
    </fieldset>
    <fieldset>
      <legend class="font-medium">Accent colour</legend>
      <p class="mt-0.5 text-xs text-muted-foreground">Used for your messages, the send button and focus rings.</p>
      <div class="mt-2 flex flex-wrap gap-2" role="radiogroup" aria-label="Accent colour">
        {#each accents as accent (accent.value)}
          <button type="button" role="radio" aria-checked={appearance.value.accent === accent.value}
            aria-label={accent.label} title={accent.label}
            class={`size-7 rounded-full border border-border ring-offset-2 ring-offset-background ${appearance.value.accent === accent.value
              ? 'ring-2 ring-foreground' : 'hover:ring-2 hover:ring-border'}`}
            style:background-color={accent.colour}
            onclick={() => appearance.set({ accent: accent.value })}></button>
        {/each}
      </div>
    </fieldset>
    <fieldset>
      <legend class="font-medium">Text size</legend>
      <div class="mt-2 inline-flex rounded-lg bg-muted p-1" role="radiogroup" aria-label="Text size">
        {#each sizes as size (size.value)}
          <button type="button" role="radio" aria-checked={appearance.value.textSize === size.value}
            class={`rounded-md px-3 py-1.5 ${appearance.value.textSize === size.value
              ? 'bg-background font-medium shadow-xs' : 'text-muted-foreground hover:text-foreground'}`}
            onclick={() => appearance.set({ textSize: size.value })}>{size.label}</button>
        {/each}
      </div>
    </fieldset>
    <div class="flex items-start justify-between gap-3">
      <div>
        <p class="font-medium" id="compact-label">Compact layout</p>
        <p class="mt-0.5 text-xs text-muted-foreground">Tighter spacing in the conversation and a narrower sidebar, for small windows.</p>
      </div>
      <Switch class="mt-0.5 data-[state=checked]:bg-success" aria-label="Compact layout"
        checked={appearance.value.compact} onCheckedChange={compact => void appearance.set({ compact })} />
    </div>
    <p class="text-xs text-muted-foreground">Saved on {words.machine}, so every browser you open MiniClaw in looks the same.</p>
    {#if appearance.error}<p role="alert" class="text-xs text-destructive">{appearance.error}</p>{/if}
  </div>
</section>
