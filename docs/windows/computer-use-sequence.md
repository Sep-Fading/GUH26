# Windows Computer Use request sequence

```mermaid
sequenceDiagram
    participant Model as Codex tool call
    participant Bridge as windows_bridge::serve
    participant UI as MiniClaw approval UI
    participant Native as Installed native helper
    Model->>Bridge: Allowlisted method and arguments
    Bridge->>Bridge: Validate fields, size, app and launch inventory
    Bridge->>UI: Exact action approval
    alt Denied or expired
        Bridge-->>Model: Refused; no native action
    else Accepted
        opt Window action
            Bridge->>Native: Resolve HWND ownership
            Native-->>Bridge: Canonical app and HWND
            Bridge->>Bridge: Require exact identity match
        end
        Bridge->>Native: Native method without approval metadata
        opt Native app approval required
            Native-->>Bridge: Canonical app approval request
            Bridge->>UI: Fresh app approval
            UI-->>Bridge: Explicit accept
            Bridge->>Native: Same method with adapter-created approval metadata
        end
        Native-->>Bridge: Result or failure
        Bridge-->>Model: Filtered text and inline image data
    end
```

The bridge, validation, consent and result conversion are in
[windows_bridge.rs](../../server/src/computer/windows_bridge.rs). Helper process
containment is in [windows_process.rs](../../server/src/windows_process.rs).
[computer.rs](../../server/src/computer.rs) configures the MCP registration and exposes
MiniClaw's approval UI. Native GUI behavior remains unverified; see the
[capability and acceptance notes](computer-use.md).
