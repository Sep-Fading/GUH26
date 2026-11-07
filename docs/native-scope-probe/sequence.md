# Native scope probe sequence

```mermaid
sequenceDiagram
    participant Harness as Synthetic test harness
    participant Policy as NativeScopePolicy
    participant Adapter as Injected fake adapter
    Harness->>Policy: Payload and authentication tag
    Policy->>Policy: Validate schema, document and generation
    Policy->>Adapter: Read session and process identity
    Adapter-->>Policy: Synthetic session state
    alt Request refused
        Policy-->>Harness: Error without dispatch
    else Request admitted
        Policy->>Policy: Reserve sequence and in-flight gate
        Policy->>Adapter: Perform one fixed operation
        Adapter-->>Policy: Synthetic observation or failure
        alt Successful call and generation still enabled
            Policy->>Adapter: Recheck current identity
            Adapter-->>Policy: Current session state
            Policy->>Policy: Check identity and output bounds
        end
        alt All result checks pass
            Policy-->>Harness: Bounded observation
        else Revoked, changed or failed
            Policy-->>Harness: No observation, uncertain mutation when applicable
        end
    end
    Note over Harness,Adapter: No native adapter instance or GUI action in these tests
```

`Harness` and `Adapter` are in
[support/native-scope.swift](../../tests/experiments/support/native-scope.swift).
`Policy` is in
[native-scope-policy.swift](../../scripts/experiments/native-scope-policy.swift).
The native adapter is compiled alongside this harness without being instantiated.

[Capability](README.md)
