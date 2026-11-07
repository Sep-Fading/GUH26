# Direct agent chat

This sequence covers an activated text-only profile and unchanged authentication.
Profile editing and stored fields are described in the [capability page](README.md).

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant API as api module
    participant Store as agents Store
    participant Codex as Owned App Server
    participant Cache as Rust Chat cache
    UI->>API: POST threads with agentId
    API->>API: Acquire admission lock and authorize
    API->>Store: prepare agent and active skill copies
    Store-->>API: Current profile and skill revisions
    alt Inactive or unsupported
        API-->>UI: Error without starting a thread
    else Eligible profile
        API->>Codex: List existing chat IDs
        Codex-->>API: Bounded baseline
        API->>Store: reserve provisional bindings and baseline
        Store-->>API: Commit before RPC
        API->>Codex: thread/start with restricted policy and instructions
        Codex-->>API: Thread and accepted model settings
        API->>Store: created records returned ID
        Store-->>API: Commit receipt
        API->>Store: check_creation moves bindings to returned ID
        Store-->>API: Atomic binding commit
        API->>Cache: Cache tagged summary
        API-->>UI: HTTP 201 with thread
        UI->>API: POST messages
        API->>Store: binding with require_active
        Store-->>API: Pinned profile or error
        alt Agent remains active
            API->>Codex: turn/start
            Codex-->>API: Turn acknowledgement
            API-->>UI: HTTP 202 receipt
            Codex--)Cache: Item and turn notifications via backend listener
            Cache--)UI: SSE deltas and completion
        else Agent deactivated
            API-->>UI: HTTP 409 agent_inactive
        end
    end
```

Codex notifications can arrive before the turn acknowledgement. The existing event
buffer handles that ordering; HTTP and SSE delivery are independent. A failed binding
move leaves the receipt for retry on listing or loading, without another creation.
Without a returned ID, unfamiliar history stays blocked. Thread creation and the local
database are not one transaction.

## Storage startup

```mermaid
sequenceDiagram
    participant Main as Backend startup
    participant Store as Store::open
    participant Identity as identity module
    participant Files as Private database and marker
    Main->>Store: Open metadata path
    Store->>Identity: Read marker, verify database identity and tables
    Identity->>Files: Read retained state
    alt Missing or mismatched retained state
        Identity-->>Store: Storage error
        Store-->>Main: Stop startup
    else New or valid legacy database
        Store->>Files: Initialize schema
        Store->>Identity: Seal database and sync marker
        Identity-->>Store: Success
        Store-->>Main: Ready store
    else Matching retained identity
        Store-->>Main: Ready store
    end
```

Sources: [Store::open](../../server/src/agents.rs),
[identity](../../server/src/agents/identity.rs).
After identity checks, [creation::migrate](../../server/src/agents/creation.rs)
creates the journal and admission records once or verifies their tables exist.

## Source map

| Participant | Implementation |
| --- | --- |
| Svelte Chat | [chat.svelte.ts](../../web/src/chat.svelte.ts): `chooseAgent`, `create`, `send`, event subscription. |
| API | [api.rs](../../server/src/api.rs): `create`, `send`, `save_agent`, `load`. |
| Store | [agents.rs](../../server/src/agents.rs): `prepare`, `reserve`, `binding`, `save`; [creation.rs](../../server/src/agents/creation.rs): receipts and recovery. |
| Owned App Server | [runtime.rs](../../server/src/runtime.rs): `launch`; RPC transport in [rpc.rs](../../server/src/rpc.rs). |
| Rust Chat cache | [chat.rs](../../server/src/chat.rs): notification listener and per-chat state. |
