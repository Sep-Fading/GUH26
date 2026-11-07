# Saved history

MiniClaw lists and resumes Codex's local tasks for its controlled workspace. It
does not mirror the ChatGPT website or maintain another transcript database.
See the [restore sequence](sequence.md).

## Entry points

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/threads` | Optional `cursor` | `threads`, `nextCursor`; includes cached chats on the first page. |
| `GET /api/threads/{id}` | Task ID | Loaded snapshot with recent history and `historyCursor`. |
| `POST /api/threads/{id}/activate` | Empty JSON object | Ensures loading/subscription, returns `{}`. |
| `GET /api/threads/{id}/history` | Optional `cursor` | Text-only `messages` and `nextCursor`. |
| `GET /api/threads/{id}/events` | Optional event cursor | Loads an uncached task before opening SSE. |

These authenticated handlers are in [api.rs](../../server/src/api.rs). History
pagination cursors and SSE replay cursors are different values and are not interchangeable.

## Task ownership

[history::list](../../server/src/history.rs) requests non-archived OpenAI tasks with
source kinds `appServer` and `vscode`, descending creation order, and 50 entries per
page. The installed runtime uses `vscode` for App Server tasks. Every returned task
also passes local validation; upstream filters alone do not grant access.

The exact rejection predicate in `history::summary` is:

```rust
    if Uuid::parse_str(id).is_err()
        || value["cwd"] != cwd
        || value["modelProvider"] != "openai"
        || value["ephemeral"] == true
```

Names use saved name, then preview, then `New chat`, truncated to 200 characters.
Created timestamps are converted from seconds to milliseconds with overflow checking.
Summaries default Computer Use to off. [Agent bindings](../agents/README.md) add
the pinned profile identity; cached summaries take precedence over saved summaries.

## Loading and recovery

`api::load` serializes hydration and returns a lease for an already-cached task.
It reads metadata, checks ownership, reserves capacity, and buffers incoming
notifications before restricted `thread/resume`. It validates the resumed ID and
workspace again, reads recent turns, then publishes history with buffered events
under one data lock. Hydration continues if its requesting browser disconnects.

[Data::make_room](../../server/src/chat.rs) keeps at most 32 loaded chats. When
full, it evicts the least recently accessed eligible history cache. Open HTTP
requests and SSE streams hold leases. Active or uncertain work, send receipts,
Computer Use and permission recovery prevent eviction. Newly created chats also
stay pinned because Codex cannot resume them before their first message.

Eviction removes only the local cache. Reopening loads Codex history with a fresh
replay epoch; no archive, deletion or resend occurs. If no eligible slot exists,
admission returns `409 task_limit`. Receipt-bearing chats remain pinned for
backend-lifetime duplicate-send protection, so this is not an aggregate memory limit.

An unloaded task whose history still contains an active turn returns HTTP 409 with
`history_active`; partial history has no cursor shared with live deltas. Already-loaded
tasks continue streaming. Notification buffering allows 512 frames and 2 MiB; overflow
fails the load. Changed account generations also prevent publication.

Restored chats use normal-chat tools, even if computer tools were previously enabled.
The pinned instructions of an agent chat are reapplied. First-page listing also
runs [computer-switch recovery](../computer-use/README.md) for ID-only pending records.
It does not unarchive unrelated saved tasks.

## Replay and slow readers

[Chat::emit](../../server/src/chat.rs) stores serialized UTF-8 JSON, retaining at
most 512 events and 2 MiB per chat, including event names and cursor IDs. JSON
escaping counts toward that budget. With at most 32 cached chats, retained replay
payload is limited to 64 MiB. Oldest replay events leave first:

```rust
        while self.events.len() >= 512 || self.replay_bytes + bytes > 2 * 1024 * 1024 {
            self.replay_bytes -= self.events.pop_front().unwrap().bytes();
        }
```

An event larger than the budget advances the sequence and clears replay, preserving
the full reducer transcript. [api::events](../../server/src/api.rs) sends
`resync.required` for an expired, missing or foreign cursor. The marker has no event
ID, so a dropped connection cannot acknowledge an undelivered snapshot. The next
body poll selects a fresh full snapshot and its cursor under the data lock.

Each SSE body selects one event per poll. Paused readers retain no cloned replay
batch or pending snapshot. Every selection checks the account generation and runtime
availability; the stream refreshes authorization every 15 seconds when polled.
The backend admits at most 64 SSE streams, acquiring a permit before loading a chat.
Excess streams receive `503 stream_limit`. Load errors, dropped response bodies and
stream completion release permits. These permits do not gate response or Stop RPCs.

The replay payload ceiling is not a process-memory ceiling. Transcripts, container
and allocator overhead, RPC queues, serialization temporaries and HTTP buffers are
outside it. Full snapshots can exceed 2 MiB. Stream admission limits their concurrent
delivery count to 64 but does not bound each snapshot's size. Active work, uncertain
sends and receipts keep their existing cache protections.

## Pages and browser state

`history::page` requests 50 turns with full items in descending order, then reverses
them into conversation order. It extracts text from `userMessage` and `agentMessage`
items only. Invalid statuses, duplicate/empty message IDs, malformed cursors, more
than one active turn, or oversized pages fail the read. Pages allow at most 10000
messages and 2 MiB of message text. Upstream cursors must be nonempty and at most
4096 bytes when present.

[Chat.taskList](../../web/src/chat.svelte.ts) follows bounded pages and deduplicates
task IDs. `refresh` chooses the saved `miniclaw.activeTask` when available; `select`
opens its snapshot/feed. Drafts and unsubmitted settings survive task switching in memory,
but not page reload. Storage access failures do not prevent chat use.

`loadOlder` merges messages by ID and rejects a cursor that does not advance. Later
snapshots preserve already-loaded older messages. [App.loadOlder](../../web/src/App.svelte)
adjusts scroll offset after prepending so the text being read stays in view.

## Conversation rail

[ConversationRail](../../web/src/ConversationRail.svelte) indexes loaded user messages.
Each mark jumps to its prompt; scroll and resize measurements track the visible
exchange. Arrow keys, Home and End move focus; Enter activates the focused mark.
Long rails keep the current mark visible unless the pointer is browsing another mark.
Reduced-motion settings disable mark transitions and smooth jumps.

[exchanges](../../web/src/conversation-index.ts) groups assistant messages under the
preceding prompt. Prompt excerpts contain at most eight words and 80 UTF-16 code
units; response excerpts contain at most 1200 code units. Loading older history adds
marks without extra rail requests. Empty chats have no rail.

The tooltip shows the prompt in bold and parses the response with markdown-it.
Raw HTML is escaped, images become escaped descriptions, and links retain their
labels without navigation. Only the open preview is rendered; saved source is unchanged.
The renderer's bounded entry point is:

```typescript
export function previewMarkdown(source: string): string {
  return markdown.render(source.slice(0, 1200));
}
```

[App](../../web/src/App.svelte) pauses automatic following during jumps. Returning to
the bottom resumes following. The composer and draft stay in place.

Coverage: [cache admission tests](../../tests/chat-cache.test.mjs),
[cache safety tests](../../server/tests/cache.rs), [history API tests](../../tests/history.test.mjs),
[replay body tests](../../server/tests/replay.rs), [replay HTTP tests](../../tests/replay-bounds.test.mjs),
[state tests](../../web/tests/history.test.ts), and
[browser restoration tests](../../tests/browser/history.spec.ts).
Rail coverage: [preview tests](../../web/tests/conversation-index.test.ts) and
[browser navigation tests](../../tests/browser/conversation-rail.spec.ts).
