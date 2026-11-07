# Notification sequence

[Capability](README.md)

```mermaid
sequenceDiagram
    participant Codex as Codex transport
    participant Backend as Backend notification listener
    participant Store as Private metadata store
    participant Controller as Controller lifetime
    participant API as Notification API
    participant UI as Notifications component
    participant Chat as Chat.select

    Codex->>Backend: Failed turn completion or unexpected disconnect
    Backend->>Backend: Check known account and eligible failure
    Backend->>Store: Persist fixed failure kind and event identity
    Store->>Store: Deduplicate and retain newest 1000
    alt Persistence fails
        Store-->>Backend: Storage error
        Backend->>Backend: Set storageError flag
    else Persistence succeeds
        Store-->>Backend: Committed notice
    end
    opt Worker connection ended
        Controller->>Backend: Await event listener completion
        Backend-->>Controller: Final store operation completed
        Controller->>Controller: Return for supervisor restart
    end
    UI->>API: GET /api/notifications
    API->>API: Refresh account and capture generation
    API->>Store: List account notices
    Store-->>API: Retained notice fields
    API->>API: Recheck generation
    API-->>UI: Safe messages, read state, storageError
    UI->>API: POST /api/notifications/{id}/read
    API->>API: Check account and origin
    API->>Store: Mark owned notice read
    Store-->>API: Acknowledged
    API-->>UI: ok
    UI->>UI: Update read state after acknowledgement
    UI->>Chat: Open affected conversation when requested
    Chat->>Chat: Preserve draft and load through normal API
```

The transport and intentional-stop marker are in
[rpc.rs](../../server/src/rpc.rs), `Client::transport` and `Client::stop`.
The listener is in [chat.rs](../../server/src/chat.rs), `Backend::new`.
Persistence and fixed messages are in [notifications.rs](../../server/src/notifications.rs),
`Store::response_failure`, `Store::runtime_failure`, `Store::record_notice` and
`Store::notices`. The routes and generation checks are in
[api.rs](../../server/src/api.rs), `notifications` and `read_notification`.
The interface is [Notifications.svelte](../../web/src/Notifications.svelte);
[App.svelte](../../web/src/App.svelte) connects its conversation action to
[`Chat.select`](../../web/src/chat.svelte.ts).

[main](../../server/src/main.rs) stops every client during shutdown and awaits
`Client::drain_events` for each before returning. `Client::stop` and `Client::closed` continue to wait
only for transport cleanup, keeping the listener independent of its own drain.

The alternate [remote controller guard](../../server/src/remote.rs) also drains after
transport closure. [Main](../../server/src/main.rs) drains during local graceful
shutdown. `Completion` observes the raw transport watch without owning a `Client`.

## Clearing read notices

```mermaid
sequenceDiagram
    actor User
    participant UI as Notifications component
    participant API as Notification API
    participant Store as Private metadata store

    User->>UI: Clear read
    UI->>UI: Disable writes and invalidate pending refreshes
    UI->>API: POST /api/notifications/clear-read
    API->>API: Check origin, header, account and generation
    API->>Store: Delete read notices for this account
    alt Storage succeeds
        Store-->>API: Number removed
        API-->>UI: cleared
        UI->>UI: Remove read rows if account is unchanged
    else Storage fails
        Store-->>API: Storage error
        API-->>UI: Safe error
        UI->>UI: Keep rows and show retryable error
    end
```

The button and account guard are in
[Notifications.svelte](../../web/src/Notifications.svelte), `clearRead`.
The route is [api.rs](../../server/src/api.rs), `clear_read_notifications`;
the delete is [notifications.rs](../../server/src/notifications.rs),
`Store::clear_read_notices`.
