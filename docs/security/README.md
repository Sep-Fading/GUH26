# Local security boundary

MiniClaw is a single-user local application, not a remote service. Its checks limit
browser access and model tools; they do not sandbox every process on the device.
See the [request sequence](sequence.md).

## HTTP boundary

[main](../../server/src/main.rs) binds only IPv4 loopback.
[api::guard](../../server/src/api.rs) requires the exact `127.0.0.1:port` Host value,
rejects `Sec-Fetch-Site: cross-site`, and permits an Origin only when it equals the
application origin. Origin may be absent, including for local command-line clients.
Mutations also require `X-MiniClaw-Request: 1` and a Content-Type starting with
`application/json`. Browser code uses relative URLs; there is no CORS allowlist for
other origins or public raw-RPC endpoint.

The guard adds `no-store`, `nosniff`, `no-referrer`, and `X-Frame-Options: DENY`.
Its CSP restricts scripts and connections to self, permits self/data images and
inline styles, and forbids framing and base URLs. These checks also wrap static assets.
Typed JSON inputs are limited to 256 KiB; endpoint validation imposes narrower limits.

Malformed typed JSON returns `invalid_request`. `Error::into_response` exposes only
`code`, a fixed `message`, `retryable` (currently false), `requestId`, and
`outcomeUnknown`. Raw Codex errors and database details do not reach that payload.

These checks deter unrelated websites. They do not authenticate another local
process: a process able to call loopback can supply the expected headers. Do not
publish MiniClaw through a proxy or tunnel and assume these checks become user auth.

## Runtime policy

[runtime::launch](../../server/src/runtime.rs) applies
[chat-policy.json](../../server/chat-policy.json) only to its owned child. This
fragment removes API-key environment credentials from that process:

```rust
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY");
```

An active custom provider, `OPENAI_BASE_URL`, an explicit OpenAI base URL, and
non-default ChatGPT endpoints are rejected. Enabled inherited MCP entries trigger
a restricted relaunch; the reserved native-helper name cannot come from inherited
configuration. Shared credential and configuration files are not rewritten.

Codex is launched with web search disabled, and MiniClaw sets `web_search` to `live` for a
conversation unless its agent or project switched search off; the search runs at the provider
([capabilities](../capabilities/README.md)). Normal chat disables shell, file-view tools, skills, plugins, hooks,
native multi-agent features, and integrations. Task creation uses read-only sandbox,
`approvalPolicy: "never"`, and empty environments/capability roots. Resuming saved
tasks reapplies the restricted configuration. The real-runtime probes check the
executable tool inventory, rather than relying on prompt instructions.

[rpc::refusal](../../server/src/rpc.rs) declines command/file approvals, grants no
requested permissions, returns no user-input answers, and rejects dynamic tool calls.
Only the recognized [Computer Use confirmation path](../computer-use/README.md)
can reach the UI for approval.

## Data and authority

[Account checks](../account/README.md) require ChatGPT sign-in for protected routes.
Account changes invalidate cached state and pending work; persisted local history is
not a multi-user account-isolation mechanism. [History checks](../history/README.md)
validate the task UUID, exact canonical workspace, provider, and non-ephemeral status.

Messages render through Svelte text interpolation in
[App.svelte](../../web/src/App.svelte), not raw HTML or Markdown. Model-produced
markup is displayed as text; it does not execute scripts or load remote images.
Raw computer screenshots/tool results are omitted from browser activity summaries.
Prompts and approved-app screenshots still reach Codex for inference.

[Defined agents](../agents/README.md) cannot create or activate profiles through tools.
Unsupported capability/access policies cannot activate, and agent chats cannot enable
the general Computer Use toggle. The native helper has not established a strict
per-agent application sandbox; an app-targeting instruction is not enforcement.

## Asking less

A read-only command line from the fixed list in `harmless.rs`, or one whose programs the user
allowed from the popup (`Target::Program`, a lasting rule on the agent), runs without the approval popup;
the list is exact programs and read-only subcommands, and any shell syntax beyond `;`, `&&`,
`||` and `|` asks. The "Full access" conversation switch stops every popup but grants nothing:
what a rule blocks stays blocked, what no rule names is refused, and the Seatbelt profile is the
same. Both are the user's own switches, per conversation, and both are told to the model.

## MCP servers

MiniClaw is the MCP client, so a provider never holds a server's command, address or token
([MCP servers](../mcp-servers/README.md)). A local server runs under the same Seatbelt profile
as a shell command unless the user marks it unrestricted; an unrestricted server runs as the
user and can read a provider sign-in, which the form, the list and every approval say. Secrets
live in the macOS Keychain and no route returns them. A remote name is pinned to public
addresses and redirects are not followed. No tool adds or switches on a server; a model can
only propose one in text, which the user reviews in the server form and saves or not.

## Limits of protection

The single persistent listener belongs to MiniClaw; Codex manages its own OAuth flow.
The configuration does not make model inference offline. A same-user local process,
compromised runtime, or permitted native action is outside a browser-origin check.
Cancellation requests do not undo completed actions.

RPC queues, input sizes, history pages, and profile counts have bounds. Aggregate
cached transcript/replay memory does not have a complete byte budget. Agent metadata
and Codex thread creation are separate commits; losing the entire metadata database
also loses the bindings. These are not guarantees of general device isolation.

Coverage: [contracts](../../tests/contracts.test.mjs),
[startup](../../tests/startup.test.mjs), [agents](../../tests/agents.test.mjs),
[computer approvals](../../tests/computer.test.mjs), and
[browser text rendering](../../tests/browser/chat.spec.ts).
