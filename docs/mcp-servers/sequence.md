# MCP servers: sequence

Back to [MCP servers](README.md).

One `call_tool` from a model to a sandboxed local server.

```mermaid
sequenceDiagram
    participant M as Model (Codex or Claude)
    participant R as rpc.rs / claude.rs
    participant G as mcp_gateway::handle
    participant P as permissions
    participant U as User (approval popup)
    participant H as Hub (Backend::mcp)
    participant C as mcp_client::Link
    participant S as MCP server (Seatbelt)

    M->>R: item/tool/call miniclaw_mcp {call_tool, server, tool, arguments}
    R->>G: ServerRequest
    G->>G: publish_activity (running)
    G->>P: permission(mcp_server) for every saved server
    P-->>G: the servers that are on
    alt server is not on
        G-->>M: refused, in words
    end
    G->>H: slot(server id)
    H->>C: connect, initialize, tools/list (first use or new revision)
    C->>S: spawn under the profile, secrets from the Keychain
    S-->>C: tools
    G->>P: permission(mcp_tool)
    alt no grant, or a coding session
        G->>U: scopedApproval with serverName, arguments, unrestricted
        U-->>G: once, session or reject
    end
    G->>P: permission(mcp_server) again
    G->>C: tools/call
    C->>S: request line on stdin
    S-->>C: result line on stdout
    C-->>G: content, isError
    G->>G: publish_activity (ok, failed or refused, with output)
    G-->>M: success and text
```

| Participant | Source |
|---|---|
| rpc.rs / claude.rs | [`rpc.rs`](../../server/src/rpc.rs) tool call filter; [`claude.rs`](../../server/src/claude.rs) `tool_call`; [`tool_bridge.rs`](../../server/src/tool_bridge.rs) `tools` |
| mcp_gateway | [`mcp_gateway.rs`](../../server/src/mcp_gateway.rs) `handle`, `run`, `list`, `call`, `ask`, `switched_on` |
| permissions | [`permissions.rs`](../../server/src/permissions.rs) `Target::McpServer`, `Target::McpTool`, `forget_mcp_server` |
| Hub | [`mcp_gateway.rs`](../../server/src/mcp_gateway.rs) `Hub`, `ensure` |
| Link | [`mcp_client.rs`](../../server/src/mcp_client.rs) `Link::connect`, `tools`, `call`, `spawn`, `http` |
| Approval popup | [`App.svelte`](../../web/src/App.svelte), the `mcp_tool` branch of the scoped approval dialog |

The diagram's syntax was not checked with a renderer.
