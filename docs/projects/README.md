# Projects

Projects group conversations and restrict which named agents may start or continue
work inside them. They do not select a filesystem directory or grant access.
Codex owns transcripts; MiniClaw owns project settings and membership.
See the [sequence](sequence.md).

## API and storage

[api.rs](../../server/src/api.rs) exposes authenticated same-origin routes:

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/projects` | None | `projects`, in creation order. |
| `POST /api/projects` | Name, optional `agentIds` and `skillIds` | HTTP 201, `project` at revision 1. |
| `POST /api/projects/{id}` | Complete settings and observed `revision` | Updated project; stale revision returns 409. |
| `POST /api/threads` | Optional `projectId` alongside existing choices | Chat summary includes project identity. |

[Definition](../../server/src/projects.rs) rejects unknown fields, names over 80
bytes, duplicate selections, more than 64 agent IDs, or more than eight skill IDs.
References must exist. Saving settings does not activate agents or skills globally.
The project registry holds at most 64 projects.

The private `agents.sqlite3` contains `projects` for definitions and `project_chats`
for thread membership and default-agent skill snapshots, capped at 4096 bindings.
[Store::bind](../../server/src/agents.rs) saves project and named-agent bindings in
one transaction. Codex thread creation precedes that transaction.

## Instructions and admission

[Store::prepare_project](../../server/src/projects.rs) checks named-agent membership.
Its skill selection is exactly:

```rust
            let skills = if agent.is_none() {
                skills::resolve(db, &project.definition.skill_ids, true)?
            } else { Vec::new() };
```

Default MiniClaw conversations pin the project's selected active skills. Named-agent
conversations use only the [agent's own skills](../skills/README.md), including skills
not selected by the project. Neither path grants additional tools.

New chats use current settings. Existing chats restore their pinned instruction text
through `configure_context`, including during Computer Use switches. A project's permission
settings can also allow a capability for its conversations, on top of what their agent allows
([capabilities](../capabilities/README.md)). Project renaming
does not change conversation identity. `tag_context` supplies `projectId` and
`projectName` on restored summaries.

[Store::admit](../../server/src/projects.rs) checks current global activation and
project membership before new turns. Removing a named agent from project settings
blocks further work there. Deactivating or deleting a pinned skill blocks affected
chats. Settings mutations request cancellation of revoked cached active turns;
history stays readable. Cancellation does not reverse completed actions. Removing
a skill from project settings affects new default chats; existing chats retain their
snapshot while that skill remains globally active.

## Interface

[TaskNav](../../web/src/TaskNav.svelte) puts expandable folder rows above ordinary
conversations. Project actions appear on hover/focus and stay visible on touch
layouts. Mobile uses the same navigation in a Bits UI dialog.
[Projects state](../../web/src/project-state.svelte.ts) stores collapsed IDs in
`localStorage`; names and instructions are not saved there.

[ProjectSettings](../../web/src/ProjectSettings.svelte) selects agents and MiniClaw
skills. Unavailable references remain removable checkboxes. Failed saves retain the
draft; revision conflicts offer an explicit reload.

Active skills select immediately. Selecting an inactive skill opens **Activate and
add**, which requires confirmation before globally activating its observed revision.
Only a successful activation checks the selection. Cancellation or failure leaves it
unchecked; pending activation prevents dismissal. Activation persists without saving
the project and may unblock other chats pinned to that skill. Removing a project
selection does not deactivate the skill globally.

[Chat](../../web/src/chat.svelte.ts)
filters the agent picker, carries `projectId` into first-send creation, and preserves
backend work when switching conversations. One turn per chat may run; different
chats can respond independently.

Coverage: [API](../../tests/projects.test.mjs), [state](../../web/tests/projects.test.ts),
[browser](../../tests/browser/projects.spec.ts), and
[installed runtime probe](../../tests/agent-runtime.test.mjs).
