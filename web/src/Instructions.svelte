<script lang="ts">
  // Mounts the editor and releases listeners.
  import { onMount } from 'svelte';
  // Types the lazily loaded editing surface.
  import type { mountEditor } from './instruction-editor';

  let { value = $bindable(''), disabled = false, onchange = () => {}, label = 'Instructions' }:
    { value?: string; disabled?: boolean; onchange?: () => void; label?: string } = $props();
  const id = $props.id();
  let host: HTMLDivElement;
  let editor = $state.raw<ReturnType<typeof mountEditor>>();
  let error = $state('');

  // Updates the bound source without replacing the editor's selection or history.
  function changed(source: string) {
    if (source !== value) { value = source; onchange(); }
  }

  onMount(() => {
    let closed = false;
    void import('./instruction-editor').then(({ mountEditor }) => {
      if (!closed) editor = mountEditor(host, value, id, disabled, changed);
    }).catch(() => { if (!closed) error = 'Could not load the editor. Reopen this dialog to retry.'; });
    return () => { closed = true; editor?.destroy(); };
  });

  $effect(() => { editor?.setSource(value, disabled); });
  $effect(() => { editor?.setDisabled(disabled); });
</script>

<div class="min-w-0">
  <div {id} class="mb-1 text-sm">{label}</div>
  <div bind:this={host} class="instruction-editor rounded-md border border-border"
    class:opacity-60={disabled}></div>
  {#if error}<p role="alert" class="mt-1 text-xs text-destructive">{error}</p>{/if}
</div>

<style>
  .instruction-editor :global(.cm-editor) { height: 12rem; font-size: .875rem; }
  .instruction-editor :global(.cm-scroller) { overflow: auto; font-family: inherit; }
  .instruction-editor :global(.cm-content) { min-height: 100%; padding: .75rem; overflow-wrap: anywhere; }
  .instruction-editor :global(.cm-line) { padding: 0; line-height: 1.6; }
  .instruction-editor :global(.cm-focused) { outline: 2px solid var(--ring); outline-offset: 2px; }
  .instruction-editor :global(em) { font-style: italic; }
  .instruction-editor :global(strong) { font-weight: 600; }
  .instruction-editor :global(code) { font-family: monospace; background: var(--muted); }
  .instruction-editor :global(.instruction-heading) { font-size: 1rem; font-weight: 600; }
  .instruction-editor :global(.instruction-code) { background: var(--muted); }
  .instruction-editor :global(.instruction-list) { padding-left: .5rem; }
  .instruction-editor :global(.instruction-quote) { border-left: 2px solid var(--border); padding-left: .75rem; }
</style>
