# Defined agents

An agent is a saved instruction profile with model defaults and explicit activation.
Each direct chat pins one profile revision. Codex stores the conversation; MiniClaw
stores the profile and binding. See the [sequence](sequence.md).

## Entry points

[api.rs](../../server/src/api.rs) defines these authenticated routes. Mutations use
the existing same-origin checks, JSON body limit, and `X-MiniClaw-Request: 1` header.

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/agents` | None | `agents`: current profiles, including inactive ones. |
| `POST /api/agents` | `Definition`, without a revision | HTTP 201 with `agent`; revision starts at 1. |
| `POST /api/agents/{id}` | `Definition`, including its current revision | Updated `agent`; stale revisions return 409. |
| `POST /api/threads` | Optional `agentId`, model, and effort | HTTP 201 with the chat summary and pinned agent identity. |
| `POST /api/threads/{id}/messages` | Existing message input | Rechecks activation before starting a new turn. |

[Definition](../../server/src/agents.rs) requires `name` and `instructions`. Optional
fields are `model`, `effort`, `revision`, `active`, `capabilities`, `access`, and
`delegatesTo`, `skillIds`, and `skillGroupIds`. Unknown fields are rejected. New profiles default to inactive with
empty capability, access, and delegation lists. Model choices are checked against
the Codex catalog.

The agent editor offers [skill groups](../skill-groups/README.md) on creation and editing.
Each profile accepts up to 16 distinct existing groups. Their IDs are pinned with
the conversation's agent revision; group contents and activation remain live.

## Capability and access policy

Capabilities describe operations such as `computer_use` and `deep_research`.
Access describes application IDs and actions, file paths and read/write grants, and
network hosts. These fields store definitions; they do not install or enable tools.

Only profiles with empty `capabilities` and `access` fields can be active.
Unsupported definitions may be saved inactive; activation returns
`agent_policy_unsupported` with HTTP 422. This exact fragment from
[Definition::supported](../../server/src/agents.rs) determines support:

```rust
        self.capabilities.is_empty()
            && self.access.applications.is_empty()
            && self.access.files.is_empty()
            && self.access.network.is_empty()
```

Delegation targets no longer make a profile unsupported ([delegation](../delegation/README.md)).
These stored `capabilities` and `access` fields come from an older design. What an agent may
use is set in its Capabilities section, and a conversation with no agent belongs to the main
agent; both are described in [capabilities](../capabilities/README.md). The editor shows the
old fields only for an agent that still carries them.

Agent chats reject the broader [Computer Use switch](../computer-use/README.md)
with HTTP 403. Agent instructions cannot override the fixed runtime configuration.
No profile-management tools are registered with Codex. Tool network access being
empty does not mean offline inference: prompts and instructions still reach Codex's
model service using the existing OAuth account.

## Storage and revisions

[main](../../server/src/main.rs) opens `agents.sqlite3` in `MINICLAW_DATA_DIR`, whose
default is `$HOME/Library/Application Support/MiniClaw`. Startup retains the existing
instance lock and single loopback listener. SQLite does not open a network port.

[Store](../../server/src/agents.rs) uses parameterized SQL and transactions through
`rusqlite`. Blocking database work runs through `spawn_blocking` with one serialized
connection. New database files use mode `0600`; symlinks and existing files with
group/other permissions are rejected. Database errors are not repaired by deleting
or replacing data.

[Identity checks](../../server/src/agents/identity.rs) pair `agents.sqlite3` with a
private `agents.sqlite3.identity` file. Startup rejects a missing or mismatched
database, missing policy tables, or a missing, linked or non-private marker.
Existing pre-identity databases are upgraded after their base tables are checked.
An interrupted identity write stops startup; it is not silently repaired.

[Issue::storage](../../server/src/agents.rs) records error categories on backend
stderr. SQLite, JSON, I/O and task failures retain the generic public API error;
diagnostics omit SQL, paths and saved contents. The
[storage regressions](../../tests/storage-errors.test.mjs) verify redaction and
recovery after an injected fault is removed, without restarting the backend.

| Table | Contents |
| --- | --- |
| `agents` | UUID and current serialized definition. |
| `agent_chats` | Codex thread ID, agent UUID, and immutable definition snapshot. |
| `skills` and `agent_chat_skills` | [Reusable instructions and pinned copies](../skills/README.md). |

The registry allows 64 profiles and 4096 chat bindings. Names are limited to 80 bytes
and instructions to 16384 bytes. Edits increment the revision in a transaction.
They apply to new chats; existing chats retain their original instructions and name.
Deactivation applies to existing chats too: later sends are rejected, while history
remains readable. Active cached turns receive a cancellation request. Cancellation
cannot undo completed work.

Keep this database and its identity file with the Codex history when moving or backing up MiniClaw.
Bindings cannot be reconstructed from transcripts. File permissions do not protect
against another process running as the same OS user, and this storage is not a
general device sandbox.

## Chat integration

[Projects](../projects/README.md) may restrict which active agents are available.
Named agents keep their own skills inside projects. [Deletion](../deletion/README.md)
removes a profile from management without losing its saved conversation identity.

[api::create_chat](../../server/src/api.rs) checks activation and supported policy,
lists existing chat IDs, and saves provisional bindings before starting a restricted
Codex thread. Creation continues if the HTTP caller disconnects.
Profile edits and run admission use `Backend::agent_gate`; independent sends share
a read lock, while profile edits take the write lock.

[Creation storage](../../server/src/agents/creation.rs) records the returned Codex ID
before moving the provisional agent, project and skill bindings in one transaction.
These calls in `api::create_chat` separate the receipt from that final commit:

```rust
    backend.agents.created(intent, id.to_owned()).await?;
    backend.agents.check_creation(id.to_owned()).await?;
