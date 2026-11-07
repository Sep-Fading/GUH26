# Confirmed chat deletion

```mermaid
sequenceDiagram
    participant User
    participant UI as ConfirmDelete / Chat
    participant API as api::delete_chat
    participant Codex as Codex App Server
    participant Store as Store::remove_chat
    User->>UI: Delete chat
    UI-->>User: Confirmation and consequences
    alt Cancel
        User->>UI: Cancel
    else Confirm
        User->>UI: Confirm
        UI->>API: POST thread delete route
        API->>API: Authorize and exclude pending work
        API->>Codex: Read ownership/status and list descendants
        alt Owned, idle, no descendants
            API->>Codex: thread/delete
            Codex-->>API: Acknowledgement
            API->>Store: Record removal and delete local bindings
            API->>API: Remove cache and wake SSE
            API-->>UI: Success
            UI->>UI: Remove list entry and close confirmation
        else Rejected or unavailable
            API-->>UI: JSON error
            UI-->>User: Keep confirmation open with error
        end
    end
```

- [ConfirmDelete](../../web/src/ConfirmDelete.svelte) and [Chat.deleteChat](../../web/src/chat.svelte.ts): confirmation and guarded UI update.
- [api::delete_chat](../../server/src/api.rs): authorization, ownership, idle and descendant checks, Codex request.
- [Store::remove_chat](../../server/src/projects.rs): transactional local cleanup and removal marker.

[Deletion contracts](README.md)
