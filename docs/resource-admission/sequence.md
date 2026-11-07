# Resource admission sequence

```mermaid
sequenceDiagram
    participant Launcher as Installed launcher
    participant Session as startPair
    participant Host as reservePair
    participant Ledger as ResourceStore / SQLite
    participant Native as Native supervisors
    Launcher->>Session: Start saved pair, with initial limits
    Session->>Host: Admit both guests
    Host->>Host: Read native resource files and attached storage
    Host->>Ledger: Open shared host-user ledger
    Note over Ledger: Existing global limits take precedence
    Host->>Ledger: Reserve pair
    Ledger->>Ledger: BEGIN IMMEDIATE, read recorded inventory
    Ledger->>Host: Refresh recorded files and measure host capacity
    Host-->>Ledger: Allocations, host totals and free space
    Ledger->>Ledger: Check combined reservations
    alt Capacity exceeded
        Ledger->>Ledger: Commit observed retained storage, preserve compute owners
        Ledger-->>Host: Blocking resource
        Host-->>Session: Refuse startup
        Session-->>Launcher: Visible error, neither VM starts
    else Admission allowed
        Ledger->>Ledger: Commit starting reservations and lease
        Ledger-->>Host: Lease token
        Host-->>Session: Reservation owner
        Session->>Native: Start Master and Linux supervisors
        Native-->>Session: Native lifecycle statuses
        Launcher->>Session: Close session
        Session->>Native: Request shutdown and await child exit
        Session->>Host: Settle confirmed stopped roles
        Host->>Ledger: Match lease, stopped or unknown
        Note over Ledger: Disk stays reserved, unknown keeps compute
        Host-->>Session: Settlement committed
        Session-->>Launcher: Exit status
    end
```

The installed launcher is [`start-host.mjs`](../../scripts/start-host.mjs).
[`startPair`](../../scripts/vm/session.mjs) owns process startup and shutdown.
[`reservePair`](../../scripts/vm/resource-admission.mjs) collects host input and
maps confirmed roles back to guest identities.
[`ResourceStore.reserve` and `settle`](../../scripts/vm/resource-store.mjs) own
durable admission and completion. A configuration or database error can also stop
the sequence before native startup.

See [host resource admission](README.md) for the inventory boundary and tests.
