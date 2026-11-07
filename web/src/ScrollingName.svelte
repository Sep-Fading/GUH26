<script lang="ts">
  let { text }: { text: string } = $props();
  let available = $state(0);
  let fullWidth = $state(0);
  const distance = $derived(Math.max(0, fullWidth - available));
</script>

<span class="name" class:overflowing={distance > 1} bind:clientWidth={available}
  style:--name-shift={`${-distance}px`} style:--name-duration={`${Math.max(3, distance / 40 + 1)}s`}>
  <span class="rest">{text}</span>
  <span class="moving" aria-hidden="true" bind:offsetWidth={fullWidth}>{text}</span>
</span>

<style>
  .name { position: relative; display: block; flex: 1; min-width: 0; overflow: hidden; }
  .rest { display: block; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .moving { position: absolute; inset-block-start: 0; inset-inline-start: 0;
    width: max-content; white-space: nowrap; opacity: 0; pointer-events: none; }

  :global(button:is(:hover, :focus-visible)) .overflowing .rest { opacity: 0; }
  :global(button:is(:hover, :focus-visible)) .overflowing .moving {
    opacity: 1;
    animation: reveal-name var(--name-duration) linear infinite alternate;
  }

  @keyframes reveal-name {
    0%, 5% { transform: translateX(0); }
    95%, 100% { transform: translateX(var(--name-shift)); }
  }

  @media (prefers-reduced-motion: reduce) {
    :global(button:is(:hover, :focus-visible)) .overflowing .moving { opacity: 0; animation: none; }
    :global(button:is(:hover, :focus-visible)) .overflowing .rest {
      opacity: 1; white-space: normal; overflow-wrap: anywhere;
    }
  }
</style>
