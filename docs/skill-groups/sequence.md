# Group changes and the next message

```mermaid
sequenceDiagram
    participant UI as GroupEditor / ConversationSkills
    participant API as api module
    participant Store as SQLite Store
    participant Chat as Conversation
    participant Codex as Owned App Server
    UI->>API: Save group or conversation selection with revision
    API->>API: Authorize and acquire admission write lock
    API->>Store: save_group / save_selection
    alt Revision conflict or invalid settings
        Store-->>API: Error
        API-->>UI: Error, retain draft
    else Valid settings
        Store-->>API: Commit revision
        API-->>UI: Saved settings
    end
    Note over Chat,Codex: Existing response<br/>continues unchanged
    Chat->>API: Send next message
    API->>API: Authorize, acquire admission read lock, check receipt and idle state
    API->>Store: admit and turn_skills
    Store->>Store: Read pinned agent group IDs, pinned skills and live group contributions
    Store->>Store: Deduplicate IDs and enforce bounds
    Store-->>API: Current selection context or error
    alt Valid context
        API->>API: Validate model and reserve message
        API->>Codex: turn/start with additionalContext.miniclaw_skills
        Codex-->>API: Turn acknowledgement
        API-->>Chat: Accepted message
        Codex-->>API: Streaming notifications
        API-->>Chat: SSE updates
    else Resolution failed
        API-->>Chat: Error, no message admitted
    end
```

| Participant | Source |
| --- | --- |
| GroupEditor / ConversationSkills | [GroupEditor.svelte](../../web/src/GroupEditor.svelte): `save`; [ConversationSkills.svelte](../../web/src/ConversationSkills.svelte): `save`; [skill-groups.svelte.ts](../../web/src/skill-groups.svelte.ts): API state |
| API | [api.rs](../../server/src/api.rs): `save_group`, `save_chat_skills`, `send` |
| Store | [skill_groups.rs](../../server/src/skill_groups.rs): `save_group`, `save_selection`, `turn_skills`; [projects.rs](../../server/src/projects.rs): `admit` |
| Conversation | [chat.svelte.ts](../../web/src/chat.svelte.ts): `send`, `select` |
| Owned App Server | [rpc.rs](../../server/src/rpc.rs): request transport; [chat.rs](../../server/src/chat.rs): notifications |

Notifications may precede the turn acknowledgement. Existing receipt and SSE
handling retain that ordering independently of group resolution.

[Capability documentation](README.md)
