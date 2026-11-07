# Briefing: sequence

Back to [What a model is told about where it is](README.md).

```mermaid
sequenceDiagram
    participant S as main.rs
    participant D as briefing::detect
    participant A as api.rs create_chat / configure_context
    participant V as api.rs survey
    participant E as executor (Seatbelt)
    participant B as briefing.rs
    participant P as Provider

    S->>D: once per start: sw_vers, uname, tool versions
    A->>A: offer tools (file tool, MCP gateway, questions, delegation)
    A->>D: wait until detection is done
    A->>V: a Mac conversation with a workspace and the file tool
    V->>E: list_folder and git status, read grant for that folder only
    E-->>V: names, branch, uncommitted count
    A->>B: standing(tools really given, machine, folder, web search)
    B-->>A: base instructions
    A->>P: thread/start or thread/resume
    Note over A,P: with every message
    A->>A: environment_note: date, agent, project, capabilities, servers, delegates
    A->>P: turn/start with additionalContext.miniclaw_environment
```

| Participant | Source |
|---|---|
| detect | [`briefing.rs`](../../server/src/briefing.rs) `detect`, `first_line` |
| create_chat / configure_context | [`api.rs`](../../server/src/api.rs) `brief`, `environment_note`, `send` |
| survey | [`api.rs`](../../server/src/api.rs) `survey` |
| briefing.rs | [`briefing.rs`](../../server/src/briefing.rs) `standing`, `state` |
| Claude | [`claude.rs`](../../server/src/claude.rs) `instructions`, `begin` (the note joins the system prompt) |

The diagram's syntax was not checked with a renderer.
