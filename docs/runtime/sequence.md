# Startup sequence

[Runtime and configuration](README.md)

```mermaid
sequenceDiagram
    participant Launch as start.mjs
    participant Build as Vite and Cargo
    participant Rust as main
    participant RPC as runtime / rpc
    participant Codex as Owned App Server
    participant UI as Browser
    Launch->>Build: Build assets and backend
    Build-->>Launch: Assets and executable path
    Launch->>Rust: Spawn backend
    Rust->>Rust: Lock data directory and open agent storage
    Rust->>Rust: Bind loopback and canonicalize chat directory
    Rust->>RPC: launch
    RPC->>Codex: Spawn with stdio and process policy
    RPC->>Codex: initialize
    Codex-->>RPC: Initialization result
    RPC->>Codex: initialized, then config/read
    Codex-->>RPC: Effective provider and MCP configuration
    RPC->>RPC: Validate configuration
    RPC-->>Rust: Client or launch failure
    Rust->>Rust: Create Backend and serve Axum router
    UI->>Rust: GET frontend assets and api/health
    Rust-->>UI: UI and health status
```

If inherited MCP entries remain enabled, `runtime::launch` stops the first process
and retries once with those entries disabled. Invalid provider settings return a
launch failure. Storage or bind errors stop startup before serving HTTP.

| Participant | Source |
| --- | --- |
| Launcher/build | [start.mjs](../../scripts/start.mjs): `start`, `run`, `stop`. |
| Rust startup | [main.rs](../../server/src/main.rs): `main`, `instance`. |
| Runtime/RPC | [runtime.rs](../../server/src/runtime.rs): `launch`, `official`; [rpc.rs](../../server/src/rpc.rs): `Client::spawn`. |
| Browser | [Connection.svelte](../../web/src/Connection.svelte): `check`; [api.rs](../../server/src/api.rs): `router`, `health`. |

## Notification capacity and failure

```mermaid
sequenceDiagram
    participant Codex as App Server
    participant Reader as RPC reader
    participant Queue as Events byte budget
    participant Backend as Backend listener
    participant Store as Notice storage
    participant Startup as Startup/controller
    Codex-->>Reader: Valid notification JSON line
    Reader->>Queue: Try to reserve raw JSON bytes
    alt Capacity available
        Queue-->>Backend: Shared notification envelope
        Backend->>Backend: Decode and apply while holding byte lease
        Backend->>Queue: Release lease when last copy drops
    else Payload budget exhausted
        Queue->>Queue: Latch terminal failure and refuse ordinary producers
        Queue-->>Reader: Signal failure without waiting
        Reader->>Reader: Reject pending calls and newly selected work
        Reader-->>Backend: One bounded terminal notification
        Reader->>Codex: Close transport and reap owned child
        Reader-->>Backend: Publish authoritative completion reason
        Backend->>Store: Persist safe failure notice when account identity exists
        Store-->>Backend: Commit result
        Backend->>Backend: Mark pending work uncertain and clear approvals/account
        Backend-->>Startup: Event drain complete
    end
```

The listener may process the terminal notification before child cleanup completes.
The completion watch supplies the same reason if normal broadcast delivery was lost
or the listener subscribed after termination. Neither route resends a message.
Generated approval-resolution events pass through the same byte admission.

Sources: [rpc.rs](../../server/src/rpc.rs) `Events::publish`, `finish`,
`Client::transport`, `Completion::closed`; [chat.rs](../../server/src/chat.rs)
`Backend::new`; [main.rs](../../server/src/main.rs) `main`;
[remote.rs](../../server/src/remote.rs) `guard`;
[notifications](../notifications/README.md) for durable safe notices.
