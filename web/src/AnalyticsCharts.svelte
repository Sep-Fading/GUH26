<script lang="ts">
  // Draws the series. This component is fetched only when the Analytics tab is opened, so the
  // drawing library and its d3 packages stay out of MiniClaw's first load.
  import { AreaChart, BarChart, PieChart } from 'layerchart';
  // A day is a band, not a point on a line, so the bars sit under their own labels.
  import { scaleBand } from 'd3-scale';
  import * as Chart from '$lib/components/ui/chart/index.js';

  // The figures and the formatters come from the panel, so both read the same way.
  let { series, split, tokenConfig, turnConfig, splitConfig, short, dateTick, dateLabel }: {
    series: { date: string; at: Date; tokens: number; turns: number; steps: number }[];
    split: { key: string; value: number; color: string }[];
    tokenConfig: Chart.ChartConfig; turnConfig: Chart.ChartConfig; splitConfig: Chart.ChartConfig;
    short: (value: number) => string;
    dateTick: (value: Date | string) => string;
    dateLabel: (value: Date | string) => string;
  } = $props();

  // A band scale would put its first tick on the first bar, under the y-axis's zero. Four dates
  // spaced inside the range keep clear of both edges.
  const bandTicks = $derived(series.length <= 4 ? series.map(day => day.date)
    : [1, 3, 5, 7].map(eighth => series[Math.min(series.length - 1, Math.floor(series.length * eighth / 8))].date));
</script>

        <section aria-label="Tokens a day" class="min-w-0 rounded-lg border border-border p-4 lg:col-span-2">
          <h3 class="mb-3 text-sm font-medium">Tokens a day</h3>
          <Chart.Container config={tokenConfig} class="h-[200px] w-full min-w-0">
            <AreaChart data={series} x="at" y="tokens" axis={true} grid={{ y: true }}
              padding={{ left: 40, bottom: 24, right: 8, top: 8 }}
              series={[{ key: 'tokens', label: 'Tokens', color: 'var(--chart-1)' }]}
              props={{ xAxis: { ticks: 5, format: dateTick }, yAxis: { ticks: 4, format: short },
                area: { line: { class: 'stroke-2' }, 'fill-opacity': 0.18 } }}>
              {#snippet tooltip()}<Chart.Tooltip labelFormatter={dateLabel} />{/snippet}
            </AreaChart>
          </Chart.Container>
        </section>

        <section aria-label="Responses and tool steps a day" class="min-w-0 rounded-lg border border-border p-4">
          <div class="mb-3 flex flex-wrap items-baseline gap-x-4 gap-y-1">
            <h3 class="text-sm font-medium">Responses and tool steps a day</h3>
            <!-- The key sits here rather than under the chart, where LayerChart's own legend
                 would land on the dates. -->
            <ul class="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
              <li class="flex items-center gap-1.5"><span class="size-2 rounded-[2px] bg-chart-1" aria-hidden="true"></span>Responses</li>
              <li class="flex items-center gap-1.5"><span class="size-2 rounded-[2px] bg-chart-3" aria-hidden="true"></span>Tool steps</li>
            </ul>
          </div>
          <Chart.Container config={turnConfig} class="h-[180px] w-full min-w-0">
            <!-- The same left and bottom room as the area chart above, so the two plots line up. -->
            <BarChart data={series} x="date" xScale={scaleBand().padding(0.25)} axis={true} grid={{ y: true }}
              seriesLayout="stack" padding={{ left: 40, bottom: 24, right: 8, top: 8 }}
              props={{ xAxis: { ticks: bandTicks, format: dateTick }, yAxis: { ticks: 3, format: short } }}
              series={[{ key: 'turns', label: 'Responses', color: 'var(--chart-1)' },
                { key: 'steps', label: 'Tool steps', color: 'var(--chart-3)' }]}>
              {#snippet tooltip()}<Chart.Tooltip labelFormatter={dateLabel} />{/snippet}
            </BarChart>
          </Chart.Container>
        </section>

        <section aria-label="Where the tokens went" class="min-w-0 rounded-lg border border-border p-4">
          <h3 class="mb-3 text-sm font-medium">Where the tokens went</h3>
          {#if split.length}
            <!-- A ring plus its legend can ask for a few pixels more than the column at phone
                 width. The wrapper clips that sideways, without making a scroll box, so the
                 tooltip above the chart still shows in full. -->
            <!-- The ring carries no legend of its own: its own key is written below, where the
                 words can say what each part means and wrap freely at phone width. -->
            <!-- The ring and its key side by side, the key as a two-column table so the four
                 parts always land in a two-by-two block, never three and one. -->
            <div class="flex items-center gap-4">
              <Chart.Container config={splitConfig} class="h-[150px] w-[150px] shrink-0">
                <PieChart data={split} key="key" value="value" innerRadius={-24} padding={4}
                  cRange={split.map(part => part.color)}>
                  {#snippet tooltip()}<Chart.Tooltip hideLabel />{/snippet}
                </PieChart>
              </Chart.Container>
              <ul class="grid min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)_auto] gap-x-2 gap-y-1.5 text-xs">
                {#each split as part (part.key)}
                  <li class="contents">
                    <span class="mt-1 size-2 rounded-[2px]" style={`background: ${part.color}`} aria-hidden="true"></span>
                    <span class="truncate">{part.key}</span>
                    <span class="text-right tabular-nums text-muted-foreground">{short(part.value)}</span>
                  </li>
                {/each}
              </ul>
            </div>
            <p class="mt-2 text-xs text-muted-foreground">Cached input is charged at a lower rate by both providers.</p>
          {:else}
            <p class="py-8 text-sm text-muted-foreground">No token breakdown was reported in this range.</p>
          {/if}
        </section>
