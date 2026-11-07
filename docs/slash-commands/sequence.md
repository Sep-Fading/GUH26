# Slash commands: sequence

Back to [Slash commands and /mcp-create](README.md).

```mermaid
sequenceDiagram
    participant U as User
    participant W as SlashMenu.svelte
    participant A as api.rs send
    participant L as slash.rs
    participant M as Model
    participant K as MessageMarkdown
    participant F as McpServers.svelte

    U->>W: types "/"
    W->>A: GET /api/slash-commands (first use)
    A->>L: commands(skills)
    L-->>W: built-ins, then callable active skills
    U->>W: Enter on /mcp-create
    W-->>U: draft becomes "/mcp-create "
    U->>A: POST messages "/mcp-create a notes server"
    A->>L: section(text, skills)
    L-->>A: "## Skill called with /mcp-create ..."
    A->>M: turn/start, text unchanged, section in miniclaw_skills
    M-->>K: final answer with a miniclaw-mcp-server block
    K->>K: serverProposal checks the fields
    K-->>U: card "Add this server to MiniClaw"
    U->>K: click
    K->>F: window event, chat.mcpProposal
    F-->>U: new server form filled in, sandboxed
    U->>F: Save server
    F->>A: POST /api/mcp/servers
```

| Participant | Source |
|---|---|
| SlashMenu | [`SlashMenu.svelte`](../../web/src/SlashMenu.svelte) `key`, `choose` |
| send | [`api.rs`](../../server/src/api.rs) `send`, `slash_commands` |
| slash.rs | [`slash.rs`](../../server/src/slash.rs) `commands`, `section`, `named`, `slug` |
| MessageMarkdown | [`message-markdown.ts`](../../web/src/message-markdown.ts) `serverProposal`, `card`; [`MessageMarkdown.svelte`](../../web/src/MessageMarkdown.svelte) `act` |
| McpServers | [`McpServers.svelte`](../../web/src/McpServers.svelte), the `chat.mcpProposal` effect |

The diagram's syntax was not checked with a renderer.
