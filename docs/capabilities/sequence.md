# Capabilities: sequence

Back to [Capabilities](README.md).

```mermaid
sequenceDiagram
    participant U as User
    participant T as Tools dropdown
    participant D as Chat draft
    participant C as api/capabilities.rs
    participant A as api.rs
    participant S as Agent store
    participant P as Provider

    U->>T: Open Tools before the first message
    T->>C: GET /api/capabilities with draft context
    C->>S: configured_switch(agent, project, target)
    C->>S: Read agent activation defaults
    C-->>T: Allowed tools and defaults
    U->>T: Choose switches
    T->>D: toggleCapability(key, on), kept locally
    U->>D: Send first prompt
    D->>A: POST /api/threads with capabilities
    A->>C: prepare, validate choices
    C->>S: Merge agent defaults, preserving explicit choices
    alt Invalid or no longer allowed
        C-->>A: Validation error
        A-->>D: Error, prompt and choices retained
    else Allowed
        C-->>A: Initial permission rules
        A->>S: reserve bindings and rules in one transaction
        A->>P: thread/start with selected web search
        P-->>A: Conversation ID
        A->>S: created, check_creation move bindings and rules
        A-->>D: Created conversation
        D->>A: Load snapshot, then POST first message
        A->>S: Read saved tool choices
        A->>P: Start first turn with configured tools
    end
```

| Participant | Source |
|---|---|
| Tools dropdown | [`Capabilities.svelte`](../../web/src/Capabilities.svelte) |
| Chat draft | [`chat.svelte.ts`](../../web/src/chat.svelte.ts) `toggleCapability`, `create`, `send` |
| api/capabilities.rs | [`api/capabilities.rs`](../../server/src/api/capabilities.rs) `list`, `prepare`, `initial_defaults` |
| api.rs | [`api.rs`](../../server/src/api.rs) `create_chat`, `send` |
| Agent store | [`capability_defaults.rs`](../../server/src/capability_defaults.rs) `capability_defaults`, [`agents.rs`](../../server/src/agents.rs) `reserve`, [`agents/creation.rs`](../../server/src/agents/creation.rs) `created`, `check_creation`, [`permissions.rs`](../../server/src/permissions.rs) `configured_switch`, `permission` |
| Provider | [`rpc.rs`](../../server/src/rpc.rs) `Client::call`, [`claude.rs`](../../server/src/claude.rs) |

The original diagrams were rendered on 2026-09-22; the activation-default update has not been rendered.


## Claude tool selection

```mermaid
sequenceDiagram
    participant U as User
    participant T as Tools dropdown
    participant A as api.rs
    participant S as Permission store
    participant B as Claude bridge
    participant C as Claude CLI

    U->>T: Switch one capability
    T->>A: POST conversation capabilities
    A->>S: Save its own permission rule
    S-->>A: Saved
    A-->>T: Updated
    U->>A: Send the next message
    A->>S: Read effective tool choices and host rules
    S-->>A: Independent choices
    A->>B: turn/start with miniclawNativeTools
    B->>B: enabled_tools builds the selected list
    B->>C: Start with tools, fetch rules and plan mode
```

| Participant | Source |
|---|---|
| Tools dropdown | [`Capabilities.svelte`](../../web/src/Capabilities.svelte) |
| api.rs | [`api/capabilities.rs`](../../server/src/api/capabilities.rs) `toggle`, [`api.rs`](../../server/src/api.rs) `claude_tools`, `send` |
| Permission store | [`permissions.rs`](../../server/src/permissions.rs) `permission`, `set_permission` |
| Claude bridge and CLI | [`claude.rs`](../../server/src/claude.rs) `NativeTools::read`, `enabled_tools`, `Bridge::command` |

## Image generation

```mermaid
sequenceDiagram
    participant U as User
    participant A as MiniClaw API
    participant C as Codex App Server
    participant N as Notification handler
    participant S as Attachment store
    participant P as Chat page
    U->>A: Start chat with permitted image switch
    A->>C: thread/start with features.image_generation
    U->>A: Send image request
    A->>C: turn/start
    C-->>N: item/started imageGeneration
    N-->>P: Generating status
    C-->>N: item/completed with PNG base64
    N->>S: generated(thread, item, result)
    S-->>N: Private attachment reference
    N-->>P: Snapshot without raw base64
    P->>A: GET attachment
    A-->>P: PNG bytes
```

API: [`api.rs`](../../server/src/api.rs) `create_chat`, `send`; notifications:
[`chat.rs`](../../server/src/chat.rs) `clean_image_notification`, `Chat::notify`; storage:
[`attachments.rs`](../../server/src/attachments.rs) `generated`; page:
[`App.svelte`](../../web/src/App.svelte), `MessageFiles`. This added diagram was checked
against source but has not been rendered.

## Timed shell access

```mermaid
sequenceDiagram
    participant U as User
    participant T as Tools dropdown
    participant A as api/capabilities.rs
    participant G as Backend grant
    participant S as shell_access.rs
    participant E as Executor process group
    U->>T: Choose duration and enable
    T->>A: Toggle with durationSeconds
    A->>A: Check host and agent/project allowance
    A->>G: Save ephemeral context and deadline
    S->>G: Check original grant on run_shell
    S->>E: Run fresh unrestricted shell
    alt Deadline or revocation
        S->>E: Cancel and kill process group
        T->>A: Read capabilities
        A-->>T: Off and expired
        T-->>U: Review and renew access
    else Command finishes
        E-->>S: Bounded output
    end
```

Source: `toggle_shell` and `list` in api/capabilities.rs, `run_unrestricted` in shell_access.rs,
`execute_unrestricted` and `ProcessGroup::drop` in executor.rs. Diagram not visually rendered.
