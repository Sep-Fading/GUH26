# Analytics

What MiniClaw records about its own use, and the Settings tab that draws it. It holds counts and
names: how many responses, what each cost in tokens, which tools ran, which agents, skills and
MCP servers took part. It holds no message, no file content and no tool output, and none of it is
ever sent to a model.

Sequence: [sequence.md](sequence.md).

## What is recorded

Three tables in the existing private metadata database
([`analytics.rs`](../../server/src/analytics.rs)), pruned to a year on every write.

| Table | One row per | Written by |
|---|---|---|
| `usage_turns` | finished response: provider, model, effort, agent, project, status, seconds, tokens | `Backend::record_turn` on `turn/completed` ([`chat.rs`](../../server/src/chat.rs)) |
| `usage_steps` | tool call: kind, status, duration; MCP steps also carry `server: tool` | `Backend::publish_activity`, for any entry that is no longer `running` |
| `usage_skills` | skill that took part in a response, with its source | `send` in [`api.rs`](../../server/src/api.rs), when the turn gets its id |

A skill's source is `agent`, `project`, `conversation` or `slash`. A step's name is empty except
for MCP, because a title otherwise holds a path, a command or something the user typed.

Rows outlive their conversation. What a machine was used for is not private chat content, and a
year that emptied itself whenever a conversation was deleted would answer nothing.

## Where the token counts come from

Both providers report what a response cost, and MiniClaw reads both through one notification.

- Codex sends `thread/tokenUsage/updated` while a turn runs (schema `ThreadTokenUsage`). The
  newest `last` breakdown is kept on the `Chat` and written down when the turn ends.
- The Claude CLI reports `usage` in its final `result` frame. `claude.rs` turns that into the
  same notification, so one path counts both. `cache_read_input_tokens` becomes cached input.

Observed with real models (`tests/experiments/real-analytics.test.mjs`): `gpt-5.6-luna` reported
5243 tokens for one short answer and Claude Haiku 4.5 reported 2991, each landing in the totals,
the daily column and the model's row.

## Reading it

`GET /api/analytics?days=N`, 1 to 365, 30 by default. Anything else is a 400. It returns
`totals`, a `daily` array with one entry for every day of the range including the empty ones, and
ranked lists: `agents`, `projects`, `providers`, `models`, `efforts`, `skills`, `mcpServers`,
`mcpTools` and `steps` by kind. Each list is capped at 24 rows.

**Attribution is by participation.** A response's tokens are counted under every agent, skill and
MCP server that took part in it, so those figures say what a thing was present for, not what it
alone caused, and the columns do not sum to the total. The tab says this in as many words. A
server that was called twice in one response counts that response's tokens once.

## The tab

Current [provider usage limits](../account/README.md#usage-normalization) appear at the
top, with percentages and reset times for each installed provider. These account-wide
readings refresh independently of the historical date range and remain visible when
that range has no recorded activity.

[`Analytics.svelte`](../../web/src/Analytics.svelte) in Settings. Its charts live in
[`AnalyticsCharts.svelte`](../../web/src/AnalyticsCharts.svelte), fetched with a dynamic import
when the tab is first opened: they carry LayerChart and its d3 packages, which nothing else in
MiniClaw needs, and which cannot even be imported under jsdom.

The charts are shadcn-svelte's (`$lib/components/ui/chart`, LayerChart 2.5.0): an area chart of
tokens a day, a banded stacked bar chart of responses and tool steps, and a ring of where the
tokens went. Series colours are `--chart-1..5` in `app.css`; the first is the user's own accent,
so the accent stays the hero and the rest read as supporting data.

Everything else is a ranked bar row or a table, not a chart. Eight agents in a ring is unreadable
at 390px; eight ranked rows are not.

Layout, tightened from screenshots on 2026-09-22: every block is a card (`section` with its own
name) in a two-column grid, so the two halves of a row stay level however many rows each holds.
Every ranked list has the same three columns, name, note and figure, and names its unit once in
its heading, so the figures line up as one column instead of ending in "160k tokens" beside
"Codex" beside "14 turns". The word for a model's answer is "response" everywhere. A skill's
source is said in words ("called with /"). The area and bar charts share the same paddings so
their plots line up, and the bar chart picks four dates spaced inside the range because a band
scale would otherwise put its first date under the y-axis zero. The ring's key is a two-column
table beside it, so four parts always sit two by two.

Both built-in legends are switched off and written by hand. LayerChart's own legend lands on the
date labels in the bar chart, and at phone width it made the ring a few pixels wider than its
column.

Responsiveness is tested rather than assumed: `tests/browser/analytics.spec.ts` resizes the
window through six widths down to 390px and fails if any chart is wider than its column or the
page gains a sideways scrollbar. The table of tool kinds scrolls sideways at phone width, so it
takes focus and a name of its own; Svelte's linter objects to a `tabindex` there and WCAG
requires it, so the rule is silenced with its reason beside it.

## Checked by

`tests/analytics.test.mjs` (both fake providers), unit tests for the range boundaries,
`tests/browser/analytics.spec.ts` (four tests on desktop and mobile, with axe), and
`tests/experiments/real-analytics.test.mjs` with real models.
