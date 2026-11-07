# Network for shell commands sequence

```mermaid
sequenceDiagram
    participant Model
    participant Backend as scoped_tool_result
    participant Rules as permission store
    participant User
    participant Executor as executor (Mac or VM worker)
    participant Proxy as egress proxy
    participant Command as sandboxed command

    Model->>Backend: run_shell { text, cwd, hosts }
    Backend->>Rules: Feature "network"
    alt switch is off
        Backend-->>Model: not run, network is off
    end
    Backend->>Rules: Command, Host for each name
    alt something still needs an answer
        Backend->>User: popup: command, folder, hosts
        User-->>Backend: once, session, session whole domains, or reject
    end
    Backend->>Executor: operation and file grants
    Executor->>Proxy: start on a free loopback port
    Executor->>Command: run under Seatbelt, HTTPS_PROXY set
    Command->>Proxy: CONNECT host:443
    alt approved host with a public address
        Proxy-->>Command: tunnel
    else anything else
        Proxy-->>Command: 403, recorded
    end
    Executor-->>Backend: output and refused destinations
    Backend-->>Model: result
```

| Participant | Source |
| --- | --- |
| scoped_tool_result | [chat.rs](../../server/src/chat.rs) `scoped_tool_result`, `ScopedApproval` |
| permission store | [permissions.rs](../../server/src/permissions.rs) `Target`, `decide`, `dns_name`, `parent_domain` |
| executor | [executor.rs](../../server/src/executor.rs) `execute_host_resolving`, `command`; [seatbelt.rs](../../server/src/seatbelt.rs) `profile_with` |
| egress proxy | [egress.rs](../../server/src/egress.rs) `Proxy`, `serve`, `system_resolver`, `private` |
| popup and switch | [App.svelte](../../web/src/App.svelte), [PermissionSettings.svelte](../../web/src/PermissionSettings.svelte) |

Back to [Network for shell commands](README.md).
