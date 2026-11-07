<script lang="ts">
  // The timeline under the Agents page's tiles: each agent's working time across the range, in equal blocks like a
  // loading bar. A block is green when its work finished, amber when some was stopped, red when something failed,
  // and empty when nothing ran. The percentage beside each row is its downtime: the part of its working time that
  // did not finish.
  import { blockState, combine, duration, percent, type Block, type BlockState, type Timeline } from './agent-stats';

  let { timeline, order, days }: {
    timeline: Timeline;
    // The agents to show, in the tiles' order ('' is the main agent's name in the timeline).
    order: { name: string; main: boolean }[];
    days: number;
  } = $props();

  const rows = $derived(order.map(agent => {
    const key = agent.main ? '' : agent.name;
    const row = timeline.agents.find(row => row.name === key);
    return { label: agent.name, row: row ?? { name: key, okMs: 0, stoppedMs: 0, failedMs: 0, downtime: 0, failures: 0,
      blocks: Array.from({ length: timeline.count }, () => ({ ok: 0, stopped: 0, failed: 0, failures: 0 })) } };
  }));
  const all = $derived(combine(rows.map(entry => entry.row), timeline.count));
  const total = $derived(all.okMs + all.stoppedMs + all.failedMs);

  const fill: Record<BlockState, string> = {
    empty: 'bg-muted', ok: 'bg-emerald-500', stopped: 'bg-amber-500', failed: 'bg-destructive',
  };

  // A block's start, worded for the range: hours within a day, dates beyond.
  const when = (index: number) => {
    const at = new Date(timeline.from + index * timeline.width);
    return days === 1 ? at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
      : at.toLocaleDateString(undefined, { day: 'numeric', month: 'short', ...(timeline.width < 86_400_000 ? { hour: '2-digit' } : {}) });
  };
  const words = (block: Block, index: number) => {
    const parts = [block.ok && `${duration(block.ok / 1000)} finished`, block.stopped && `${duration(block.stopped / 1000)} stopped`,
      block.failed && `${duration(block.failed / 1000)} failed`, block.failures && `${block.failures} ${block.failures === 1 ? 'failure' : 'failures'}`]
      .filter(Boolean);
    return `${when(index)}: ${parts.length ? parts.join(', ') : 'nothing ran'}`;
  };
  // Labels under the bars: the start and two points between; "now" sits at the end.
  const ticks = $derived([0, Math.round(timeline.count / 3), Math.round((2 * timeline.count) / 3)]);
</script>

{#snippet bar(blocks: Block[], label: string)}
  <div class="flex h-5 min-w-0 flex-1 gap-[2px]" role="img" aria-label={`${label} timeline`}>
    {#each blocks as block, index (index)}
      <div class={`min-w-0 flex-1 rounded-[2px] ${fill[blockState(block)]}`} title={words(block, index)}></div>
    {/each}
  </div>
{/snippet}

<section aria-label="Timeline" class="rounded-lg border border-border p-4">
  <div class="flex flex-wrap items-baseline justify-between gap-2">
    <h2 class="text-sm font-medium">Timeline</h2>
    <ul class="flex flex-wrap gap-3 text-xs text-muted-foreground" aria-label="Legend">
      <li class="flex items-center gap-1.5"><span class="size-2.5 rounded-[2px] bg-emerald-500" aria-hidden="true"></span>Finished</li>
      <li class="flex items-center gap-1.5"><span class="size-2.5 rounded-[2px] bg-amber-500" aria-hidden="true"></span>Stopped or incomplete</li>
      <li class="flex items-center gap-1.5"><span class="size-2.5 rounded-[2px] bg-destructive" aria-hidden="true"></span>Failed</li>
      <li class="flex items-center gap-1.5"><span class="size-2.5 rounded-[2px] bg-muted" aria-hidden="true"></span>Nothing ran</li>
    </ul>
  </div>
  <p class="mb-3 mt-0.5 text-xs text-muted-foreground">
    Each block is {duration(timeline.width / 1000)} of the range. Downtime is the part of an agent's working time that was
    stopped or failed instead of finishing. Hover a block for its details.
  </p>

  <!-- The whole range as one loading bar: how much of all the agents' working time finished. -->
  {#if total}
    <div class="mb-4">
      <div class="flex h-2.5 overflow-hidden rounded-full bg-muted" role="img"
        aria-label={`${percent(all.okMs / total)} finished, ${percent(all.stoppedMs / total)} stopped, ${percent(all.failedMs / total)} failed`}>
        <div class="bg-emerald-500" style:width={`${(all.okMs / total) * 100}%`}></div>
        <div class="bg-amber-500" style:width={`${(all.stoppedMs / total) * 100}%`}></div>
        <div class="bg-destructive" style:width={`${(all.failedMs / total) * 100}%`}></div>
      </div>
      <p class="mt-1.5 text-xs text-muted-foreground tabular-nums">
        <span class="font-medium text-foreground">{percent(all.downtime)} downtime</span> ·
        {percent(all.okMs / total)} finished, {percent(all.stoppedMs / total)} stopped, {percent(all.failedMs / total)} failed
        {#if all.failures} · {all.failures} {all.failures === 1 ? 'failure' : 'failures'}{/if}
      </p>
    </div>
  {/if}

  <div class="grid grid-cols-[minmax(0,8rem)_minmax(0,1fr)_3.5rem] items-center gap-x-3 gap-y-1.5">
    <span class="truncate text-xs font-medium">{all.name}</span>
    {@render bar(all.blocks, all.name)}
    <span class="text-right text-xs font-medium tabular-nums" aria-label={`${all.name} downtime ${percent(all.downtime)}`}>{percent(all.downtime)}</span>
    <div class="col-span-3 my-0.5 border-t border-border"></div>
    {#each rows as entry (entry.row.name)}
      <span class="truncate text-xs" title={entry.label}>{entry.label}</span>
      {@render bar(entry.row.blocks, entry.label)}
      <span class={`text-right text-xs tabular-nums ${entry.row.downtime > 0 ? 'text-destructive' : 'text-muted-foreground'}`}
        aria-label={`${entry.label} downtime ${percent(entry.row.downtime)}`}>{percent(entry.row.downtime)}</span>
    {/each}
    <span></span>
    <div class="relative h-4 text-[11px] text-muted-foreground">
      {#each ticks as tick (tick)}
        <span class="absolute whitespace-nowrap" style:left={`${(tick / timeline.count) * 100}%`}>{when(tick)}</span>
      {/each}
      <span class="absolute right-0 bg-background pl-1 whitespace-nowrap">now</span>
    </div>
    <span></span>
  </div>
</section>
