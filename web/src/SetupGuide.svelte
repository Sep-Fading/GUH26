<script lang="ts">
  // The same numbered guides setup and `miniclaw guide` print.
  import { guide } from '../../scripts/setup/guides.mjs';
  // Picks the Mac or Windows steps for the machine MiniClaw runs on.
  import { host } from './platform.svelte';
  // Marks the disclosure as help.
  import CircleHelp from '@lucide/svelte/icons/circle-help';

  // `topic` names a guide in scripts/setup/guides.mjs; `label` is the button's visible and accessible name.
  let { topic, label }: { topic: string; label: string } = $props();
  let open = $state(false);
  const id = $props.id();
  const item = $derived(guide(topic, { platform: host.os === 'windows' ? 'win32' : 'darwin' }));
</script>

<button type="button" class="inline-flex items-center gap-1 text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
  aria-expanded={open} aria-controls={`guide-${id}`} onclick={() => { open = !open; }}>
  <CircleHelp size={12} aria-hidden="true" />{label}</button>
{#if open}
  <section id={`guide-${id}`} aria-label={item.title} class="mt-1 rounded-md border border-border bg-muted/40 p-3 text-xs">
    <p class="font-medium">{item.title}</p>
    <ol class="mt-2 list-decimal space-y-1.5 pl-5">
      {#each item.steps as step, index (index)}
        {@const [prose, ...commands] = step.split('\n')}
        <li>{prose}
          {#each commands as command, line (line)}<code class="mt-1 block overflow-x-auto whitespace-pre rounded bg-background px-2 py-1 font-mono">{command}</code>{/each}
        </li>
      {/each}
    </ol>
    <p class="mt-2 text-muted-foreground">Source:
      {#each item.sources as source (source)}<a class="ml-1 break-all underline" href={source} target="_blank" rel="noreferrer">{source}</a>{/each}</p>
  </section>
{/if}
