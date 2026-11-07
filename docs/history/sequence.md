# Restore a saved task

[History contracts](README.md)

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant API as api::load
    participant Codex as Owned App Server
    participant Buffer as Data.loading
    participant Cache as Rust Chat cache
    UI->>API: GET task snapshot
    API->>API: Acquire load gate and authorize
    alt Task already cached
        API->>Cache: Pin cache for caller and read snapshot
        Cache-->>UI: Snapshot through HTTP handler
    else Task not cached
        API->>Codex: thread/read metadata
        Codex-->>API: Saved task
        API->>API: Validate ownership and read agent binding
        API->>Cache: Reserve slot, evict eligible idle history or reject if full
        API->>Buffer: Begin buffering notifications
        API->>Codex: thread/resume with restricted policy
        Codex-->>API: Resumed task
        API->>Codex: thread/turns/list
        Codex-->>API: Recent full-item turns
        Codex--)Buffer: Notifications received during loading
        API->>API: Validate history and reject active partial history
        API->>Buffer: Take buffered frames
        Buffer-->>API: Frames or overflow state
        API->>Cache: Publish history, apply frames and pin cache if valid
        Cache-->>UI: Snapshot through HTTP handler
    end
```

Buffering begins before resume and continues until publication. Its arrow shows
possible notification delivery, not a required event after the history response.
Any validation failure removes the loading entry without publishing a usable chat.

| Participant | Source |
| --- | --- |
| Svelte Chat | [chat.svelte.ts](../../web/src/chat.svelte.ts): `select`, `loadOlder`. |
| Loader | [api.rs](../../server/src/api.rs): `load`, `snapshot`, `activate`, `older`. |
| History reads | [history.rs](../../server/src/history.rs): `summary`, `page`. |
| Buffer | [history.rs](../../server/src/history.rs): `Loading::push`; [chat.rs](../../server/src/chat.rs): notification listener. |
| Chat cache | [chat.rs](../../server/src/chat.rs): `Data::make_room`, `Chat::pin`, `notify`, `snapshot`. |

## Replay after a slow reader

```mermaid
sequenceDiagram
    participant Browser
    participant API as api::events
    participant Cache as Chat cache
    Browser->>API: Open SSE with last received event ID
    API->>API: Acquire stream permit or return 503
    API->>Cache: Load and pin chat
    loop Each HTTP body poll
        API->>Cache: Check account generation and runtime
        alt Cursor still retained
            Cache-->>Browser: One next replay event through SSE
        else Cursor missing or expired
            API-->>Browser: resync.required without event ID
            Note over API: No snapshot retained while reader is paused
            Browser->>API: Next body poll
            API->>Cache: Recheck account and runtime, select current snapshot
            Cache-->>Browser: Full snapshot with current event ID through SSE
        end
    end
    Browser->>API: Close stream
    API->>API: Release stream permit and cache lease
```

Sources: [api::events](../../server/src/api.rs) owns stream admission and polling;
[Chat::emit](../../server/src/chat.rs) bounds replay payload. Account or runtime
loss closes the stream before its next event. A fresh connection without a cursor
starts with the current snapshot. Neither resynchronization nor closing a stream
resends a message.

## Navigate loaded exchanges

```mermaid
sequenceDiagram
    participant Chat as Chat.messages
    participant Rail as ConversationRail
    participant Preview as conversation-index
    participant App as App scroll container
    Chat-->>Rail: Loaded history or streaming text
    Rail->>Preview: exchanges(messages)
    Preview-->>Rail: Bounded prompt and response excerpts
    alt Hover or keyboard focus
        Rail->>Preview: previewMarkdown(response)
        Preview-->>Rail: Inert HTML for Bits UI tooltip
    else Activate a mark
        Rail->>App: onjump pauses automatic following
        Rail->>App: scrollTo prompt offset
        App-->>Rail: Scroll and resize notifications
        Rail->>Rail: Measure current prompt
    end
```

Sources: [ConversationRail](../../web/src/ConversationRail.svelte),
[preview grouping and rendering](../../web/src/conversation-index.ts),
[App](../../web/src/App.svelte). Navigation does not call the backend.
