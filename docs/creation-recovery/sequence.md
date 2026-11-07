# Creation recovery sequence

[Capability](README.md)

```mermaid
sequenceDiagram
    participant User
    participant UI as CreationRecovery
    participant API as Recovery routes
    participant Codex as Owned Codex connection
    participant Store as Metadata transaction

    User->>UI: Open Notifications then Creation recovery
    UI->>API: GET /api/creation-recovery
    API->>Codex: account/read
    Codex-->>API: Current sign-in
    API->>Store: Read mandatory fence and pending receipts
    Store-->>API: Counts and confirmation token
    API->>API: Recheck account generation
    API-->>UI: Preview
    User->>UI: Acknowledge permanent denial and confirm
    UI->>API: POST token and confirmed=true
    API->>API: Acquire creation, agent and load gates
    API->>Codex: account/read
    Codex-->>API: Current sign-in
    API->>Store: Begin compaction transaction under account lock
    Store->>Store: Validate fence and match preview token
    alt Snapshot changed or storage invalid
        Store-->>API: Error with original state retained
        API-->>UI: Safe retry or storage error
        UI->>UI: Clear preview and acknowledgement
    else Snapshot matches
        Store->>Store: Intersect prior fence and unknown baselines
        Store->>Store: Write and verify permanent fence
        Store->>Store: Remove only unknown intents and provisional copies
        Store->>Store: Commit all changes together
        Store-->>API: Released slot count
        API-->>UI: Acknowledged recovery
        UI-->>User: Unknown chats remain blocked, no message resent
    end
```

[CreationRecovery.svelte](../../web/src/CreationRecovery.svelte) owns the preview,
checkbox and confirmation. [Notifications.svelte](../../web/src/Notifications.svelte)
provides its entry point. [api.rs](../../server/src/api.rs), `creation_recovery` and
`compact_creations`, handle authorization and gate order. The underlying
[quarantine module](../../server/src/agents/quarantine.rs), `read`, `preview`,
`Store::creation_recovery` and `Store::compact_creations`, owns state validation and
the transaction. [creation.rs](../../server/src/agents/creation.rs), `migrate` and
`Store::check_creation`, applies the mandatory schema and permanent fence to history.
