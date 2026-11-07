# MiniClaw

Local Svelte + TypeScript interface with a Rust Axum backend.

Create and resume chats, receive streamed replies, and stop a response. Projects
group chats and select available agents and skills. Named agents pin their
instructions to direct chats. The backend runs on the Mac and starts its own
providers: a background Codex App Server and a Claude Code bridge. Either one alone
is enough, and normal chat works with the desktop apps closed. Agents get one tool
whose shell commands run in a Seatbelt sandbox. A macOS VM for untrusted projects is
optional. Each provider reuses its existing sign-in; there is no separate MiniClaw
account or logout.

The composer has model and reasoning-effort selectors populated from Codex's full
catalog, including hidden entries. Changing models selects that model's default
effort. Choices apply to the next response; recorded settings return when a saved
task is reopened after restart. They do not change shared Codex settings. Catalog entries
can still be rejected by the service when a response is requested.

## Develop on the host

Use Node 24 (`.nvmrc`) and Rust stable. Install JavaScript dependencies:

```sh
npm ci
```

Build and start the app:

```sh
npm start
```

Open [MiniClaw](http://127.0.0.1:8787/). Rust serves the UI, API, and event streams
on this one address. The launcher builds both frontend and backend; Cargo must
be on PATH (`source "$HOME/.cargo/env"` after a fresh Rust installation).

For frontend development, use this instead:

```sh
npm run dev
```

It uses the same address. Vite watches and rebuilds files without opening a server;
refresh the browser after a frontend change. Restart after Rust or config changes.
There is no separate Vite URL or hot-reload WebSocket.

Ctrl-C stops the launcher and its owned backend/runtime. An occupied port is an
error, not a reason to open the next port. A file lock allows one backend per
`MINICLAW_DATA_DIR`, even if a second command specifies another port. The OS releases
the lock on exit; leave `instance.lock` in place. An explicit `MINICLAW_PORT` override
changes the single address without needing a frontend config edit.

`MINICLAW_TRASH_DIR` replaces the folder that deleted files, uploads and conversation folders
are moved to (the Trash on the Mac, `Recovered files` in the data folder on Windows). Every
build honours it; tests set it so they never fill the user's Trash. `MINICLAW_DOCUMENTS_DIR`
replaces the Documents folder that holds conversation folders (`Documents/MiniClaw`), but only
in test and debug builds: those folders become sandbox grants, so a release build always uses
the user's own Documents folder.

## Installation wizard

On Apple silicon Macs, after `npm ci`, register the checkout command once with
`npm link --offline --ignore-scripts`, then run `miniclaw setup`.

The wizard checks the Mac first: hardware, Rust, the Seatbelt sandbox, and whether
Codex and Claude Code are installed and signed in. The default path then asks three
things: which providers to enable, the port, and whether to add a VM. It shows a
review and installs only after approval. Progress runs as numbered steps, with a bar
for each download and a build log that clears on success and stays on failure. It
ends with the start command and any provider you still need to sign in to.

Setup needs Node 24, Cargo, C build tools and at least one provider. It installs a
separate runtime and leaves the checkout unchanged. Setup remembers where it installed,
so the everyday commands need no path:

```text
miniclaw                  start MiniClaw
miniclaw setup [PATH]     install; --preview changes nothing, --mac-only skips the VM
miniclaw resume           finish an interrupted VM setup
miniclaw ledger           show the shared VM budget; --reconcile repairs it
miniclaw update           rebuild the backend, the page and the VM scripts in place
miniclaw update-vm        rebuild the VM's setup disk; --computer-use adds the helper
miniclaw where            print the installation these commands use
```

Give a path only for an installation other than the remembered one, for example
`miniclaw start ~/Other-MiniClaw`.

The VM is optional and off by default on every Mac. Choosing it downloads about
20 GiB, reserves 64 GiB of disk, builds the VM and boots it once so you can finish
macOS setup and sign in to Codex inside it. After that it boots only when a project
asks for it. If VM setup is interrupted, `miniclaw resume` continues. One budget for CPU, RAM and disk is shared by every VM on this Mac; `miniclaw ledger`
shows it, and `miniclaw ledger --reconcile` releases a VM whose supervisor
died. Setup checks that budget, and what is already downloaded, before it builds
anything. Ctrl-C requests shutdown; a VM that has not stopped after 90 seconds is
powered off with a warning and exit code 2, and unsaved guest data may be lost.
See [installation](installation/README.md).

## Local network boundary

MiniClaw binds only to `127.0.0.1`, never a LAN address or `0.0.0.0`. There is no
host override, public tunnel, separate development server, or Codex WebSocket
listener. The API accepts only its exact host and browser origin; mutations also
require JSON and a custom header. Other local processes are outside this boundary.

When fresh browser OAuth is needed, Codex may briefly open its own localhost
callback listener. Its [callback implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs)
also binds to `127.0.0.1`. This is separate from MiniClaw's one persistent listener.
These restrictions apply to MiniClaw, not every application running on the computer.

## Codex runtime

`CODEX_BIN` overrides discovery in `/Applications/Codex.app` and
`/Applications/ChatGPT.app`. The executable is needed, but its desktop UI is not. MiniClaw uses
private stdio, not an existing desktop session or an open WebSocket port.

Codex manages OAuth and tokens in its normal credential store. MiniClaw does not
read credential files or send tokens to the browser. Its own chat workspace is
`~/Library/Application Support/MiniClaw/chat-workspace`; override its parent with
`MINICLAW_DATA_DIR`. Process-only settings disable environment tools and inherited
integrations without rewriting shared Codex settings. Custom Codex model providers
are not supported.

A provider that exits on its own is started again after 1, 2, 5 and then every 30
seconds, unless you disabled it or its executable is missing. The UI's Retry action
rechecks the connection. Inside a VM the guest user's
launchd job restarts a failed worker. Requests are never resent automatically.

## Claude Code runtime

Claude models appear in the model selector beside Codex's when the `claude` CLI is
installed. MiniClaw finds it at `~/.local/bin/claude`, `~/.claude/local/claude`, or on
PATH; `CLAUDE_BIN` overrides this and an empty value disables it. The model chosen for
a new chat pins that chat to Claude or Codex; an open chat only offers its own
provider's models.

An in-process bridge (`server/src/claude.rs`) presents the CLI through the same App
Server protocol subset the backend already uses. Each turn runs
`claude -p --output-format stream-json` with `--safe-mode`, `--tools ""`,
`--strict-mcp-config`, and the chat's instructions as the whole system prompt. The CLI
uses its own subscription sign-in (`claude auth login`); `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_BASE_URL` are removed from its environment.
The CLI keeps the model-side session (resumed by chat ID); MiniClaw keeps the visible
transcript in `claude-threads/` under its data directory, because the CLI's session
files are not a documented format.

## Providers

Codex and Claude Code are independent providers. Either one alone is a working app;
each has its own sign-in, and a chat stays with the provider of the model it was
started with. The Providers panel (plug icon) shows each provider's install and
sign-in state and enables or disables it without a restart. Choices persist in
`providers.json` in the data directory. `CODEX_BIN` and `CLAUDE_BIN` override binary
discovery; an empty value disables that provider. Codex is discovered in
`/Applications/Codex.app` or `/Applications/ChatGPT.app`.

Agents, projects, skills and the task list need any signed-in provider. Task routes
need the sign-in of the task's own provider. Usage and Computer Use are Codex-only.
Claude chats have no VM execution.

## Host tools and the Seatbelt sandbox

A host chat bound to an agent or a project, and every VM chat, gets one tool,
`miniclaw_scoped_native`, with `list_folder`, `search`, `read_file`, `write_file`,
`replace_in_file` and `run_shell`. `search` matches literal text and follows no link;
`replace_in_file` changes a passage only when it occurs exactly once. Listings and
searches are bounded (500 entries, 200 matches, 5,000 files, 1 MiB per file). Codex receives it as a dynamic tool;
Claude receives the same tool through a stdio MCP shim (`miniclaw-server
--tool-bridge`), which relays each call to the backend as the same `item/tool/call`
request. Both providers therefore share one consent flow and one sandbox. Native
provider tools stay off.

Every call is checked against the agent, project and conversation permission rules.
A `folder` rule covers the files beneath it; an exact file rule, including a
conversation block, takes precedence. Anything not allowed raises the approval
popup. Shell commands always need approval of their exact text and working
directory, and then run under `/usr/bin/sandbox-exec` with a profile generated for
that call (`server/src/seatbelt.rs`): deny by default, system toolchain readable,
granted files and folders readable or writable as granted, blocked files denied,
no network. Paths are passed as profile parameters, never as profile text. Provider
sign-in files (`~/.codex`, `~/.claude`, `~/.claude.json`, the Keychain folder) and
MiniClaw's own state (data folder, installation, VM link folder, ledger) are closed to
every grant, so granting a home folder does not expose them. A
revoked rule fails the next command because the profile is rebuilt every time.

AppleScript and TextEdit actions are refused on every machine: Apple Events make another,
unsandboxed app act, which Seatbelt cannot contain. A VM chat's tool calls run in that
VM's own worker under the same kind of profile, with the grants sent along.

Providers keep no tools of their own, on purpose. Probes showed that a profile wide
enough for a provider CLI to sign in lets the agent's shell read that sign-in, that
Codex's own sandbox reads every file, and that Seatbelt profiles cannot be nested. See
[the native tools probe](native-tools-probe/README.md). `sandbox-exec` is marked
deprecated by Apple but is the same mechanism Codex's own sandbox uses. File
metadata (`stat`) is readable everywhere so path resolution works; directory
listings and file contents are not.

## Opt-in VMs with the backend on the Mac

The backend is meant to run on the Mac, where chat, Claude, provider settings and the
Seatbelt tool have no VM in their path. VMs are an opt-in executor for untrusted
project work. A host installation whose `settings.json` has `pair.mac` starts a VM
session that boots nothing: it opens the private broker and control sockets and writes
a private link file, which `start-host.mjs` passes to the backend as
`MINICLAW_VM_LINKS`. The Mac's own Codex then runs behind the worker protocol
in-process so it also answers VM control; Master and project VM workers attach over
their private Unix sockets when a task starts them. A VM task never falls back to the
Mac. Without `pair.mac` none of this is active.

Status: the backend, launcher and installer are covered by tests that use the real
broker, control module and worker binary with a stand-in VM process. No real VM has
been booted in this mode. The Linux controller that used to host the backend is
removed. Computer Use works on the Mac and in the main VM, each with its own helper.

## What else is here

Agents can delegate to each other, run on a schedule, and work under a coding session that
shows every change for review; the right-hand panel shows a conversation's files, git changes
and activity. Messages typed during a response wait in a queue, and a model can ask you a question in a popup with options and room for your own answer. MiniClaw is the client of your
MCP servers, local ones sandboxed, for both providers, and `/mcp-create` builds one with you.
What an agent may use (web search, network, MCP servers and more) is set per agent in Settings,
with a main agent for plain conversations, and switched per conversation from the composer's
Tools dropdown. Start from [PLAN.md](../PLAN.md) for the design and [docs/](README.md) for each
feature. `miniclaw verify` checks a running installation and its VM, feature by feature.

## Current limits

The Computer Use switch in the composer's Tools dropdown enables experimental Computer Use in
the current chat using the installed Codex helper. Switch between messages. Tool settings
reload in the backend without reloading the page or clearing the draft. Failed steps
show an inline error and retry; interrupted archives have restart recovery. Tasks
with descendants cannot switch. Native approvals, cancellation, and discovery remain.
The helper's caller-authentication failure was fixed by preserving its direct Codex
parent. Real Calculator and browser-canvas vision/action checks passed. Native grant
lifetime, Stop during an action, and a first-click coordinate failure still need
testing. `GET /api/computer` currently reports whether
the helper executable was found, not whether the native service will authorize it.
`MINICLAW_HELPER_BIN` can select an installed helper path on the Mac. VM setup can
package the existing vendor-signed helper into guest installation media. It
copies application code only, not credentials or OS permissions. See
[Computer Use](computer-use/README.md).

Saved tasks appear from Codex's local history. The browser remembers the selected
task ID, and older messages load on request. No second transcript store is created.
Drafts and loaded messages survive a failed event stream while the browser refreshes
account and task state, then opens a fresh snapshot stream. Recovery does not reload
the page or resend a message. Drafts are not saved across page reloads.

Saved chats can be reopened and continued after restart. They resume as normal chats,
with computer access off. The real-runtime test verifies that a previously available
fake computer tool cannot execute after this change. Use the same-chat toggle to
enable computer access again. A task that is still responding can be opened: its partial text
is shown once and replaced by the whole message when it arrives.

Assistant messages render as inert Markdown with raw HTML escaped; your own messages stay
plain text ([message Markdown](message-markdown/README.md)). The embedded agent runtime is
deferred.

Account plan and usage appear when Codex supplies them. Missing usage remains
unavailable rather than showing zero. Existing ChatGPT authorization and a real
streamed conversation were verified. Fresh guest device OAuth and retained sign-in
across two VM starts passed live tests without logging out the host account.

A running VM gets a filtered virtual network interface for public IPv4 TCP/UDP, DNS and
DHCP, plus a checked public HTTPS proxy for Codex. No guest has a host folder share.
These network results were measured under the earlier Linux-controller layout, whose
VM side is unchanged: installed guest HTTP, certificate-verified HTTPS and Safari checks passed, while a
host-LAN request timed out. Public outbound access can still carry uploads or
tunnels. Scoped agents remain blocked from broad Computer Use until native app and
file restrictions are enforced. VM shutdown hangs remain unresolved; the 30-second
policy bounds the wait.

## Checks

```sh
cargo test
npm run test:startup
npm run test:local
npm run test:conversation
npm run test:models
npm run test:claude
npm run test:host-tools
npm run test:mcp
npm run test:agents
npm run test:host-vm
npm run test:computer
npm run test:web
npm run check
npm run test:check
npm run build
```

`npm run test:local` checks both launch modes, one loopback listener, and child
shutdown. It uses POSIX `ps` and `lsof` (included on macOS) and disposable test data.
Other integration tests also open temporary loopback ports and close them afterwards.

`npm run test:ui` includes desktop/mobile history cases. `npm run test:capabilities`
requires the installed Codex executable and a local listener; it captures a request
to a fake inference server using a disposable Codex home, without a paid model call.

`npm run test:computer-runtime` also requires an explicit `MINICLAW_HELPER_BIN`.
It uses real Codex, disposable homes, and fake inference to inspect the executable
tool registry. The resume case uses a fake MCP helper and an independent call journal;
no native computer tool is called. Normal, computer, and restricted-resume checks pass.
An earlier resume failure was a fixture indexing error, corrected in TESTING.md. The
older capability test is a separate diagnostic with outdated wrapper-name assumptions.

See [TESTING.md](../TESTING.md) for results and test-first evidence,
[PLAN.md](../PLAN.md) for the implementation contract, and [TODO.md](../TODO.md) for
unfinished work. Dated entries in those files from before 2026-09-19 describe the
Linux-controller layout, which has been removed. Automatic replacement of a failed
provider process and a real-VM check of the Mac backend remain open.
