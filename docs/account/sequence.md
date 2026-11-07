# Codex-managed sign-in

[Account and usage](README.md)

```mermaid
sequenceDiagram
    participant UI as Svelte Chat
    participant API as api module
    participant Codex as Owned App Server
    participant Auth as Codex-managed authorization
    participant Cache as Backend Data
    UI->>API: POST auth/start with method
    API->>Codex: account/read
    Codex-->>API: Signed-out account
    API->>Codex: account/login/start
    Codex-->>API: loginId and auth URL or device fields
    API->>API: Validate URL host and scheme
    API->>Cache: Store login attempt
    API-->>UI: Public attempt status and auth details
    UI->>Auth: Open returned URL for user sign-in
    Auth-->>Codex: Authorization result
    Codex--)Cache: account/login/completed through listener
    UI->>API: GET account and auth/status during refresh
    API->>Codex: account/read
    Codex-->>API: Current account
    API-->>UI: Normalized account and cached attempt status
```

OAuth runs inside Codex; this diagram does not specify its token exchange. Login
completion may precede the start reply. `login_results` reconciles that ordering.
The account read, not a successful popup navigation, authorizes later chat requests.

| Participant | Source |
| --- | --- |
| Svelte Chat | [chat.svelte.ts](../../web/src/chat.svelte.ts): `signIn`, `refresh`, `cancelLogin`. |
| API | [api.rs](../../server/src/api.rs): `login`, `read_account`, `auth_url`, `login_status`. |
| Codex transport | [rpc.rs](../../server/src/rpc.rs): `Client::call` and notification routing. |
| Backend Data | [chat.rs](../../server/src/chat.rs): login notification handling in `Backend::new`. |

## Provider limits

```mermaid
sequenceDiagram
    participant UI as Chat and UsageLimits
    participant API as usage::read
    participant Cache as Backend Data
    participant Provider as Provider client
    participant CLI as Claude CLI
    UI->>API: GET /api/usage with provider and optional refresh
    API->>Provider: Read provider account
    Provider-->>API: Current identity
    API->>Cache: Check provider reading and identity
    alt Fresh cached Claude reading and no forced refresh
        Cache-->>API: Cached reading
    else Read current limits
        API->>Provider: account/rateLimits/read
        alt Claude
            Provider->>CLI: initialize, then get_usage with skip_behaviors
            CLI-->>Provider: Structured limits or failure
        end
        Provider-->>API: Provider limits or failure
        API->>Cache: Save normalized reading or mark previous reading stale
    end
    API-->>UI: Limits, freshness and fetch time
```

| Participant | Source |
| --- | --- |
| Chat and UsageLimits | [chat.svelte.ts](../../web/src/chat.svelte.ts): `refreshUsage`, `usage`; [UsageLimits.svelte](../../web/src/UsageLimits.svelte). |
| usage::read | [api/usage.rs](../../server/src/api/usage.rs): authorization, cache selection, normalization. |
| Provider client | [providers.rs](../../server/src/providers.rs): `provider_client`; [claude.rs](../../server/src/claude.rs): `serve`, `Bridge::usage`. |
| Backend Data | [chat.rs](../../server/src/chat.rs): per-provider `usage` map. |

The Codex client answers through the owned App Server. The Claude client delegates the
read to its CLI. Both provider refreshes can run concurrently; neither starts a chat turn.
