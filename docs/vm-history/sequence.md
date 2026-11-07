# VM chat history sequence

```mermaid
sequenceDiagram
    participant Page
    participant API as Backend API
    participant Chat as Backend chat cache
    participant Store as transcripts (Mac disk)
    participant VM as VM worker

    VM-->>Chat: turn/completed
    Chat->>Store: keep_transcript (background thread)
    Page->>API: GET /api/threads/{id} (VM attached)
    API->>Chat: load, snapshot
    API->>Store: keep_transcript when the chat is idle
    API-->>Page: live snapshot

    Note over VM: VM shuts down
    Page->>API: GET /api/threads/{id}
    API->>Store: load
    API-->>Page: kept messages, vmOffline: true

    Note over VM: VM is ready again
    Page->>API: GET /api/vms/{vm_id}/status
    API-->>Page: ready: true
    Page->>API: GET /api/threads/{id}
    API-->>Page: live snapshot
```

| Participant | Source |
| --- | --- |
| Page | [chat.svelte.ts](../../web/src/chat.svelte.ts) `refreshVmStatus`, `select` |
| Backend API | [api.rs](../../server/src/api.rs) `snapshot`, `delete_chat` |
| Backend chat cache | [chat.rs](../../server/src/chat.rs) `keep_transcript`, `listen_client` |
| transcripts | [transcripts.rs](../../server/src/transcripts.rs) `save`, `load`, `remove` |

Back to [VM chat history on the Mac](README.md).
