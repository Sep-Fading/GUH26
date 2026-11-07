# Deletion

Deletion requires an explicit browser confirmation. Management mutations use the
existing [local request guard](../security/README.md) and ChatGPT sign-in.
See the [sequence](sequence.md).

## Routes

[api.rs](../../server/src/api.rs) accepts these JSON POST requests:

| Route | Input | Effect |
| --- | --- | --- |
| `/api/threads/{id}/delete` | Empty object | Deletes one owned idle Codex transcript and local bindings. |
| `/api/agents/{id}/delete` | Observed `revision` | Removes the profile from management and denies further execution. |
| `/api/skills/{id}/delete` | Observed `revision` | Deletes the owned copy without modifying its installed source. |

Successful requests return `{}`. Agent and skill revision mismatches return 409.
Unknown fields and invalid IDs are rejected. Management changes serialize with
turn admission through `agent_gate`.

## Conversations

`delete_chat` verifies the exact MiniClaw workspace/provider and idle Codex status.
Cached active, pending, uncertain, or failed computer-switch states block deletion.
Codex deletion includes descendants; MiniClaw checks both active and archived
descendants and rejects a parent rather than deleting unconfirmed child tasks.

The upstream call and local cleanup occur in this order:

```rust
        call(&backend, "thread/delete", json!({ "threadId": id })).await?;
        backend.agents.remove_chat(id.clone()).await?;
```

[Store::remove_chat](../../server/src/projects.rs) records the ID in `removed` and
deletes project, agent, and skill bindings in one transaction. The cache is removed
and SSE readers wake and close. Markers prevent stale saved-list replies from
restoring acknowledged deletions; repeated requests for marked IDs return success.

An upstream failure leaves local records unchanged. Work continues if the HTTP
caller disconnects. Codex deletion and local cleanup are separate commits: a process
crash or storage failure between them can leave stale local bindings. MiniClaw does
not claim transactional deletion across Codex and SQLite or secure disk erasure.

## Agents and skills

Agent removal stores a marker and excludes the profile from listing and new-chat
creation. Its registry row remains because saved bindings reference it. Pinned
profile instructions and identity remain readable, but admission rejects new turns.
An old named-agent chat never becomes an unrestricted MiniClaw chat.

Skill removal deletes the owned row. Installed Codex sources and instruction copies
already pinned to conversations remain. A missing pinned skill blocks new turns;
affected cached active turns receive an interrupt request. Existing agent/project
settings retain missing references, which the settings forms let the user remove.

## Confirmation state

[ConfirmDelete](../../web/src/ConfirmDelete.svelte) uses a Bits UI Alert Dialog.
Cancel sends no mutation. Confirm disables repeat actions while pending; failure
keeps the dialog and its error visible. The list changes only after acknowledgement.
[Chat.deleteChat](../../web/src/chat.svelte.ts) invalidates stale list replies without
invalidating another conversation's active stream.

Coverage: [API tests](../../tests/projects.test.mjs),
[browser tests](../../tests/browser/projects.spec.ts),
[stream state regression](../../web/tests/projects.test.ts), and
[disposable installed-runtime deletion](../../tests/agent-runtime.test.mjs).
