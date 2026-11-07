# API fixture lifecycle

[Test architecture](README.md)

```mermaid
sequenceDiagram
    participant Test as Node test
    participant Harness as launchApp
    participant Rust as Real Rust backend
    participant Peer as Scripted Codex peer
    Test->>Harness: launchApp with overrides
    Harness->>Harness: Create disposable homes and scenario
    Harness->>Rust: Spawn with fixture binary and loopback port
    Rust->>Peer: Spawn stdio App Server fixture
    Rust->>Peer: initialize and config/read
    Peer-->>Rust: Scripted replies
    Harness->>Rust: Poll health
    Rust-->>Harness: HTTP 200
    Harness->>Peer: Wait for control socket
    Harness-->>Test: App request/control helpers
    Test->>Rust: HTTP request
    Rust->>Peer: Operation-specific RPC
    Peer-->>Rust: Scripted result
    Rust-->>Test: Public HTTP response
    Test->>Harness: inspect or emit
    Harness->>Peer: Unix socket control
    Peer-->>Harness: Observed requests or control acknowledgement
    Harness-->>Test: Fixture state
    Test->>Harness: close
    Harness->>Rust: Stop child
    Harness->>Peer: Cleanup and shutdown if needed
    Harness->>Harness: Remove disposable directory
```

Tests can emit notifications before acknowledgements or hold replies to exercise
races. This diagram shows a simple successful request, not every scripted ordering.

| Participant | Source |
| --- | --- |
| Tests | [tests](../../tests): API and contract suites. |
| Harness | [harness.mjs](../../tests/support/harness.mjs): `peerFixture`, `launchApp`, `stopChild`, `control`. |
| Rust backend | [main.rs](../../server/src/main.rs), [rpc.rs](../../server/src/rpc.rs), [api.rs](../../server/src/api.rs). |
| Scripted peer | [fake-codex.mjs](../../tests/support/fake-codex.mjs), [fixtures.mjs](../../tests/support/fixtures.mjs). |
