# Computer Use sequences

These diagrams trace the implementation described in the [capability page](README.md).
They separate configuration changes from approval of a later computer action.

## Existing-chat tool switch

This path assumes a loaded idle chat, a changed mode, unchanged authentication,
and no pending recovery. Early validation failures return before archive.

```mermaid
sequenceDiagram
    participant UI as Svelte chat state
    participant API as api::set_computer
    participant Chat as Rust Chat cache
    participant Disk as recovery module
    participant Codex as Owned App Server
    UI->>API: POST computer with enabled
    API->>API: load and authorized
    API->>Chat: Reserve chat and emit computer.updated
    Chat--)UI: SSE changing state
    API->>Codex: check_switch reads metadata and descendants
    Codex-->>API: Eligible idle task
    API->>Disk: mark task ID and sync
    Disk-->>API: Recovery record saved
    API->>Codex: thread/archive
    Codex-->>API: Acknowledgement
    API->>Codex: thread/unarchive
    alt Restoration fails
        Codex-->>API: Error
        API->>Chat: Record computer_error and emit update
        Chat--)UI: SSE error state
        API-->>UI: HTTP error
    else Restoration confirmed
        Codex-->>API: Restored task ID
        API->>Disk: clear recovery record
        API->>API: resume_params and optional computer::configure
        API->>Codex: thread/resume with selected tool policy
        alt Resume fails
            Codex-->>API: Error
            API->>Chat: Keep confirmed mode and record error
            Chat--)UI: SSE error state
            API-->>UI: HTTP error
        else Resume succeeds
            Codex-->>API: Valid task
            API->>Chat: Confirm mode and emit computer.updated
            Chat--)UI: SSE confirmed state
            API-->>UI: Preserved snapshot
        end
    end
```

HTTP replies and SSE delivery travel independently; their arrival order at the
browser is not guaranteed. Both update only tool state through `applyComputer`.
Retry and restart recovery check archived location before deciding whether to
unarchive. Account-change and archive-error branches are detailed in
`set_computer`; they are omitted here to keep the normal switch readable.

## App approval during a turn

This path begins when Codex forwards a native-helper approval request. It does not
imply that every tool invocation requests a new approval.

```mermaid
sequenceDiagram
    participant Codex as Owned App Server
    participant RPC as rpc::Client
    participant Backend as Backend request listener
    participant Chat as Rust Chat cache
    participant UI as Svelte approval UI
    participant API as api::decide
    Codex->>RPC: mcpServer/elicitation/request
    RPC->>Backend: ServerRequest with answer channel
    Backend->>Backend: Approval::new and task/turn checks
    alt Unsupported or ineligible
        Backend->>RPC: Drop unanswered request
        RPC-->>Codex: Decline
    else Pending approval accepted
        Backend->>Chat: Store approval and emit snapshot
        Chat--)UI: SSE snapshot with approval
        UI->>API: POST approval action
        API->>Chat: Check ID, age, active turn, and Stop state
        alt Valid decision
            API->>RPC: Answer channel with accept or decline
            RPC-->>Codex: JSON-RPC result
            API->>Chat: Emit snapshot without pending approval
            Chat--)UI: SSE snapshot
            API-->>UI: Empty JSON acknowledgement
        else Invalid decision
            API-->>UI: Error
        end
    end
```

The answer-channel delivery, RPC reply, and browser snapshot are asynchronous; the
diagram does not require the RPC reply to arrive before the browser update. If no
valid answer arrives, the RPC wait declines on timeout or channel closure. Stop
clears the pending approval and calls `turn/interrupt` independently of that wait.

## Guest configuration

```mermaid
sequenceDiagram
    participant API as Linux controller
    participant Worker as worker::serve_with
    participant Policy as guest_computer::Policy
    participant Codex as Guest App Server
    API->>Worker: Authenticated miniclaw/computer/read
    Worker->>Policy: route
    Policy-->>API: Guest helper availability
    API->>Worker: thread/start or thread/resume, miniclawComputerUse
    Worker->>Policy: route
    alt Invalid marker or controller MCP override
        Policy-->>API: JSON-RPC error
    else Valid guest capability request
        Policy->>Policy: Remove marker, configure guest helper if enabled
        Policy-->>Worker: Forward frame
        Worker->>Codex: JSON-RPC request with guest-local paths
        Codex-->>API: Response through worker output
    end
```

The worker answers discovery locally. A missing helper rejects enable requests;
ordinary frames pass through unchanged. This diagram describes routing, not proof
of successful native screen capture or input.

## Source map

| Participant | Implementation |
| --- | --- |
| Svelte chat state | [chat.svelte.ts](../../web/src/chat.svelte.ts): `setComputer`, `applyComputer`, event subscription. |
| Svelte approval UI | [App.svelte](../../web/src/App.svelte) and [chat.svelte.ts](../../web/src/chat.svelte.ts): approval controls and `decide`. |
| API | [api.rs](../../server/src/api.rs): `set_computer`, `check_switch`, `resume_params`, `recover_tasks`, `decide`, `interrupt`. |
| Rust Chat cache | [chat.rs](../../server/src/chat.rs): `computer_changed`, `snapshot`, `emit`. |
| Recovery module | [recovery.rs](../../server/src/recovery.rs): `mark`, `clear`, `pending`. |
| Backend request listener | [chat.rs](../../server/src/chat.rs): `Backend::new`; form validation in [computer.rs](../../server/src/computer.rs): `Approval::new`. |
| RPC client | [rpc.rs](../../server/src/rpc.rs): `Client::spawn` reader and approval wait. |
| Guest routing | [guest_computer.rs](../../server/src/guest_computer.rs): `Policy::route`; [worker.rs](../../server/src/worker.rs): `serve_with`, `requests`, `send`. |
| Owned App Server | Launched by [runtime.rs](../../server/src/runtime.rs): `launch`. Its internal native-action execution is outside these diagrams. |
