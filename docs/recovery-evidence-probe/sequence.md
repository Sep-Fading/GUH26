# Recovery evidence probe sequence

```mermaid
sequenceDiagram
    participant Test as Disposable Node harness
    participant Publisher as publishReceipt
    participant Helper as File descriptor helper
    participant FS as Host filesystem
    Test->>Publisher: Bounded synthetic receipt and sync callbacks
    Publisher->>FS: Create private staging file and write JSON
    Publisher->>Helper: Flush staging descriptor
    Helper->>FS: F_FULLFSYNC
    alt File flush failed
        Publisher-->>Test: Error, no final receipt
    else File flush succeeded
        Publisher->>FS: Verify directory identity and create exclusive final link
        Publisher->>FS: Remove staging name
        Publisher->>Helper: Sync directory descriptor
        Helper->>FS: fsync
        alt Directory sync succeeded
            Publisher-->>Test: Published fixture path
        else Sync failed or publisher exited
            Publisher-->>Test: Error or process exit, final file may remain
        end
    end
    Note over Test,FS: Visible fixture files do not authorize resource release
```

`Test` is [recovery-proof.test.mjs](../../tests/experiments/recovery-proof.test.mjs).
`Publisher` is [publishReceipt](../../scripts/experiments/recovery-proof.mjs).
`Helper` is [file-evidence.c](../../scripts/experiments/file-evidence.c), invoked
through explicitly supplied test callbacks. The pure classifier is called
separately with synthetic evidence; it never calls the publisher or filesystem.

[Capability](README.md)
