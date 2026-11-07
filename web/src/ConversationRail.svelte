<script lang="ts">
  // Waits for rendered message anchors before measuring.
  import { tick } from 'svelte';
  // Positions accessible previews beside each exchange marker.
  import { Tooltip } from 'bits-ui';
  // Groups messages and renders bounded Markdown previews.
  import { exchanges, previewMarkdown } from './conversation-index';
  // Reuses reactive messages without owning chat state.
  import type { Chat } from './chat.svelte';

  let { messages, viewport, onjump }: {
    messages: Chat['messages']; viewport: HTMLElement | undefined; onjump: () => void;
  } = $props();
  const entries = $derived(exchanges(messages));
  const signature = $derived(entries.map(entry => entry.id).join('\0'));
  let current = $state(''), opened = $state('');
  let hovered = $state(-1);
  let rail = $state<HTMLElement>();

  // Keeps the reading position visible in long rails without disturbing pointer browsing.
  $effect(() => {
    const id = current;
    if (!rail || hovered >= 0) return;
    void tick().then(() => {
      if (!rail || current !== id || hovered >= 0) return;
      const mark = rail.querySelector<HTMLElement>('[aria-current="location"]');
      if (!mark) return;
      const bounds = rail.getBoundingClientRect();
      const item = mark.getBoundingClientRect();
      if (item.top < bounds.top) rail.scrollTop += item.top - bounds.top - 4;
      else if (item.bottom > bounds.bottom) rail.scrollTop += item.bottom - bounds.bottom + 4;
    });
  });

  // Tracks the visible exchange after scrolling, streaming, history loads and resizing.
  $effect(() => {
    signature;
    const root = viewport;
    if (!root || !signature) { current = ''; return; }
    let disposed = false, frame = 0;
    let observer: ResizeObserver | undefined;
    let anchors: HTMLElement[] = [];

    // Marks the last prompt above the reading line, or the last at the bottom.
    function measure() {
      frame = 0;
      const line = root!.getBoundingClientRect().top + Math.min(96, root!.clientHeight / 4);
      let active: HTMLElement | undefined = anchors[0];
      for (const anchor of anchors) {
        if (anchor.getBoundingClientRect().top <= line) active = anchor;
        else break;
      }
      if (root!.scrollHeight - root!.scrollTop - root!.clientHeight < 2) active = anchors.at(-1);
      current = active?.dataset.promptId ?? '';
    }

    // Coalesces scroll and resize notifications into one measurement per frame.
    function schedule() {
      if (!frame) frame = requestAnimationFrame(measure);
    }

    void tick().then(() => {
      if (disposed) return;
      anchors = [...root.querySelectorAll<HTMLElement>('[data-prompt-id]')];
      root.addEventListener('scroll', schedule, { passive: true });
      observer = new ResizeObserver(schedule);
      observer.observe(root);
      const content = root.querySelector('[data-conversation-content]');
      if (content) observer.observe(content);
      measure();
    });
    return () => {
      disposed = true;
      root.removeEventListener('scroll', schedule);
      observer?.disconnect();
      cancelAnimationFrame(frame);
    };
  });

  // Scrolls the conversation to a prompt without moving the composer or changing focus.
  function jump(id: string) {
    if (!viewport) return;
    const target = [...viewport.querySelectorAll<HTMLElement>('[data-prompt-id]')]
      .find(node => node.dataset.promptId === id);
    if (!target) return;
    const offset = viewport.scrollTop + target.getBoundingClientRect().top
      - viewport.getBoundingClientRect().top - 24;
    const top = Math.max(0, Math.min(offset, viewport.scrollHeight - viewport.clientHeight));
    if (Math.abs(top - viewport.scrollTop) < 1) return;
    onjump();
    viewport.scrollTo({ top, behavior: matchMedia('(prefers-reduced-motion: reduce)').matches
      ? 'instant' : 'smooth' });
  }

  // Moves keyboard focus through marks without scrolling the conversation until activation.
  function navigate(event: KeyboardEvent, index: number) {
    let next: number;
    switch (event.key) {
      case 'ArrowDown': next = Math.min(index + 1, entries.length - 1); break;
      case 'ArrowUp': next = Math.max(index - 1, 0); break;
      case 'Home': next = 0; break;
      case 'End': next = entries.length - 1; break;
      default: return;
    }
    event.preventDefault();
    rail?.querySelectorAll<HTMLButtonElement>('button')[next]?.focus();
  }

  // Expands the hovered mark and its neighbors while leaving button dimensions fixed.
  function width(index: number) {
    if (hovered < 0) return entries[index].id === current ? 16 : 8;
    return [28, 22, 16, 11][Math.abs(index - hovered)] ?? 8;
  }
