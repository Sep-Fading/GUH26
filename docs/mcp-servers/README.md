# MCP servers

MiniClaw is the MCP client. It starts local servers and calls remote ones itself, and offers
both providers one tool that relays to them. Codex and Claude are never given a server's
command, address or token. Codex's own `mcp_servers` stay disabled and Claude still runs with
`--strict-mcp-config`; their config files are read for import only.

Sequence: [sequence.md](sequence.md).

## The list

Settings, MCP servers ([`McpServers.svelte`](../../web/src/McpServers.svelte)). One list serves
both providers. Rows live in the `mcp_servers` table of the metadata database
([`mcp_servers.rs`](../../server/src/mcp_servers.rs)); at most 32 exist.

| Field | Local command (`stdio`) | Remote address (`http`) |
|---|---|---|
| `name` | lowercase letters, digits, `-`, `_`; 40 at most; not starting with `miniclaw`; unique | same |
| `command`, `args` | absolute path; at most 64 arguments | refused |
| `folder` | optional. The one folder a sandboxed server reads and writes, and its working folder. A folder MiniClaw protects, or one that contains such a folder, is refused, judged as written and as it resolves | refused |
| `hosts` | optional, 16 at most. Plain DNS names the server may reach over HTTPS | refused |
| `env` | names and values; the values are secrets | refused |
| `ports` | optional, 8 at most, 1024 and above. Ports on this Mac the server may connect to, for a server that drives an app through the app's own socket (Blender's add-on listens on 9876). MiniClaw's own port is refused | refused |
| `listen` | optional, 8 at most, 1024 and above, none shared with `ports`. Ports the server itself listens on, loopback only, for a server that an app on this Mac connects to (Roblox Studio's plugin dials 13469). MiniClaw's own port is refused | refused |
| `unrestricted` | runs outside the sandbox. Off by default | refused |
| `headers` | refused | optional, 16 at most: extra HTTP headers whose values are secrets. `Host`, `Accept`, `Content-Type`, `Content-Length`, `Mcp-Session-Id`, `MCP-Protocol-Version` and the like are MiniClaw's to set |
| `url` | refused | `https://` anywhere; `http://` only for `127.0.0.1`, `localhost`, `[::1]`; no sign-in in the address |
| `token` | refused | optional bearer token; a secret |

Both kinds also take:

| Field | Meaning |
|---|---|
| `provider` | `both` (default), `codex` or `claude`: whose conversations may use the server |
| `startupSeconds` | 1 to 120, default 15: how long the server may take to start and answer `initialize` and a listing |
| `toolSeconds` | 1 to 3600, default 300: how long one tool call may take |
| `enabledTools`, `disabledTools` | when `enabledTools` is not empty only those are offered; a disabled tool is never offered |

An edit carries the `revision` the page saw; a stale one answers 409. A server never changes
its kind. No OAuth flow exists yet.

### The provider choice

MiniClaw is the client, so one server works for both providers and neither needs an entry of
its own. The Provider dropdown in the form does two things. It limits who may use the server:
the gateway, the per-message note and the Tools dropdown leave out a server kept to the other
provider, and a call to it is refused with that reason. And it shows the fields that
provider's own configuration has, with its names for them: Codex has `startup_timeout_sec`,
`tool_timeout_sec`, `enabled_tools` and `disabled_tools`; Claude Code has a timeout and
`headers`. The fields are not labels: MiniClaw honours each one itself
([`mcp_client.rs`](../../server/src/mcp_client.rs), [`mcp_gateway.rs`](../../server/src/mcp_gateway.rs)).
Check connection lists every tool the server has, each marked with whether the model is offered it.

### Secrets

Environment values and the token go to the macOS Keychain under the service
`MiniClaw MCP server` ([`secrets.rs`](../../server/src/secrets.rs)). The database row holds the
names and `hasToken` only, and no route returns a value. They are read in two places: when a
local server's process is started, and when a remote request is built.

```rust
/// The Keychain account name of a server's token.
pub fn token_key(server: &str) -> String { format!("{server}:token") }

/// The Keychain account name of one environment value of a server.
pub fn env_key(server: &str, name: &str) -> String { format!("{server}:env:{name}") }
```
[`secrets.rs`](../../server/src/secrets.rs)

In an edit, `env` with a value sets that name, `null` keeps its saved value, a name left out is
removed, and leaving `env` out keeps everything. `token` left out keeps it, `null` removes it.
Tests set `MINICLAW_TEST_SECRETS` to a private file so they never touch the real Keychain.

### Import

`GET /api/mcp/sources?provider=codex|claude` and `POST /api/mcp/import`
([`mcp_import.rs`](../../server/src/mcp_import.rs)) read Codex's `config.toml` (`mcp_servers`)
and Claude Code's `.claude.json` (`mcpServers`, top level and per project). A project's
`.mcp.json` is not read. Only the definition is copied. Values of `env`, `headers`,
`http_headers` and the rest are dropped once the file is parsed; their names are saved in
`needs`, and the row says "Enter its token" or "Enter NAME" until the user does. A bare
command such as `npx` is resolved on the PATH, since a saved server names an absolute path; one
that cannot be found is refused. An imported server is switched on nowhere, and the provider's
file is never changed.

## The client

[`mcp_client.rs`](../../server/src/mcp_client.rs) follows the MCP specification of 2025-06-18
and also speaks `2025-03-26` and `2024-11-05`.

A local server is a child process with newline-delimited JSON-RPC on its pipes. Its
environment is cleared and rebuilt: a fixed `PATH`, the saved values, and for a sandboxed
server `HOME` and `TMPDIR` pointing at its folder. Unless `unrestricted` is set it runs under
the same Seatbelt profile as a shell command, with the same closed paths
([security](../security/README.md)):

```rust
let mut grants = Vec::new();
if let Some(folder) = &folder { grants.push(Grant { path: folder.clone(), write: true, folder: true, blocked: false }); }
if let Some(prefix) = install_prefix(program) { grants.push(Grant { path: prefix, write: false, folder: true, blocked: false }); }
```
[`mcp_client.rs`](../../server/src/mcp_client.rs) `spawn`

With `ports` set, the profile allows connections to exactly those loopback ports
([`seatbelt.rs`](../../server/src/seatbelt.rs) `profile_net`); every other port on this Mac stays
refused. With `listen` set it also allows `network-bind` and `network-inbound` on exactly those
ports, on the loopback addresses only, so the server can accept an app's connection and nothing
off this Mac can reach it, and `network-outbound` to the same port, because Roblox's proxy is a
host and a client in one process whose client half dials the host (without that rule it logs
"WS host connection error: Operation not permitted" and Studio never gets through). Any process on this Mac can, which the field's help says. With `hosts` set, the per-command proxy of [command network](../command-network/README.md) is
the server's only way out, passed as `HTTPS_PROXY` with `NODE_USE_ENV_PROXY=1`. The process
runs in its own group, and dropping the connection kills the group, because a launcher such as
`npx` starts the real server as its child. A line that is not JSON is skipped; `ping` is
answered; any other request from the server gets "Method not found".

A remote server is called over Streamable HTTP: one POST per message, `Accept` of both JSON and
an event stream, then `Mcp-Session-Id` and `MCP-Protocol-Version` on every later request. A
public name is resolved once and pinned to public addresses, so it cannot lead to this Mac or
the local network. Redirects are not followed, so the token reaches the saved address only.
HTTP 401 or 403 is reported as a refused token.

Limits: 8 MiB per message or answer, 128 tools per server, and the server's own start and
tool timeouts (15 s and 300 s unless it sets others).

`POST /api/mcp/servers/{id}/check` connects, lists the tools and disconnects. A failure
answers 502 `mcp_unreachable` with the reason in words. It switches nothing on.

## Servers that listen: Roblox Studio

Blender's server connects to Blender's add-on, which listens on 9876; that is `ports`. Roblox's
bundled server is the other way round: `/Applications/RobloxStudio.app/Contents/MacOS/StudioMCP`
speaks MCP on stdio and binds a WebSocket host on `127.0.0.1:13469` that Studio's plugin dials
every few seconds (read from the binary and from `~/Library/Logs/Roblox/*Studio*last.log`, where
Studio logs `ConnectFail` while nothing listens). Its tool list arrives only once a Studio has
connected, so before the port was allowed the check answered success with no tools; the check
now says so ("The server offers no tools yet…") whenever a server lists none.

To use it: save the server with folder `/Applications/RobloxStudio.app/Contents/MacOS` and
`listen: [13469]`; in Studio open a place and enable the MCP server under Assistant settings;
Check connection then lists the tools. The proxy keeps a tools cache at
`~/Library/Application Support/StudioMCP/`, outside its folder, so it logs "starting with empty
cache" and works without it.

On Windows the bundle registers `roblox_studio` as `cmd.exe /d /c %LOCALAPPDATA%\Roblox\mcp.bat`
(the path written out) with no folder, so Studio's own folder (sign-in profile, `mcp.bat`, per-user Studio
builds) is never a sandbox grant. It cannot run sandboxed there: a sandboxed server with listening ports
is refused before anything is prepared, and on ARM64 Studio's programs are x64. The refusal names the
way to run it, "Run outside the sandbox", which only the user turns on. See
[sandbox review](../windows/sandbox-review.md#roblox-studio-on-windows).

## Switching a server on

A saved server is off everywhere. Two permission targets exist
([`permissions.rs`](../../server/src/permissions.rs)):

- `mcp_server { server }` is a bounded switch ([capabilities](../capabilities/README.md)): the
  agent (or the main agent, or the project) allows the server, which also switches it on in
  its conversations, and a conversation switches it off and back on from the Tools dropdown.
  A conversation cannot switch on a server its agent does not allow. It is never asked about.
- `mcp_tool { server, tool }` asks, like a command.

Both name the server by its id. Deleting a server removes its rules and session grants
(`forget_mcp_server`), so a later server of the same name starts switched on nowhere.

## The gateway tool

Codex fixes a conversation's tools at `thread/start` and `thread/resume`, while a switch can
change at any moment. So both providers get one stable tool, `miniclaw_mcp`
([`mcp_gateway.rs`](../../server/src/mcp_gateway.rs)), and what it reaches is decided at each
call:

| Action | Result |
|---|---|
| `list_tools`, with optional `server` and `query` | the servers that are on, each tool as a name and the first sentence of its description, or the reason a server could not be reached. `query` keeps the tools whose name or description carries all its words |
| `describe_tool` with `server`, `tool` | one tool's full description and input schema |
| `call_tool` with `server`, `tool`, `arguments` | the tool's text, at most 256 KiB, and its pictures. Audio and other content is named, not passed on |

The list carries no schemas on purpose. Roblox Studio's server lists about 70 KB of tools; a
result that size is one Claude Code files on disk, at a path no grant reaches, so the model saw
nothing and guessed tool names. The list stays under 48 KiB (past that, names alone; past that
still, a count per server and a note to narrow with `query`), and a tool's schema comes one at a
time through `describe_tool`. A tool name nobody has is refused with the close names (sharing a
piece of three letters or more, eight at most) before anyone is asked, so a guess costs no
approval.

Pictures (a viewport screenshot, for example) reach the model as pictures: PNG, JPEG, WebP or
GIF, 4 per result at most, about 4.5 MB each. Codex receives an `inputImage` content item with a
data URL, which the installed Codex's generated schema allows for a dynamic tool's result;
Claude receives MCP image content through the shim
([`tool_bridge.rs`](../../server/src/tool_bridge.rs)). The text says how many follow and how
many could not be passed on.

MiniClaw keeps each picture with the conversation (requirement 2026-09-22, for Roblox Studio
screenshots): `mcp_gateway.rs` `keep` decodes it into the conversation's attachment store
([`attachments.rs`](../../server/src/attachments.rs) `keep`), named `<server>-<tool>-<n>.png`.
Its record carries the message id `attachments::TOOL` (nil): it has no turn, so it is never shown
under a user message, and it is never discarded as an unsent upload. The
[Activity](../activity/README.md) entry gains `pictures: [{id, name, url}]`, a link to the
existing attachment route and never the bytes; the page shows thumbnails under the entry that
open full size. The model's text names each kept picture as `attachment <id> (<name>)`, which
is how it hands one to another agent ([delegation](../delegation/README.md)). A picture that
cannot be kept still reaches the model and is only missing from Activity. Kept pictures go to
the Trash with the conversation, like uploads.

No action adds, changes or switches on a server. The tool is offered to every conversation on
this Mac, Computer Use conversations included: Computer Use excludes MiniClaw's file and command
tool, and the gateway asks about every tool by itself. A VM conversation gets it when its
worker carries it. The servers still run on the Mac in their own sandbox, and the VM conversation's
briefing says so: their tools act on the Mac and cannot see the VM's files. See [worker transport](../worker-transport/README.md#tools-the-mac-answers).

A Claude turn gets the tool only while at least one server is on for its conversation
(`miniclawMcp` on `turn/start`). With none on, the turn has no shim and keeps `--safe-mode`.

The order of a call: the conversation and its running turn are checked, the server must be on,
the tool must exist, the user approves, the switch is checked again, the server is called. The
popup shows the server's name, the tool, the exact arguments, and a warning for an unrestricted
or a remote server. The choices are once, for this session and reject; the session grant covers
the tool with any input. A [coding session](../coding-session/README.md) asks every time. An
unattended [scheduled run](../schedules/README.md) waits as long as its schedule says.

One connection per server is kept in `Backend::mcp` and reused; calls to one server wait for
each other. Editing or deleting a server drops its connection, and a call that fails drops it
too.

Every call is an [activity](../activity/README.md) entry of kind `mcp`, with what was sent and
what came back.

## Tests

`npm run test:mcp` (28): the provider choice, tool filters, timeouts, headers and pictures with
both fake providers ([`mcp-provider.test.mjs`](../../tests/mcp-provider.test.mjs)); the list and import ([`mcp-servers.test.mjs`](../../tests/mcp-servers.test.mjs)),
the client against a fake stdio server and a fake Streamable HTTP server
([`mcp-client.test.mjs`](../../tests/mcp-client.test.mjs)), the gateway with both providers
([`mcp-tools.test.mjs`](../../tests/mcp-tools.test.mjs)). The page:
[`mcp-servers.spec.ts`](../../tests/browser/mcp-servers.spec.ts).

Observed by the user on 2026-09-21: a real published server (`blender-mcp` 1.30.0, a local
command, sandboxed) connected through Check connection and listed its 26 tools.

Observed with a real Codex model on 2026-09-21
([`real-mcp.test.mjs`](../../tests/experiments/real-mcp.test.mjs), opt-in, `gpt-5.6-luna` at low
effort): asked to "use the Parrot MCP", it listed the tools through `miniclaw_mcp`, called the
tool after the user's approval, and reported the result. Before the
[briefing](../briefing/README.md) existed the same kind of request failed for the user: a plain
conversation was told "No tools are available", so the model said it had no access to the
Blender MCP without ever calling the gateway, although the server was saved and allowed. The
tool's description and the per-message note now say that the gateway is the only way to any MCP
server.

Not observed: a real remote server and the real Keychain prompts.
