# Protected mutation

[Security boundary](README.md)

```mermaid
sequenceDiagram
    participant Caller as HTTP caller
    participant Guard as api::guard
    participant API as Protected handler
    participant Codex as Owned App Server
    Caller->>Guard: JSON mutation request
    Guard->>Guard: Validate Host, Origin, fetch metadata, headers
    alt Request disallowed
        Guard-->>Caller: HTTP 403 with security headers
    else Request allowed
        Guard->>API: Route request
        API->>Codex: account/read through authorized
        Codex-->>API: Account state
        alt Not signed in with ChatGPT
            API-->>Guard: HTTP 401
        else Authorized
            API->>API: Validate operation and resource ownership
            API->>Codex: Narrow operation-specific RPC if valid
            Codex-->>API: Result or failure
            API-->>Guard: Public JSON response
        end
        Guard-->>Caller: Response with security headers
    end
```

This diagram shows the common authorization boundary, not a required ordering for
every handler's input parsing. Invalid bodies or identifiers can fail before an
account read. Agent management writes SQLite instead of issuing a chat operation.

| Participant | Source |
| --- | --- |
| Guard | [api.rs](../../server/src/api.rs): `guard`, `router`, `Error::into_response`. |
| Protected handlers | [api.rs](../../server/src/api.rs): `authorized`, `send`, `create`, `save_agent`, `load`. |
| Codex transport | [rpc.rs](../../server/src/rpc.rs): `Client::call`, `refusal`; [runtime.rs](../../server/src/runtime.rs): `launch`. |
