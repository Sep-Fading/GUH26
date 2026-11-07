# Project chat

```mermaid
sequenceDiagram
    participant UI as TaskNav / Chat
    participant API as api::create / send
    participant Store as agents::Store
    participant Codex as Codex App Server
    UI->>UI: newChat(projectId), choose agent
    UI->>API: First Send creates chat with projectId
    API->>Store: prepare_project(projectId, agentId)
    alt Default MiniClaw
        Store->>Store: Resolve active project skills
    else Named agent
        Store->>Store: Check project agent membership
        API->>Store: prepare(agentId), resolve own skills
    end
    Store-->>API: Instruction context
    API->>Codex: thread/start with bounded policy and instructions
    Codex-->>API: Thread identity
    API->>Store: bind(thread, agent, project) transaction
    API-->>UI: Thread summary with projectId
    UI->>API: Send message to thread
    API->>Store: admit(thread): current activation and membership
    alt Authorized
        API->>Codex: turn/start
        Codex-->>UI: Notifications through backend SSE
    else Revoked
        API-->>UI: Error, history remains readable
    end
```

- [TaskNav](../../web/src/TaskNav.svelte) and [Chat](../../web/src/chat.svelte.ts): selection, creation, send, SSE.
- [api.rs](../../server/src/api.rs): `create`, `send`, `configure_context`, `tag_context`.
- [projects.rs](../../server/src/projects.rs): `prepare_project`, `project_context`, `admit`.
- [agents.rs](../../server/src/agents.rs): `prepare`, `bind`.

[Project contracts](README.md)
