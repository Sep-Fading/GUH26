# Conversations and streaming

Rust owns each loaded conversation's response state and replay buffer. Svelte
submits commands over HTTP and receives updates over SSE. Codex owns inference and
the durable transcript. See the [send and reconnect sequences](sequence.md).

## API

These routes in [api.rs](../../server/src/api.rs) require ChatGPT sign-in.

| Route | Input | Result |
| --- | --- | --- |
| `POST /api/threads` | Optional `model`, `effort`, `computerUse`, `agentId`, `projectId` | HTTP 201 with `thread`. |
| `GET /api/threads/{id}` | Task ID | Current snapshot; loads saved history when needed. |
| `POST /api/threads/{id}/messages` | `text`, UUID `clientMessageId`, optional `model` and `effort` | HTTP 202 with `turnId`. |
| `POST /api/threads/{id}/interrupt` | `turnId` | Empty JSON acknowledgement. |
| `GET /api/threads/{id}/events` | Optional `after`; `Last-Event-ID` takes precedence | SSE snapshot or replay, then updates. |

New chat in the UI initially creates only an empty composer. First Send calls
`thread/start`, selects its snapshot/feed, then submits the message. Agent settings
and computer opt-in are applied at creation; their policies are documented under
[agents](../agents/README.md) and [Computer Use](../computer-use/README.md).

The send route requires an already-loaded chat. A caller reopening a saved task must
first load its [snapshot or activation route](../history/README.md).

## Send admission

Text must be nonblank and at most 32768 UTF-8 bytes. The backend validates model
settings and checks active, sending, uncertain, and computer-switch state before
reserving the chat. It repeats the state check after asynchronous model discovery.
Only one response per chat is admitted; independent chats can run concurrently.

`Receipt` stores submitted text, requested settings, and the acknowledged turn ID.
An identical acknowledged request returns that turn ID. Reusing its UUID with
different text/settings, or before acknowledgement, returns 409. Receipts are
process-local, capped at 128 per loaded chat, and are not durable idempotency records.

The backend stores the user message and emits a snapshot before waiting for Codex.
`turn/start` receives `clientUserMessageId`, text input, and empty environments.
This exact fragment of [api::send](../../server/src/api.rs) handles completion
arriving before the HTTP acknowledgement:

```rust
            if !chat.finished.iter().any(|id| id == turn) {
                chat.active = Some(turn.into());
            }
```

An upstream rejection returns `codex_rejected`. Other send failures can leave
`outcomeUnknown: true`, which blocks another send. The browser preserves the draft
on failure and does not automatically retry a message. Its request timeout is
35 seconds; the [RPC timeout](../runtime/README.md) is shorter.

## Names

A conversation is named by its provider, or by its first message (60 characters) while the
provider has no name yet. The user can replace that: `POST /api/threads/{id}/name {name}`
([`api.rs`](../../server/src/api.rs) `name_chat`). The name is one line of at most 120 characters,
trimmed; an empty name gives the conversation back the one it had.

MiniClaw keeps the name itself, in `chat_names` of the metadata database
([`names.rs`](../../server/src/names.rs)), so it works the same for Codex, Claude and VM
conversations and the provider is never told. `tag_context` lays it over every summary, in the
list, the snapshot and an offline VM conversation, and keeps the provider's name beside it as
`providerName`, which is how the page knows to offer "Use original name". Deleting the
conversation deletes the row. On the page the command is the pencil on a conversation's row
([`RenameChat.svelte`](../../web/src/RenameChat.svelte)).

Checked by `tests/conversation-names.test.mjs` with both fake providers and
`tests/browser/rename-chat.spec.ts`.

## Events and snapshots

[Chat](../../server/src/chat.rs) and [API handlers](../../server/src/api.rs) emit
these browser events:

| SSE event | Effect |
| --- | --- |
| `snapshot` | Messages, task summary, pending/active/uncertain state, cursors, approval, computer state. |
| `turn.started` | Set the active `turnId`. |
| `message.delta` | Append `delta` to assistant `itemId`. |
| `message.completed` | Replace that item's partial text with final `text`. |
| `turn.completed` | Clear active/uncertain state; expose status and a fixed error for failure. |
| `turn.error` | Show the send failure and its outcome uncertainty. |
| `computer.updated` | Change only tool-switch state, leaving messages intact. |
| `resync.required` | Mark replay invalid; followed by a current snapshot. |

Each loaded chat has a UUID epoch and increasing sequence. The
[history replay contract](../history/README.md#replay-and-slow-readers) defines the
512-event/2 MiB retention limits, 64-stream admission limit and delivery one event
per body poll. Expired cursors receive a marker without an ID, then a fresh snapshot.
The stream sends keepalive comments every ten seconds and refreshes authorization
every 15 seconds when polled. Identity change or transport loss closes it.

Completed assistant text replaces accumulated deltas. Late deltas cannot change
an already-complete item. The backend remembers 128 completed turn IDs to suppress
their later notifications. These bounds do not impose a total transcript memory limit.

## Stop and UI state

Stop checks the current turn ID, drops its pending approval, marks stopping, and
sends `turn/interrupt`. Its acknowledgement is not completion. The response remains
active until `turn/completed`; cancellation cannot reverse prior actions.

[Chat.select](../../web/src/chat.svelte.ts) closes the previous feed and ignores
late responses/events from an earlier selection. Only the selected chat has a browser
SSE connection; selecting another chat does not cancel backend work.

`Chat.follow` ignores events from replaced feeds as well as previous selections.
After a closed SSE connection, a successful account/task refresh opens a new feed
without clearing the draft, model choices or loaded history:

```ts
          if (this.active && this.feed?.readyState === EventSource.CLOSED) {
            this.follow(this.active, this.selected);
          }
```

Native automatic retries also switch to a fresh snapshot feed. The composer remains
locked until the snapshot arrives; opening a connection alone does not unlock it.
Recovery does not reload the page or resend messages. Visible pages refresh every
15 seconds; focus also triggers a refresh.

[App.svelte](../../web/src/App.svelte) renders messages as escaped plain text.
The textarea starts at one line, grows to four, then scrolls using `field-sizing`
and line-height units in [app.css](../../web/src/app.css). Browsers without
`field-sizing` retain the single-row scrolling fallback. Enter submits; Shift+Enter
and IME composition remain editing actions. The new conversation button is at the top of the
sidebar at every width; on a narrow window the sidebar is a drawer and the top bar has no such
button. Model/effort selectors stay compact
beside the Tools dropdown ([capabilities](../capabilities/README.md)), with Send/Stop at the
right. While a response runs, Enter queues the message instead
([message queue](../message-queue/README.md)), and typing `/` lists
[slash commands](../slash-commands/README.md).

The draft is disabled during response/reconnect uncertainty and initial saved-chat
selection. This prevents selection from erasing text entered while history loads.
Explicit new chats, established-chat refreshes and computer-tool switches leave the
draft editable. [Chat.responding](../../web/src/chat.svelte.ts) owns this condition.
Auto-scroll follows new text only when the reader is
within 100 pixels of the bottom. Error banners use `role="alert"`; response status
uses a polite live region. Desktop uses grouped task navigation; mobile has a task
selector and navigation drawer. [Projects](../projects/README.md) group chats and
select available agents. [Deletion](../deletion/README.md) requires confirmation.

Coverage: [conversation tests](../../tests/conversation.test.mjs),
[stream contracts](../../tests/contracts.test.mjs),
[chat state tests](../../web/tests/chat.test.ts),
[browser chat tests](../../tests/browser/chat.spec.ts), and
[composer tests](../../tests/browser/composer.spec.ts).
