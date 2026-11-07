# Skill import and chat binding

```mermaid
sequenceDiagram
    participant UI as Skills dialog
    participant API as api module
    participant Source as skills module
    participant Store as SQLite Store
    participant Codex as Owned App Server
    UI->>API: GET skills/sources after Import from Codex
    API->>Source: sources()
    Source-->>API: Folder IDs and names
    API-->>UI: Import candidates
    UI->>API: POST skills/import with selected source
    API->>API: Authorize account
    API->>Source: import(source)
    Source->>Source: Open anchored SKILL.md and read bounded text
    Source-->>API: Inactive definition
    API->>Store: save_skill under admission write lock
    Store-->>API: Saved copy
    API-->>UI: HTTP 201 with inactive skill
    UI->>API: Save activated skill and agent attachment
    API->>Store: Validate and save revisions
    UI->>API: Start chat with agentId
    API->>Store: prepare agent and active skills
    alt Missing or inactive skill
        Store-->>API: Error
        API-->>UI: No chat started
    else Eligible profile
        Store-->>API: Profile and skill revisions
        API->>Codex: thread/start with composed instructions
        Codex-->>API: Thread
        API->>Store: bind profile and skill snapshots
        Store-->>API: Commit
        API-->>UI: Chat summary
    end
```

The import copy is created before activation or attachment. These are separate user
actions, not automatic consequences of import. Codex thread creation and SQLite
binding are separate commits. See [agent recovery constraints](../agents/sequence.md).

## Source map

| Participant | Implementation |
| --- | --- |
| Skills dialog | [Skills.svelte](../../web/src/Skills.svelte): `discover`, `importSkill`, `save`; [Agents.svelte](../../web/src/Agents.svelte): attachment form. |
| API | [api.rs](../../server/src/api.rs): `skill_sources`, `import_skill`, `save_skill`, `save_agent`, `create`. |
| Source | [skills.rs](../../server/src/skills.rs): `sources`, `source_file`, `import`. |
| Store | [skills.rs](../../server/src/skills.rs): `save_skill`, `resolve`; [agents.rs](../../server/src/agents.rs): `prepare`, `configure`, `bind`, `binding`. |
| Owned App Server | [rpc.rs](../../server/src/rpc.rs): request transport. |

[Capability documentation](README.md)
