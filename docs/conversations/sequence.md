# Conversation sequences

[Conversation contracts](README.md)

## Send and completion

This path assumes an authorized, loaded idle chat and a new message ID. The SSE
connection already exists. HTTP acknowledgements and event delivery are independent.

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant API as api::send
    participant Cache as Rust Chat
    participant Codex as Owned App Server
    UI->>API: POST messages
    API->>API: Authorize, validate input, profile, and model
    API->>Cache: Check receipt and reserve sending state
    API->>Cache: Store user message and emit snapshot
    Cache--)UI: SSE snapshot
    API->>Codex: turn/start
    Codex--)Cache: turn/started and item notifications via listener
    Cache--)UI: SSE turn.started and message.delta
    Codex-->>API: Turn acknowledgement
    API->>Cache: Store receipt and retain completion state
    API-->>UI: HTTP 202 with turnId
    Codex--)Cache: item/completed and turn/completed
    Cache--)UI: SSE final text and completion
    UI->>UI: Clear response state and refresh usage
```

Completion can also arrive before the acknowledgement. The backend finished-turn
list and frontend finished-turn set prevent that late reply from reopening a turn.

## Reconnect

```mermaid
sequenceDiagram
    participant UI as Browser EventSource
    participant API as api::events
    participant Cache as Rust Chat
    UI->>API: GET events with Last-Event-ID
    API->>API: Load task, authorize, subscribe to changes
    API->>Cache: Check cursor against retained events
    alt Cursor retained
        Cache-->>API: Events after cursor
        API--)UI: Replay events
    else Cursor expired or foreign
        Cache-->>API: Current snapshot and cursor
        API--)UI: resync.required
        API--)UI: snapshot
    end
    Cache--)API: Change notification
    API--)UI: Later queued events
```

A connection without a cursor begins with a snapshot. A stream closes on account
generation change or loss of the owned runtime.

## Browser recovery

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant Feed as Browser EventSource
    participant API as MiniClaw API
    Feed-->>UI: error
    UI->>UI: Lock composer, retain draft and history
    alt Feed permanently closed
        UI->>API: Refresh account and task list
        API-->>UI: Authorized account and retained task
        UI->>UI: follow selected task
    else Native retry opens again
        Feed-->>UI: open
        UI->>UI: follow selected task
    end
    UI->>Feed: Replace feed without replay cursor
    Feed->>API: GET events
    API-->>UI: Current snapshot through SSE
    UI->>UI: Apply snapshot and end reconnect state
```

Recovery does not send a message or navigate the page.

| Participant | Source |
| --- | --- |
| Svelte Chat / EventSource | [chat.svelte.ts](../../web/src/chat.svelte.ts): `send`, `select`, `follow`, `refresh`, `apply`, `append`, `stop`. |
| API | [api.rs](../../server/src/api.rs): `send`, `receipt`, `events`, `interrupt`. |
| Rust Chat | [chat.rs](../../server/src/chat.rs): `notify`, `emit`, `snapshot`, `Backend::new`. |
| Codex | [rpc.rs](../../server/src/rpc.rs): reply/notification routing. |
