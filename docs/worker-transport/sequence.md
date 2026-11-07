# Worker transport sequence

```mermaid
sequenceDiagram
    participant Caller
    participant Client as worker::connect
    participant Worker as worker::serve
    participant Codex as Owned App Server
    Caller->>Client: Private stream and worker key
    Client->>Worker: Protocol marker and controller challenge
    Worker-->>Client: Worker challenge and worker proof
    Client->>Client: Verify worker proof
    Client->>Worker: Controller proof
    Worker->>Worker: Verify controller proof
    alt Invalid proof or handshake timeout
        Worker-->>Caller: Failure without starting Codex
    else Authenticated
        Worker-->>Client: Acknowledge
        Worker->>Codex: Start caller-supplied process
        Client->>Codex: initialize through worker forwarding
        Codex-->>Client: Initialization result
        Client->>Codex: initialized
        Caller->>Client: RPC call
        Client->>Codex: Bounded JSON line through worker
        Codex-->>Client: Replies and notifications through worker
        Client-->>Caller: Matched reply or subscribed event
        Caller->>Client: Stop
        Client->>Worker: Close stream
        Worker->>Codex: Kill and wait for owned child
    end
```

Sources: [connect, serve, exchange and forward](../../server/src/worker.rs),
[Client::attach, transport and stop](../../server/src/rpc.rs).

## Installed guest policy

```mermaid
sequenceDiagram
    participant Controller
    participant Worker as worker::requests
    participant Policy as guest_computer::Policy
    participant Codex as Guest App Server
    Controller->>Worker: Authenticated JSON frame
    Worker->>Policy: route(frame)
    alt Disallowed method or chat parameters
        Policy-->>Worker: Route::Reply with protocol error
        Worker-->>Controller: Error without forwarding
    else Permitted chat request
        Policy->>Policy: Resolve optional Computer Use marker locally
        Policy-->>Worker: Route::Forward
        Worker->>Codex: Validated request
        Codex-->>Controller: Reply through worker
    end
```

Sources: [requests](../../server/src/worker.rs),
[route, permitted and safe_params](../../server/src/guest_computer.rs).

[Worker transport](README.md)
