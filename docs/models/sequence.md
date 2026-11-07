# Model discovery and selection

[Model contracts](README.md)

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant API as api module
    participant Models as models module
    participant Codex as Owned App Server
    UI->>API: GET models
    API->>API: Authorize
    API->>Models: read
    loop Until nextCursor is absent, within limits
        Models->>Codex: model/list with includeHidden
        Codex-->>Models: Catalog page
        Models->>Models: Validate and deduplicate
    end
    Models-->>API: Combined catalog
    API-->>UI: models
    UI->>UI: Select model and supported effort
    UI->>API: POST messages with model and effort
    API->>Models: choose reads current catalog
    Models->>Codex: model/list pages
    Codex-->>Models: Current catalog
    Models-->>API: Catalog for validation
    alt Invalid choice
        API-->>UI: HTTP 400 invalid_model
    else Valid choice
        API->>Codex: turn/start with selected settings
        Codex-->>API: Turn acknowledgement
        API-->>UI: HTTP 202 receipt
    end
```

The Codex send path shown assumes a loaded idle chat and a new message ID. An identical
already-acknowledged receipt returns before catalog discovery.

| Participant | Source |
| --- | --- |
| Svelte Chat | [chat.svelte.ts](../../web/src/chat.svelte.ts): `refreshModels`, `setModel`, `setEffort`, `send`. |
| API | [api.rs](../../server/src/api.rs): `catalog`, `read_models`, `choose`, `send`. |
| Models | [models.rs](../../server/src/models.rs): `read`, `pages`. |
| Codex | Requests pass through [rpc.rs](../../server/src/rpc.rs): `Client::call`. |

## Provider refresh

```mermaid
sequenceDiagram
    participant UI as Providers panel / Chat
    participant API as api catalog
    participant Codex as Owned App Server
    participant Bridge as Claude bridge
    participant CLI as Claude Code CLI
    UI->>API: GET models with refresh=true
    API->>API: Authorize signed-in providers
    opt Codex signed in
        API->>Codex: model/list pages
        Codex-->>API: Current catalog
    end
    opt Claude signed in
        API->>Bridge: model/list with refresh=true
        Bridge->>CLI: initialize control request, no prompt
        CLI-->>Bridge: Model picker metadata
        Bridge->>Bridge: Normalize and replace successful cache
        Bridge-->>API: Shared catalog shape
    end
    API-->>UI: Combined models or discovery error
    UI->>UI: Update counts and choices, preserve draft and selection
```

`Providers.svelte` calls `Chat.refresh(true)` through its existing header control.
[api.rs](../../server/src/api.rs) owns `CatalogQuery` and `catalog`.
[models.rs](../../server/src/models.rs) owns `refresh_claude` and pagination.
[claude.rs](../../server/src/claude.rs) owns `Bridge::models`, `control`, and normalization
in `catalog`. A failed discovery leaves the previous cache intact and returns an error;
the page retains its prior list and reports the failure.
