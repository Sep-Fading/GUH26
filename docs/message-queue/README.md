# Message queue

While a response runs, further messages wait in a queue the backend keeps for that
conversation. They leave one per turn, in order. One turn at a time per conversation stays the
rule; a direct send during a response still answers 409 `busy`.

Sequence: [sequence.md](sequence.md).

## What is stored

Two tables in the metadata database ([`queue.rs`](../../server/src/queue.rs)):
`message_queue` holds each message with its position, the `clientMessageId` it was queued
under, and the model and effort chosen when it was queued. `message_queue_state` holds whether
a conversation's queue is paused and why. A queue holds 20 messages; a message holds 32 KiB,
like a direct send. Deleting a conversation deletes its queue
([`projects.rs`](../../server/src/projects.rs) `remove_chat`).

## Routes

| Route | Effect |
|---|---|
| `GET /api/threads/{id}/queue` | the queue |
| `POST /api/threads/{id}/queue` | queues a message (`text`, `clientMessageId`, optional `model`, `effort`). The same id with the same content is queued once; with other content it answers 409 `queue_conflict`. A full queue answers 409 `queue_full` |
| `POST .../queue/{item}` | replaces its words |
| `POST .../queue/{item}/delete`, `.../queue/clear` | removes one, or all |
| `POST .../queue/{item}/send-now` | makes it next and interrupts the running response |
| `POST .../queue/pause`, `.../queue/resume` | the user's own pause, and resume |

The conversation's HTTP snapshot carries `queue`, and a `queue` event follows every change.

## When a message leaves

The notification loop calls `Backend::turn_ended` when a conversation's turn ends
([`chat.rs`](../../server/src/chat.rs)); the API layer installs the handler
([`queue.rs`](../../server/src/api/queue.rs) `install`).

```rust
let reason = match status.as_str() {
    _ if asked => None,
    "completed" if !stopping => None,
    "failed" => Some("failed"),
    "interrupted" | "completed" => Some("stopped"),
    _ => Some("unknown"),
};
```
[`api/queue.rs`](../../server/src/api/queue.rs) `install`

With no reason, `drain` sends the first message. It goes through the same `send` the page uses,
with the id it was queued under, so every guard of a send applies and a repeated attempt
starts no second turn. The message is removed from the queue only after that send was
accepted. `busy` leaves it queued for the next turn; any other refusal pauses the queue with
`refused`. One drain runs at a time.

A queue that is paused sends nothing until Resume. Reasons: `failed`, `stopped`, `unknown`
(the turn's outcome is not known), `refused`, `user`, and `restart`: `main.rs` pauses every
queue that holds something when the backend starts, because no turn of that run has ended.
Queueing into an idle conversation with a running queue sends at once. An emptied queue is no
longer paused.

"Send now" moves the message to the front, clears a pause, and interrupts the running turn.
The conversation is remembered in `sending_now`, so that interrupted turn does not pause the
queue. The message is sent when the turn has really ended, never beside it.

## Versions

Each queue view carries `version`, taken before its rows are read
([`queue.rs`](../../server/src/queue.rs) `next_version`): the start time in microseconds, then
one more per read. An HTTP answer can reach the page after a later event; the page keeps the
view with the larger version ([`chat.svelte.ts`](../../web/src/chat.svelte.ts) `takeQueue`).

## The page

The message box stays open while a known response runs and nothing about it is in doubt
(`Chat.queueable`): not while a send is unacknowledged, the page is reconnecting, the outcome
is unknown, or Stop was pressed. Enter queues. With words in the box the main button becomes
"Queue message" and Stop moves beside it. [`MessageQueue.svelte`](../../web/src/MessageQueue.svelte)
lists the queue above the composer with Send now, Edit and Remove per message, Pause or Resume,
Clear, and the reason for a pause in words.

## Tests

[`message-queue.test.mjs`](../../tests/message-queue.test.mjs) (6, fake Codex) and
[`message-queue.spec.ts`](../../tests/browser/message-queue.spec.ts) (3).

Not observed with a real model. Not covered: a Claude conversation's queue, and a queue in a VM
conversation whose VM is switched off while messages wait (the send would be refused and the
queue paused with `refused`, which is untested).
