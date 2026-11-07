# Models and reasoning

MiniClaw discovers models and reasoning levels from each enabled, signed-in provider.
Selections apply to a conversation or its next turn without changing provider settings.
See the [selection and refresh sequences](sequence.md).

## Catalog

`GET /api/models` requires at least one signed-in provider and combines its model lists.
`GET /api/models?refresh=true` also bypasses Claude's five-minute discovery cache.
Codex always receives `model/list`; its own runtime controls upstream freshness.
MiniClaw maintains no model-name list for either provider.

[models::read](../../server/src/models.rs) collects paginated catalogs within ten
seconds, including hidden entries. This exact fragment in `pages` adds the refresh flag
only for the Claude bridge:

```rust
        let mut params = json!({ "limit": 100, "includeHidden": true, "cursor": cursor });
        if refresh && cursor.is_none() { params["refresh"] = json!(true); }
```

The collector deduplicates model IDs, rejects empty IDs and repeated cursors, and caps
reads at 100 pages and 10000 unique entries. Public models contain `model`, `displayName`,
`description`, `hidden`, `isDefault`, `defaultReasoningEffort`, `supportedReasoningEfforts`,
`serviceTiers`, and `provider` for Claude.

[Claude's bridge](../../server/src/claude.rs) reads `models` from a CLI `initialize`
control response. The command runs in safe mode, without tools, a model prompt or a
persisted session. The shared control reader limits runtime to ten seconds, each line to
256 KiB and total output to 512 KiB. Discovery runs separately from the bridge's chat and
Stop handlers. Only successful catalogs are cached, keyed by the reported account.

Full picker IDs retain context suffixes; aliases use `resolvedModel` when reported.
Duplicate default rows merge into the corresponding model. Display names and descriptions
come from Claude. Reasoning options come from `supportedEffortLevels`; explicit
`supportsEffort: false` suppresses them. MiniClaw chooses medium when advertised, otherwise
the first level. Missing levels mean no explicit effort. Codex retains its reported
default when both providers are available; otherwise Claude's default or first entry wins.

Failed refresh returns an error and retains the last successful catalog in memory.
Ordinary reads can reuse it until its five-minute expiry. The page keeps its selection and
draft and displays an error; it does not replace the catalog with an empty success. Catalog
presence does not establish inference entitlement.

Sources: [Codex model/list](https://learn.chatgpt.com/docs/app-server#list-models-modellist),
[Claude model configuration](https://code.claude.com/docs/en/model-config),
[Claude SDK initialization](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py).
The CLI response fields were also observed locally on Claude Code 2.1.280 on 2026-09-22.

## Validation and application

[api::choose](../../server/src/api.rs) validates choices used by thread creation,
message submission, and agent definitions.

| Requested choice | Behavior |
| --- | --- |
| Neither model nor effort | Leave Codex defaults in place. |
| Effort without model | HTTP 400, `invalid_model`. |
| Known model, omitted effort | Use its advertised default when reasoning options exist. |
| Model with no reasoning options | Omit effort. An explicit unsupported effort is rejected. |
| Unknown model or unsupported effort | HTTP 400, `invalid_model`. |
| `speed` the model does not advertise, or a speed without a model | HTTP 400, `invalid_model`. |

## Speed

Codex 0.155 lists each model's speed tiers in `model/list` (`serviceTiers`, each with an
`id`, a `name` and a `description`; verified on this Mac on 2026-09-22: every GPT model offers
`priority`, shown as "Fast", and GPT-5.6 Sol also `ultrafast`). MiniClaw passes them to the
page as `serviceTiers` and takes a `speed` (a tier's id) beside `model` and `effort` on
conversation creation, on every send, on a queued message, and as a default on the main agent
and on a named agent. A chosen speed goes to Codex as `serviceTier` on `thread/start` and on
every `turn/start`. Codex echoes it on `thread/start` but does not keep it on the thread
(`thread/read` answers null), so [`speeds.rs`](../../server/src/speeds.rs) keeps a
conversation's speed the way [`names.rs`](../../server/src/names.rs) keeps its name, and
`tag_context` lays it over the summary. A send with a model and no speed means standard and
removes the record.

The composer shows a Speed choice beside Reasoning effort, disabled for a model with no
tiers (Claude models have none). The list shows each tier in Codex's own words, such as
"Fast: 1.5x speed, increased usage", because Fast uses more of the plan (OpenAI's API guide:
Fast mode costs twice the standard rate, and `service_tier` takes `fast` or `priority`); the
closed control and a footer badge show the tier's name alone. Sources:
[Fast mode](https://developers.openai.com/api/docs/guides/fast-mode),
[Codex config reference](https://learn.chatgpt.com/docs/config-file/config-reference)
(`service_tier`, the `/fast` command).

Thread creation passes model and `config.model_reasoning_effort`, with provider
fallback disabled for an explicit model. Turns pass `model` and `effort` to
`turn/start`. The send receipt records the requested choice so reusing a message ID
with different settings conflicts. Snapshot summaries retain the selected settings;
restricted resume reads saved settings back from Codex.

## Browser state

[Choice.svelte](../../web/src/Choice.svelte) wraps Bits UI Select with an accessible
name and bounded menu width. [Chat](../../web/src/chat.svelte.ts) owns the catalog
and selection. An initially empty selection uses the default catalog entry, falling
back to the first entry. Changing model selects that model's default effort.

The existing **Refresh providers** control in Settings > Providers calls
`Chat.refresh(true)`, then `refreshModels(true)`. It refreshes both providers, shows their
model counts, and disables repeated clicks during the read. No second refresh control is
added. Normal page refresh uses the ordinary cached read.

Refresh preserves an existing choice, even if it disappears from the catalog. That
choice becomes invalid and blocks sending rather than silently changing the model.
Discovery errors have a separate `modelError` and `Retry models` control. Pending
work disables the selectors. Per-chat unsent settings are kept in browser memory.

Coverage: [API tests](../../tests/models.test.mjs),
[Claude discovery tests](../../tests/claude-models.test.mjs),
[state tests](../../web/tests/models.test.ts),
[model browser tests](../../tests/browser/models.spec.ts), and
[provider browser tests](../../tests/browser/providers.spec.ts).
[The opt-in live catalog probe](../../tests/experiments/real-models.test.mjs) verified both
providers and discovered `claude-opus-5-5[1m]` without sending a model prompt.
