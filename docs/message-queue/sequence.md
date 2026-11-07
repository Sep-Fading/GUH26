# Message queue: sequence

Back to [Message queue](README.md).

```mermaid
sequenceDiagram
    participant U as User
    participant P as Page (chat.svelte.ts)
    participant Q as api/queue.rs
    participant S as queue.rs (SQLite)
    participant C as chat.rs notification loop
    participant X as api.rs send
    participant M as Provider

    U->>P: Enter while a response runs
    P->>Q: POST /queue {text, clientMessageId, model, effort}
    Q->>S: enqueue
    Q-->>P: 201 item, and a queue event
    M-->>C: turn/completed
    C->>Q: turn_ended(thread, status, stopping)
    alt completed and not stopped, or "send now" was asked
        Q->>S: first message
        Q->>X: send with the queued clientMessageId
        X->>M: turn/start
        Q->>S: remove the message
    else failed, interrupted or unknown
        Q->>S: pause with the reason
    end
    Q-->>P: queue event
    U->>P: Resume
    P->>Q: POST /queue/resume
    Q->>S: unpause
    Q->>X: send, if the conversation is idle
```

| Participant | Source |
|---|---|
| Page | [`chat.svelte.ts`](../../web/src/chat.svelte.ts) `enqueue`, `queueAction`, `takeQueue`, `queueable`; [`MessageQueue.svelte`](../../web/src/MessageQueue.svelte) |
| api/queue.rs | [`api/queue.rs`](../../server/src/api/queue.rs) `add`, `install`, `drain`, `resume`, `send_now`, `publish` |
| queue.rs | [`queue.rs`](../../server/src/queue.rs) `enqueue`, `pause_queue`, `queue_first`, `pause_queues_after_restart` |
| notification loop | [`chat.rs`](../../server/src/chat.rs) `listen_client`, `Backend::turn_ended` |

The diagram's syntax was not checked with a renderer.
