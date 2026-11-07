<script lang="ts">
  // Restores the reading preference after mounting.
  import { onMount } from 'svelte';

  let { width = $bindable(), actual }: { width?: number; actual: number } = $props();
  let viewport = $state(1440);
  let drag = $state<{ id: number; x: number; width: number } | null>(null);
  const maximum = $derived(Math.max(200, Math.min(560, viewport - 420)));
  const storageKey = 'miniclaw.navigation.width';

  onMount(() => {
    try {
      const saved = Number(localStorage.getItem(storageKey));
      if (Number.isFinite(saved) && saved >= 200 && saved <= 560) width = saved;
    } catch { /* Browser storage is optional. */ }
  });

  // Saves only completed gestures, avoiding storage work during pointer movement.
  function save() {
    try {
      if (width === undefined) localStorage.removeItem(storageKey);
      else localStorage.setItem(storageKey, String(width));
    } catch { /* Resizing still works without browser storage. */ }
  }

  // Keeps the preferred width within the current window's useful range.
  function resize(next: number) { width = Math.max(200, Math.min(maximum, Math.round(next))); }

  // Captures the pointer so a fast drag continues outside the narrow handle.
  function start(event: PointerEvent) {
    if (event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.focus();
    handle.setPointerCapture(event.pointerId);
    drag = { id: event.pointerId, x: event.clientX, width: actual };
  }

  // Measures movement from the initial edge without jumping to the pointer centre.
  function move(event: PointerEvent) {
    if (drag?.id === event.pointerId) resize(drag.width + event.clientX - drag.x);
  }

  // Ends normal, cancelled and lost-capture gestures and remembers the chosen width.
  function stop(event: PointerEvent) {
    if (drag?.id !== event.pointerId) return;
    drag = null;
    const handle = event.currentTarget as HTMLElement;
    if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    save();
  }

  // Restores the normal or compact layout's default sidebar width.
  function reset() { width = undefined; save(); }

  // Moves the splitter with arrows or jumps to its minimum and maximum widths.
  function keydown(event: KeyboardEvent) {
    const next = { ArrowLeft: actual - 16, ArrowRight: actual + 16, Home: 200, End: maximum }[event.key];
    if (next === undefined) return;
    event.preventDefault(); resize(next); save();
  }
</script>

<svelte:window bind:innerWidth={viewport} />
<!-- A focusable separator is the interactive ARIA splitter widget. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div role="separator" aria-label="Resize sidebar" aria-orientation="vertical"
  aria-controls="task-navigation" aria-valuenow={Math.round(actual)} aria-valuemin={200} aria-valuemax={maximum}
  aria-valuetext={`${Math.round(actual)} pixels`} tabindex="0"
  title="Drag to resize; double-click to reset" class="sidebar-resize" class:dragging={drag !== null}
  onpointerdown={start} onpointermove={move} onpointerup={stop} onpointercancel={stop}
  onlostpointercapture={stop} onkeydown={keydown} ondblclick={reset}></div>

<style>
  .sidebar-resize { position: absolute; inset: 0 -4px 0 auto; width: 8px; z-index: 10;
    cursor: col-resize; touch-action: none; user-select: none; }
  .sidebar-resize::after { content: ''; position: absolute; inset: 0 3px; border-radius: 2px; }
  .sidebar-resize:hover::after, .sidebar-resize:focus-visible::after, .sidebar-resize.dragging::after {
    background: var(--ring);
  }
  .sidebar-resize:focus-visible { outline: 2px solid var(--ring); outline-offset: -2px; }
</style>
