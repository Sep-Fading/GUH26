<script lang="ts">
  import type * as Chart from '$lib/components/ui/chart/index.js';
  // Loads the figures when the panel is shown.
  import { onMount } from 'svelte';
  // Marks the range control and the empty state.
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  // Picks the range with the control the rest of Settings uses.
  import Choice from './Choice.svelte';
  // Reads the figures through the backend.
  import type { Chat } from './chat.svelte';
  // Shows current quotas above historical analytics.
  import UsageLimits from './UsageLimits.svelte';

  let { chat }: { chat: Chat } = $props();

  type Named = { name: string; turns?: number; tokens?: number; calls?: number; failures?: number;
    invocations?: number; source?: string; provider?: string };
  type Day = { date: string; turns: number; tokens: number; inputTokens: number; outputTokens: number; cachedTokens: number; steps: number };
  type Kind = { kind: string; calls: number; ok: number; failed: number; refused: number; averageMs: number };
  type Report = { days: number; totals: { turns: number; tokens: number; inputTokens: number; outputTokens: number;
    cachedTokens: number; reasoningTokens: number; seconds: number; steps: number; conversations: number };
    daily: Day[]; agents: Named[]; projects: Named[]; providers: Named[]; models: Named[]; efforts: Named[];
    steps: Kind[]; mcpServers: Named[]; mcpTools: Named[]; skills: Named[] };

  let report = $state<Report>();
  let days = $state('30');
  let loading = $state(false), error = $state('');

  async function load() {
    loading = true;
    try { report = await chat.analytics<Report>(Number(days)); error = ''; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The figures could not be read.'; }
    finally { loading = false; }
  }

  // Whole numbers with thousands separators; large token counts shorten so a row never wraps.
  const count = (value: number) => value.toLocaleString();
  const short = (value: number) => value >= 1_000_000 ? `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 0 : 1)}M`
    : value >= 1000 ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k` : String(value);
  const hours = (seconds: number) => seconds >= 3600 ? `${(seconds / 3600).toFixed(1)} h`
    : seconds >= 60 ? `${Math.round(seconds / 60)} min` : `${Math.round(seconds)} s`;
  // An axis tick reads as "6 Mar"; a tooltip spells the day out. Both take either the Date the
  // area chart plots or the `YYYY-MM-DD` the bar chart bands by.
  const asDate = (value: Date | string) => value instanceof Date ? value : new Date(`${value}T12:00:00`);
  const dateTick = (value: Date | string) => asDate(value).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  const dateLabel = (value: Date | string) => asDate(value).toLocaleDateString(undefined, { dateStyle: 'medium' });

  const totals = $derived(report?.totals);
  const busiest = $derived(report ? report.daily.reduce((most, day) => Math.max(most, day.tokens), 0) : 0);
  const anything = $derived(!!totals && (totals.turns > 0 || totals.steps > 0));
  // Every day of the range is drawn, so an idle week reads as an idle week and not as a gap.
  const series = $derived((report?.daily ?? []).map(day => ({ ...day, at: new Date(`${day.date}T12:00:00`) })));
  // The token split is a part of a whole with few parts, which is the one case a ring beats a bar.
  const split = $derived(totals ? [
    { key: 'Fresh input', value: Math.max(totals.inputTokens - totals.cachedTokens, 0), color: 'var(--chart-1)' },
    { key: 'Cached input', value: totals.cachedTokens, color: 'var(--chart-4)' },
    { key: 'Output', value: Math.max(totals.outputTokens - totals.reasoningTokens, 0), color: 'var(--chart-2)' },
    { key: 'Reasoning', value: totals.reasoningTokens, color: 'var(--chart-5)' },
  ].filter(part => part.value > 0) : []);

  const tokenConfig = { tokens: { label: 'Tokens', color: 'var(--chart-1)' } } satisfies Chart.ChartConfig;
  const turnConfig = { turns: { label: 'Responses', color: 'var(--chart-1)' },
    steps: { label: 'Tool steps', color: 'var(--chart-3)' } } satisfies Chart.ChartConfig;
  const splitConfig = { value: { label: 'Tokens' } } satisfies Chart.ChartConfig;

  /** The share of the largest row, for the bar behind a ranked row. */
  const share = (rows: Named[], value: number, key: 'tokens' | 'calls' | 'invocations' | 'turns') => {
    const most = rows.reduce((top, row) => Math.max(top, row[key] ?? 0), 0);
    return most > 0 ? Math.round((value / most) * 100) : 0;
  };

  // Fetched once, when the tab is first shown.
  const charts = import('./AnalyticsCharts.svelte');

  onMount(load);
</script>

<section aria-label="Analytics" class="flex min-h-0 flex-1 flex-col">
  <header class="flex h-12 shrink-0 items-center gap-3 border-b border-border px-5 pr-12">
    <h2 class="text-base font-semibold">Analytics</h2>
    <div class="ml-auto flex items-center gap-1.5">
      <Choice label="Range" value={days} placeholder="Last 30 days" disabled={loading}
        options={[{ value: '7', label: 'Last 7 days' }, { value: '30', label: 'Last 30 days' },
          { value: '90', label: 'Last 90 days' }, { value: '365', label: 'Last year' }]}
        onchange={value => { days = value; void load(); }} />
      <button class="icon-button" aria-label="Reload analytics" disabled={loading} onclick={load}>
        <RefreshCw size={15} aria-hidden="true" class={loading ? 'animate-spin' : ''} />
      </button>
    </div>
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto">
    <UsageLimits {chat} />
    {#if error}
      <p role="alert" class="m-5 text-sm text-destructive">{error}</p>
    {:else if !report}
      <p class="m-5 text-sm text-muted-foreground">Reading what MiniClaw has recorded...</p>
    {:else if !anything}
      <div class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
        <ChartNoAxesColumn size={28} class="text-muted-foreground" aria-hidden="true" />
        <p class="max-w-sm text-sm text-muted-foreground">Nothing was recorded in this range. Send a message, and this tab
          fills with what each response cost and which agents, skills and servers took part.</p>
      </div>
    {:else}
      <!-- One column on a phone, two where there is room. `min-w-0` everywhere: a chart's own
           width must never be what decides the column's width, or resizing jumps. -->
      <div class="grid grid-cols-1 gap-4 p-5 lg:grid-cols-2">

        <!-- The headline: what the range cost, with the rest of the totals reading as one sentence. -->
        <div class="min-w-0 pb-2 lg:col-span-2">
          <p class="text-sm text-muted-foreground">Tokens in the last {report.days === 365 ? 'year' : `${report.days} days`}</p>
          <p class="mt-0.5 text-4xl font-semibold tracking-tight tabular-nums">{count(totals!.tokens)}</p>
          <p class="mt-1.5 text-sm text-muted-foreground">
            {count(totals!.turns)} {totals!.turns === 1 ? 'response' : 'responses'} across
            {count(totals!.conversations)} {totals!.conversations === 1 ? 'conversation' : 'conversations'},
            {count(totals!.steps)} tool {totals!.steps === 1 ? 'step' : 'steps'},
            {hours(totals!.seconds)} of waiting.
          </p>
        </div>

        <!-- The charts are their own component, fetched when this tab is first opened: they
             carry a drawing library the rest of MiniClaw never needs. -->
        {#await charts}
          <div class="min-w-0 lg:col-span-2" aria-hidden="true"><div class="h-[200px] w-full animate-pulse rounded-md bg-muted/50"></div></div>
        {:then Charts}
          <Charts.default {series} {split} {tokenConfig} {turnConfig} {splitConfig} {short} {dateTick} {dateLabel} />
          {#if busiest === 0}
            <p class="min-w-0 text-xs text-muted-foreground lg:col-span-2">No provider reported token counts in this range.</p>
          {/if}
        {:catch}
          <p class="min-w-0 text-sm text-destructive lg:col-span-2">The charts could not be loaded. The figures below are unaffected.</p>
        {/await}

        <!-- Every ranked list has the same three columns: name, a note, the figure. The figure's
             unit is in the heading, once, so the rows line up and read as one table. -->
        {#snippet ranked(title: string, rows: Named[], key: 'tokens' | 'calls' | 'invocations' | 'turns',
          unit: string, note: string, second?: (row: Named) => string)}
          <section aria-label={title} class="flex min-w-0 flex-col rounded-lg border border-border p-4">
            <div class="flex items-baseline justify-between gap-3">
              <h3 class="text-sm font-medium">{title}</h3>
              <span class="text-xs text-muted-foreground">{unit}</span>
            </div>
            <p class="mb-3 mt-0.5 text-xs text-muted-foreground">{note}</p>
            {#if rows.length}
              <!-- A ranked bar stays readable at any width, which a ring of eight slices does not. -->
              <ul class="space-y-2">
                {#each rows.slice(0, 8) as row (`${row.name}-${row.source ?? ''}`)}
                  <li class="grid min-w-0 grid-cols-[minmax(0,1fr)_auto_3.5rem] items-baseline gap-x-3 text-sm">
                    <span class="min-w-0 truncate" title={row.name}>{row.name}</span>
                    <span class="text-xs text-muted-foreground">{second ? second(row) : ''}</span>
                    <span class="text-right tabular-nums">{short(row[key] ?? 0)}</span>
                    <div class="col-span-3 mt-1 h-1 w-full overflow-hidden rounded-full bg-muted">
                      <div class="h-full rounded-full bg-chart-1" style={`width: ${share(rows, row[key] ?? 0, key)}%`}></div>
                    </div>
                  </li>
                {/each}
              </ul>
            {:else}
              <p class="py-3 text-sm text-muted-foreground">Nothing yet.</p>
            {/if}
          </section>
        {/snippet}

        {@render ranked('Agents', report.agents, 'tokens', 'tokens',
          'Tokens of the responses each agent gave. Conversations with no agent belong to the main agent.',
          row => `${count(row.turns ?? 0)} ${row.turns === 1 ? 'response' : 'responses'}`)}

        {@render ranked('Models', report.models, 'tokens', 'tokens',
          'What each model was asked to do, across both providers.',
          row => row.provider === 'claude' ? 'Claude' : 'Codex')}

        {@render ranked('Skills', report.skills, 'invocations', 'responses',
          'How many responses a skill was part of, and where it came from.',
          row => ({ agent: 'from an agent', project: 'from a project', conversation: 'from the conversation', slash: 'called with /' })[row.source ?? ''] ?? row.source ?? '')}

        {@render ranked('MCP servers', report.mcpServers, 'calls', 'calls',
          'Calls MiniClaw made on your behalf. The note is the tokens of the responses they took part in.',
          row => `${short(row.tokens ?? 0)} tokens${row.failures ? `, ${count(row.failures)} failed` : ''}`)}

        {@render ranked('MCP tools', report.mcpTools, 'calls', 'calls',
          'The individual tools, named server and tool.',
          row => row.failures ? `${count(row.failures)} failed` : 'no failures')}

        {@render ranked('Projects', report.projects, 'tokens', 'tokens',
          'Only conversations that belong to a project are counted here.',
          row => `${count(row.turns ?? 0)} ${row.turns === 1 ? 'response' : 'responses'}`)}

        <section aria-label="Tool steps" class="min-w-0 rounded-lg border border-border p-4 lg:col-span-2">
          <h3 class="text-sm font-medium">Tool steps by kind</h3>
          <p class="mb-2 mt-0.5 text-xs text-muted-foreground">What MiniClaw's own tools were asked to do, and how each request ended.
            A refused step is one no rule allowed and nobody approved.</p>
          {#if report.steps.length}
            <!-- The table scrolls sideways at phone width, so it takes focus and a name of its own:
                 a keyboard alone has to be able to reach and scroll it (axe scrollable-region-focusable).
                 Svelte's own rule says the opposite for a non-interactive element; WCAG wins. -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <div class="overflow-x-auto" tabindex="0" role="region" aria-label="Tool steps by kind">
              <table class="w-full min-w-[26rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs text-muted-foreground">
                    <th scope="col" class="py-1.5 pr-3 font-medium">Kind</th>
                    <th scope="col" class="py-1.5 pr-3 text-right font-medium">Calls</th>
                    <th scope="col" class="py-1.5 pr-3 text-right font-medium">Ran</th>
                    <th scope="col" class="py-1.5 pr-3 text-right font-medium">Failed</th>
                    <th scope="col" class="py-1.5 pr-3 text-right font-medium">Refused</th>
                    <th scope="col" class="py-1.5 text-right font-medium">Typical</th>
                  </tr>
                </thead>
                <tbody>
                  {#each report.steps as step (step.kind)}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="py-1.5 pr-3">{step.kind}</td>
                      <td class="py-1.5 pr-3 text-right tabular-nums">{count(step.calls)}</td>
                      <td class="py-1.5 pr-3 text-right tabular-nums">{count(step.ok)}</td>
                      <td class="py-1.5 pr-3 text-right tabular-nums" class:text-destructive={step.failed > 0}>{count(step.failed)}</td>
                      <td class="py-1.5 pr-3 text-right tabular-nums">{count(step.refused)}</td>
                      <td class="py-1.5 text-right tabular-nums text-muted-foreground">{step.averageMs < 1000 ? `${step.averageMs} ms` : `${(step.averageMs / 1000).toFixed(1)} s`}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {:else}
            <p class="py-3 text-sm text-muted-foreground">No tool ran in this range.</p>
          {/if}
        </section>

        <p class="min-w-0 text-xs text-muted-foreground lg:col-span-2">
          Tokens are what the provider reported for each response. A response is counted under every agent, skill and
          server that took part in it, so those figures say what a thing was present for, not what it alone caused.
          MiniClaw keeps counts and names for a year; it keeps no message, file or tool output here, and sends none of this to a model.
        </p>
      </div>
    {/if}
  </div>
</section>