</script>

{#if entries.length}
  <nav aria-label="Conversation timeline" bind:this={rail}
    class="conversation-rail" onpointerleave={() => hovered = -1}>
    {#each entries as entry, index (entry.id)}
      <Tooltip.Root delayDuration={120} onOpenChange={value => {
        if (value) opened = entry.id;
        else if (opened === entry.id) opened = '';
      }}>
        <Tooltip.Trigger class="rail-mark" aria-label={`Jump to prompt ${index + 1}: ${entry.prompt}`}
          aria-current={current === entry.id ? 'location' : undefined}
          tabindex={current === entry.id || (!current && index === 0) ? 0 : -1}
          onpointerenter={() => hovered = index} onfocus={() => hovered = index}
          onblur={() => hovered = -1} onkeydown={event => navigate(event, index)}
          onclick={() => jump(entry.id)}>
          <span aria-hidden="true" style:width={`${width(index)}px`}></span>
        </Tooltip.Trigger>
        <Tooltip.Portal>
          <Tooltip.Content role="tooltip" side="right" sideOffset={10} collisionPadding={12}
            class="rail-preview z-20 w-[min(320px,calc(100vw-24px))] rounded-md border
              border-border bg-background p-3 text-sm text-foreground shadow-lg">
            {#if opened === entry.id}
              <strong class="mb-2 block [overflow-wrap:anywhere]">{entry.prompt}</strong>
              <div class="response-preview">
                {#if entry.response}{@html previewMarkdown(entry.response)}{:else}No response yet{/if}
              </div>
            {/if}
          </Tooltip.Content>
        </Tooltip.Portal>
      </Tooltip.Root>
    {/each}
  </nav>
{/if}

<style>
  /* The rail lies over the left edge instead of taking width, so the messages stay centred on
     the same line as the composer below them. */
  .conversation-rail {
    position: absolute;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    z-index: 5;
    width: 40px;
    max-height: calc(100% - 32px);
    overflow-y: auto;
    overflow-x: hidden;
    padding: 4px 4px 4px 8px;
    scrollbar-width: none;
  }
  .conversation-rail :global(.rail-mark) {
    display: flex;
    align-items: center;
    width: 28px;
    height: 24px;
    background: transparent;
  }
  .conversation-rail :global(.rail-mark span) {
    display: block;
    height: 2px;
    flex-shrink: 0;
    background: color-mix(in srgb, var(--muted-foreground) 45%, transparent);
    transition: width 150ms ease-out, background-color 150ms ease-out;
  }
  .conversation-rail :global(.rail-mark[aria-current] span),
  .conversation-rail :global(.rail-mark:hover span),
  .conversation-rail :global(.rail-mark:focus-visible span) { background: var(--foreground); }
  .conversation-rail :global(.rail-mark:focus-visible) { outline-offset: -2px; }
  :global(.response-preview) {
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
    overflow: hidden;
    max-height: 112px;
    line-height: 1.5;
    font-weight: 400;
    overflow-wrap: anywhere;
    color: var(--muted-foreground);
  }
  :global(.response-preview p + p) { margin-top: 0.5em; }
  :global(.response-preview ul) { list-style: disc; padding-left: 1.2em; }
  :global(.response-preview ol) { list-style: decimal; padding-left: 1.2em; }
  :global(.response-preview code) { font-size: 12px; background: var(--muted); }
  :global(.response-preview pre) { white-space: pre-wrap; }
  :global(.response-preview blockquote) { border-left: 2px solid var(--border); padding-left: 8px; }
  @media (max-width: 639px) {
    .conversation-rail { width: 32px; padding-left: 4px; padding-right: 4px; }
    .conversation-rail :global(.rail-mark) { width: 24px; }
    .conversation-rail :global(.rail-mark span) { max-width: 24px; }
  }
  @media (prefers-reduced-motion: reduce) {
    .conversation-rail :global(.rail-mark span) { transition: none; }
  }
</style>
