# Notifications

MiniClaw records failed responses and unexpected Codex transport disconnections.
The global Notifications control shows retained notices, their read state and a
button to open an affected conversation. "Clear read" removes the current account's
read notices from storage. [Sequence](sequence.md).

## Failure capture

The notification listener in [chat.rs](../../server/src/chat.rs) captures a failed
`turn/completed` for a loaded chat whose active turn matches, or whose send is
awaiting acknowledgement. Completed turn IDs suppress repeated events. The store
also deduplicates by account, conversation and turn ID. Capture uses fixed messages;
upstream error text, prompts, screenshots, file paths and tool output are not copied.

[Client::stop](../../server/src/rpc.rs) sets shared shutdown intent before signaling
the transport. The transport includes that intent in its disconnect event; an
intentional stop does not produce a failure notice. Controller teardown waits for
the backend event listener to finish before dropping the router. The listener also
observes raw transport completion, so a connection closed before listener registration
cannot leave the drain waiting forever. Its completion observer holds no RPC
shutdown sender, so dropping the last backend/client still stops an idle child. Unexpected disconnections have
one event identity per backend session. Capture requires a known signed-in account.

## Persistence and identity

[notifications.rs](../../server/src/notifications.rs) extends the existing private,
identity-checked [agent store](../../server/src/agents.rs). It stores notice identity,
account key, failure kind, optional conversation ID, time and read state. The account
key is SHA-256 of the account email already supplied by Codex. Missing or empty
identity is rejected. The digest separates records; it is not encryption or proof
of account identity independent of Codex.

`Store::record_notice` uses a unique account/event constraint. A repeated failure
leaves the original notice and read state intact:

```rust
            tx.execute("INSERT INTO notifications (id, owner, event, kind, thread_id, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(owner, event) DO NOTHING",
                params![Uuid::new_v4().to_string(), owner, event, kind, thread,
                    chrono::Utc::now().timestamp()]).map_err(Issue::storage)?;
```

The same transaction retains the newest 1,000 notices across accounts. Older read
and unread notices are removed. The list is therefore recent failure history, not
an audit log. Read state survives backend restarts. Stored conversation IDs open
through the ordinary ownership-checked chat route; a removed conversation produces
that route's normal error.

## API and interface

The [API router](../../server/src/api.rs) lists notices at `GET /api/notifications`
and exposes authenticated mutation routes:

```rust
        .route("/api/notifications/read-all", post(read_all_notifications))
        .route("/api/notifications/clear-read", post(clear_read_notifications))
        .route("/api/notifications/{id}/read", post(read_notification))
```

GET refreshes Codex account state and checks its generation before returning the
account's notices. POST marks only an owned notice read under the existing origin
and request guards. A storage write failure sets a process-local `storageError`
flag; the list exposes it so the interface can report incomplete failure history.
The flag clears on account invalidation or change. Storage errors retain the existing
bounded category logging, without SQL or paths.

`clear_read_notifications` checks sign-in, captures the account generation and holds
the account lock through the storage operation. `Store::clear_read_notices` deletes
only that account's read rows and returns their count as `cleared`; repeating the
request with nothing left returns zero. It uses the existing Origin and mutation-header
guards. A failed delete returns a safe storage error and leaves the rows intact.

The deletion in [notifications.rs](../../server/src/notifications.rs) is:

```rust
            db.execute("DELETE FROM notifications WHERE owner = ?1 AND read = 1", [owner])
                .map_err(Issue::storage)
```

[Notifications.svelte](../../web/src/Notifications.svelte) polls every ten seconds
while the page is visible, and refreshes when opened or explicitly requested. It
clears its state on account changes, discards older requests and waits for POST
acknowledgement before marking a notice read. Failed reads remain retryable with
fixed public messages. Malformed success payloads are rejected before updating the
list or formatting dates. The [App](../../web/src/App.svelte) hook delegates navigation
to `Chat.select`, which preserves the conversation's unsent draft.

"Clear read" sits beside "Mark all as read". It is disabled while loading or saving,
or when no notice is read. `clearRead` invalidates pending refreshes, waits for the
server's acknowledgement and removes read rows only if the account has not changed.
A failed request keeps the list and shows "Could not clear read notifications. Try again."

## Verification

The 2026-09-22 clear-read tests cover mixed read/unread lists, account separation,
restart persistence, repeated clears, Origin/header/sign-in guards and failed deletes.
All 10 notification API and 10 component cases pass. A pending clear cannot change the
next account's list. A deliberate mutation that removed unread rows failed the
preservation assertion; the correct filter was restored. Both sequence diagrams render.
All four notification browser cases pass at desktop/mobile sizes, including the empty
state after clearing and reloading. The isolated full browser suite passed 299 cases,
with one skipped; Rust passed 213 with one ignored, and frontend unit tests passed 88.

[Backend acceptance](../../tests/notifications.test.mjs) covers persistence,
deduplication, fixed summaries, foreign origins, account separation, missing identity,
stale account reads, normal shutdown, unexpected runtime exit, the retention bound
and storage failure reporting. All seven cases passed with disposable synthetic peers.
[Component tests](../../web/tests/notifications.test.ts) cover read acknowledgement,
failed writes, an account change during a pending fetch, clearing notices after
an unauthorized refresh, and malformed response data; all seven passed.
[Browser acceptance](../../tests/browser/notifications.spec.ts) checks conversation
navigation, draft retention, read state after reload and Escape focus restoration; both desktop and mobile
cases passed.

[Backend lifetime tests](../../server/tests/backend_lifetime.rs) cover a listener
registered after the connection has already closed and a dropped backend stopping its
child. The controller-exit case was removed with the retired Linux controller. The failure-drain regression failed before the barrier; all
eight startup tests passed after it. A separate drop-cleanup regression reproduced
an observer retaining the idle child, then passed with the lightweight observer.