```

If the move fails, listing or loading that ID retries it, including after restart.
It does not call `thread/start` again. Missing required binding rows fail closed.
Startup migrates existing binding requirements once; missing creation tables after
migration stop startup rather than recreating empty admission state.

Without a returned ID, MiniClaw cannot reliably associate an upstream chat with the
request. The retained creation record blocks unfamiliar, unadmitted history with
`chat_creation_uncertain` (HTTP 409). Lists mark it `recoveryRequired`; deletion
remains available. Previously listed chats and successfully admitted new chats remain
usable. A Codex error retains the same guard because partial creation is possible.
No automatic adoption, deletion, or resend occurs. At most 64 unresolved creations
and 4096 baseline IDs per creation are accepted; exhausting these bounds blocks new
creation. Explicit [compaction](../creation-recovery/README.md) releases unknown journal
slots while retaining their denial permanently.

## Recovering interrupted creation

Refresh the chat list after the storage fault is resolved. A saved returned ID lets
`check_creation` retry the original binding transaction; reopening that chat also
retries it. Verify the original agent identity before continuing. Do not repeat the
original message automatically. The [storage regressions](../../tests/storage-errors.test.mjs)
cover this retry after restart without another `thread/start`.

If the chat reports `chat_creation_uncertain`, a current or previously compacted
creation has no recorded returned ID, and this chat cannot be matched safely to
admitted history. A permanent quarantine fence can keep it blocked even when the
pending creation journal is empty.
Keep it blocked. The interface allows confirmed [deletion](../deletion/README.md)
of that owned idle chat, or creation of a new chat while admission capacity remains.
Deleting an unfamiliar chat does **not** reconcile the unknown creation record or
release its journal slot: there is no reliable mapping between them.

At 64 unresolved creations, new creation stops. Open Notifications, then Creation
recovery to review the pending counts and explicitly free unknown journal slots.
The [recovery workflow](../creation-recovery/README.md) retains their combined denial
as a permanent quarantine fence before removing provisional copies. Known receipts
keep their existing retry path. The checkbox starts unchecked; no chat is admitted,
deleted or resent by recovery.

A baseline larger than 4096 IDs still prevents admission. Storage faults must be
resolved before recovery can commit. Preserve the metadata database, identity file
and matching Codex history; do not remove admission or quarantine records manually.

[Agent::configure](../../server/src/agents.rs) adds instructions without changing
tool configuration:

```rust
        params["developerInstructions"] = json!(instructions);
```

`instructions` combines the profile text and its selected skill snapshots. Skills
do not change tool configuration. See [skill binding](../skills/README.md).

[api::load](../../server/src/api.rs) restores the pinned instructions on restricted
resume. Task lists and snapshots include `agentId`, `agentName`, and `agentRevision`.
Account-generation checks prevent late replies from publishing old-account state.

[Agents.svelte](../../web/src/Agents.svelte) provides profile editing in a Bits UI
dialog with live Markdown instruction editing and explicit skill attachments.
[App.svelte](../../web/src/App.svelte) selects an active agent for an empty
chat; selection is fixed once that chat exists. The manager preserves unsupported
access fields when editing an inactive definition. Profile-list errors do not disable
ordinary MiniClaw chat. [Chat.saveAgent](../../web/src/chat.svelte.ts) prevents an
older list response from overwriting a completed edit.

## Verification

- [API tests](../../tests/agents.test.mjs): auth, origin checks, validation, revisions,
  restoration, deactivation, cancellation, and forbidden tool-policy changes.
- [Storage tests](../../server/tests/agents.rs): symlinks, public permissions,
  corruption preservation, and literal SQL-looking instructions.
- [Browser tests](../../tests/browser/agents.spec.ts): profile editing, activation,
  direct messages, preserved access fields, and narrow-screen layout.
- [State test](../../web/tests/agents.test.ts): stale list responses after deactivation.
- [Runtime probe](../../tests/agent-runtime.test.mjs): installed Codex with disposable
  homes and fake inference. Normal agent inventories contain only `clock__curr_time`
  before and after restart. A separate test-only `delegate_task` returns a fake receipt;
  it does not run a worker or establish production delegation.
