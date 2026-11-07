# Skill groups

Groups collect owned skills without moving files or granting tools. A skill can
belong to multiple groups. See the [sequence](sequence.md).

## Activation and selection

[skill_groups.rs](../../server/src/skill_groups.rs) stores each group's name,
`active` master switch, revision, and members. Each member has a `skillId` and
`enabled` flag. The master switch preserves member flags while the group is off.
Neither switch changes individual skill activation or another group.

A group contributes an enabled member even when that skill's individual `active`
flag is false. Deleted groups and deleted members contribute nothing. Deleting a
group never deletes its skills.

Projects attach `skillGroupIds` to default MiniClaw conversations. Named agents do
not inherit project groups. Named agents save their own `skillGroupIds` through
[Agents.svelte](../../web/src/Agents.svelte) and `Store::save` in
[agents.rs](../../server/src/agents.rs). Existing conversations keep the group IDs
from their pinned agent revision; editing the agent selection applies to new conversations.
Group contents and activation are read fresh each turn.

Conversations can also select groups and individual
skills explicitly, including before their first message. Sources remain separate.
`Store::turn_skills` deduplicates skill IDs and excludes IDs already pinned by an
agent or project. Pinned copies take precedence over live group copies of the same skill.

Individual conversation selections pin instructions when added. Changing group
selections preserves those copies. Removing and re-adding an individual selection
picks up its current revision. Individual selections still require globally active
skills; their activation checks remain separate from group activation.

## Storage and routes

The existing private `agents.sqlite3` stores `skill_groups` and
`chat_skill_selections`. The latter contains independent references and individual
instruction snapshots. New conversation selections commit with agent/project
bindings after Codex creates the task. Codex creation and local binding are separate
commits; see [recovery constraints](../agents/sequence.md).

[api.rs](../../server/src/api.rs) uses the existing account checks, loopback origin
checks, mutation header, and `agent_gate`. Group edits take the write lock; sending
holds the read lock through turn admission. Group edits never request cancellation.

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/skill-groups` | None | `groups`, including inactive groups |
| `POST /api/skill-groups` | Group definition | HTTP 201, `group` at revision 1 |
| `POST /api/skill-groups/{id}` | Complete definition and observed revision | Updated `group` |
| `POST /api/skill-groups/{id}/delete` | Observed `revision` | Empty JSON object |
| `GET /api/threads/{id}/skills` | Loaded owned task | Explicit `selection` |
| `POST /api/threads/{id}/skills` | Complete selection and observed revision | Updated `selection` |

`POST /api/threads` optionally accepts `skillSelection` at revision 0. An untouched
conversation selection also reads as revision 0. Subsequent saves increment it.
Stale writes return 409. Unknown fields, duplicates, and invalid references are
rejected. Missing/deleted references must be removed before saving settings again.

Limits: 64 groups, 80-byte names, 128 members per group, 16 selected groups per
agent, project or conversation, 8 explicit individual skills, and 4096 stored conversation
selections. A turn allows 32 additional distinct skills and at most 131072 bytes of
composed selection context. Exceeding the turn limit returns `skill_group_limit`
before adding a user message or requesting inference.

## Next-message context

`Store::turn_skills` reads current group membership, member switches, activation,
and skill text before each new message. Changes leave an active response alone.
`api::send` adds the resulting context to `turn/start` through this source fragment:

```rust
            "additionalContext": { "miniclaw_skills": { "kind": "application",
                "value": instructions.unwrap_or_else(|| "No conversation skill selections are active.".into()) } },
```

The same key is sent when the selection becomes empty. Pinned agent/project
instructions remain in their existing developer context. No archive, resume,
transcript reload, new listener, or shared Codex configuration write is required.
Disabling a skill does not erase earlier conversation history or undo past actions.

The installed runtime probe verifies this keyed context reaches the model request
and that disabling the group replaces its current contribution. The attempted
`collaborationMode` custom-instruction override was accepted but omitted from the
model request, so production does not use it. The field is experimental; generated
schemas require `--experimental`, and the existing handshake enables `experimentalApi`.

## What repeating the text costs

MiniClaw sends the skills text with every message. Whether that is wasteful was measured with
the installed Codex against a fake model endpoint
([`skill-context-cost.test.mjs`](../../tests/experiments/skill-context-cost.test.mjs), run with
`MINICLAW_RUNTIME_PROBE=1`; no credentials and no paid inference), counting copies of the text
in each request the model would receive:

| Turn | Sent | Copies the model receives |
|---|---|---|
| 1 to 3 | the same text under `miniclaw_skills` | 1 |
| 4 | nothing | 1: the earlier copy stays in the conversation |
| 5 | changed text | 2: the old copy stays and the new one is added |
| 7 | the same skills, plus a second key for that turn only | skills unchanged; the extra once |

So with Codex an unchanged key costs nothing, and a changed key is added again in full and
stays. Reading the skills at every message is therefore free until something changes; an edit
to a skill or a group costs each open conversation one more copy of the block on its next
message, which is also how the edit reaches it.

Because a change re-adds the whole block, text that belongs to one message never goes into it.
A called skill travels as `miniclaw_slash` and delivered delegation results as
`miniclaw_delegations` ([`api.rs`](../../server/src/api.rs) `send`).

Claude is different. The CLI resends the conversation on every run and Anthropic caches the
unchanged prefix, in the order tools, system, messages; a change to the system prompt
invalidates the cached messages after it
([prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)). The
skills block is part of the system prompt, so it costs nothing extra while unchanged, and a
change makes the next run re-read the conversation uncached once. The one-message extras go in
front of the message the CLI reads instead, inside `<miniclaw-context>`, so the system prompt
does not change; the saved message stays as typed ([`claude.rs`](../../server/src/claude.rs)
`stream`). Whether Claude Code applies caching to these runs was not observed.

## Interface and checks

[Skills.svelte](../../web/src/Skills.svelte) lists folder rows and opens
[GroupEditor.svelte](../../web/src/GroupEditor.svelte). Membership and enabled state
are separate controls. [GroupChoices.svelte](../../web/src/GroupChoices.svelte)
serves agent creation/editing, project settings and [ConversationSkills.svelte](../../web/src/ConversationSkills.svelte).
Conversation saves retain the message draft and transcript view. Errors retain
unsaved settings; deletion requires confirmation.

- [API tests](../../tests/skill-groups.test.mjs): source isolation, next-turn changes,
  persistence, limits, deletion, and pinned individual copies.
- [Browser tests](../../tests/browser/skill-groups.spec.ts): group editing, project
  selection, conversation selection, confirmation, and narrow layouts.
- [State tests](../../web/tests/skill-groups.test.ts): stale lists, account changes,
  and failed edits.
- [Runtime probe](../../tests/agent-runtime.test.mjs): production-generated context
  through installed Codex with fake inference and unchanged tested tool inventories.
