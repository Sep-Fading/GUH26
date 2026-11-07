# Search sequence

```mermaid
sequenceDiagram
    actor User
    participant Page as ConversationSearch
    participant API as api/search
    participant History as Owned provider history
    participant Index as SQLite search index
    participant Chat as App and Chat
    User->>Page: Open and type
    Page->>API: GET /api/search?q=...
    API->>API: Check account identities
    opt First read or refresh
        API-->>History: Start background listing and page reads
        History-->>Index: Index each message page
    end
    API->>Index: Reconcile completed cached messages and query
    Index-->>API: Bounded ranked matches
    API->>API: Filter removed and unavailable conversations
    API-->>Page: Results and indexing status
    opt Still indexing
        Page->>API: Poll current query
    end
    User->>Page: Choose result
    Page->>Chat: Open conversation and message ID
    Chat->>History: Load conversation and older pages as needed
    History-->>Chat: Messages
    Chat-->>User: Scroll to highlighted message
```

- Page: [`ConversationSearch.svelte`](../../web/src/ConversationSearch.svelte), `read`, `choose`.
- API: [`api/search.rs`](../../server/src/api/search.rs), `search`, `build`, `index_thread`.
- History: [`api.rs`](../../server/src/api.rs), `tasks`, and [`history.rs`](../../server/src/history.rs), `page`.
- Index: [`search.rs`](../../server/src/search.rs), `Index::put`, `Index::query`.
- Chat: [`App.svelte`](../../web/src/App.svelte), `openSearch`, and
  [`chat.svelte.ts`](../../web/src/chat.svelte.ts), `select`, `loadOlder`.

[Conversation search](README.md)
