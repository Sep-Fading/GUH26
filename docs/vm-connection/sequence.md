# VM connection sequence

```mermaid
sequenceDiagram
    actor User
    participant Wizard as connectSetup
    participant Host as startPair and private broker
    participant Linux as Linux controller
    participant Master as macOS worker
    participant Codex as Guest App Server
    participant Egress as Host CONNECT proxy
    participant OpenAI
    User->>Wizard: Approve paired setup or resume
    Wizard->>Host: Start selected guests and fixed routes
    Host->>Master: Boot guest with setup media
    Host->>Linux: Boot headless controller
    Note over User,Master: Guest account setup, installer and login may require user action
    User->>Wizard: Confirm Master setup complete
    Master->>Host: Worker stream on virtual port 10242
    Linux->>Host: Controller stream on virtual port 10242
    Host->>Host: Pair private Unix streams
    Linux->>Master: Mutual worker authentication
    Master->>Codex: Start validated guest executable
    Linux->>Codex: Initialize RPC through worker
    Wizard->>Linux: Poll health and read account
    opt Guest needs sign-in
        Wizard->>Linux: Start device OAuth
        Linux->>Codex: account/login/start
        Codex->>Master: HTTPS through guest-loopback proxy
        Master->>Egress: CONNECT through virtual port 10243
        Egress->>OpenAI: Pinned public address, end-to-end TLS
        OpenAI-->>Codex: Device login response
        Codex-->>Wizard: Public device URL and code through API
        Wizard-->>User: Display sign-in request
        User->>OpenAI: Complete sign-in
        Wizard->>Linux: Poll completion and verify account
    end
    Wizard->>Wizard: Save installed readiness receipt
    User->>Linux: Chat message through loopback HTTP relay
    Linux->>Codex: turn/start through authenticated worker
    Codex-->>Linux: Item and turn notifications
    Linux-->>User: SSE response
```

Sources: [connectSetup](../../scripts/setup/connect.mjs),
[signIn](../../scripts/setup/auth.mjs), [finishGuest](../../scripts/setup/wizard.mjs),
[startPair](../../scripts/vm/session.mjs), [startBroker](../../scripts/vm/broker.mjs),
[controller startup](../../server/src/main.rs),
[worker](../../server/src/bin/miniclaw-worker.rs),
[egress](../../scripts/vm/egress.mjs).

[VM connection](README.md)

## Master networking

```mermaid
sequenceDiagram
    participant Pair as startPair
    participant Helper as Network helper
    participant Master as Master VM NIC
    participant Filter as Packet policy
    participant Stack as gVisor TCP/IP stack
    participant Public as Public destination
    Pair->>Helper: Start on private Unix socket
    Helper-->>Pair: network_ready
    Pair->>Master: Boot with connected datagram attachment
    Master->>Helper: Ethernet frame
    Helper->>Filter: Validate source and destination
    alt Allowed public TCP/UDP
        Filter->>Stack: Checked frame
        Stack->>Public: Outbound connection or datagram
        Public-->>Stack: Response
        Stack-->>Master: Ethernet response through helper
    else Private, malformed or unsupported traffic
        Filter-->>Helper: Drop frame
    end
```

Sources: [startPair](../../scripts/vm/session.mjs),
[attachment](../../scripts/vm/guest-network.swift),
[helper](../../scripts/vm/network-helper/network.go),
[policy](../../scripts/vm/network-helper/policy.go).

## Readiness and failure

```mermaid
sequenceDiagram
    actor User
    participant Launcher as Installed launcher or setup wizard
    participant Check as waitReady and waitStopped
    participant Pair as Native VM supervisors
    participant API as Guest API
    Launcher->>Pair: startPair
    Launcher->>Check: Await readiness and watch process exits
    loop Until ready or deadline
        Check->>API: GET /api/health
        API-->>Check: Backend and Codex status
        Check-->>Launcher: Report changed wait stage
    end
    alt Backend and Codex connected
        Check-->>Launcher: Ready
        Launcher-->>User: Ready or continue guest OAuth
    else Deadline or unexpected VM exit
        Check-->>Launcher: Reject with startup failure
        Launcher->>Pair: Request graceful shutdown
        opt No stop confirmation after 90 seconds
            Pair->>Pair: Native power-off
            Pair-->>Launcher: Forced stop warning or power-off failure
        end
        Pair-->>Launcher: Confirm native exits
        Launcher-->>User: Report failure
    end
```

Sources: [readiness.mjs](../../scripts/vm/readiness.mjs),
[installed launcher](../../scripts/start-host.mjs),
[connectSetup](../../scripts/setup/connect.mjs).
