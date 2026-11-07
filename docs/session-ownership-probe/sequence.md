# Session ownership probe sequence

```mermaid
sequenceDiagram
    participant Test as Node test owner
    participant Helper as Native lock helper
    participant OS as Host file locks
    participant Contender as Independent descriptor
    Test->>Test: Open disposable private file
    Test->>Helper: Spawn with open descriptor at fd 3
    Helper->>OS: Validate descriptor and acquire exclusive lock
    OS-->>Helper: Acquired
    Helper-->>Test: Exit without explicit unlock
    Contender->>OS: Try exclusive lock on same inode
    OS-->>Contender: Contention
    Test->>OS: Close final shared descriptor
    Contender->>OS: Try exclusive lock again
    OS-->>Contender: Acquired
```

`Test` and `Contender` are the `acquire` and `contend` helpers in
[session-lock.test.mjs](../../tests/experiments/session-lock.test.mjs). `Helper`
is `main` in [session-lock.c](../../scripts/experiments/session-lock.c).
The test waits for the native helper's exit before checking contention.

[Capability](README.md)
