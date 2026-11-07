# Computer Use

Computer Use adds the installed native MCP helper to a chat's Codex tool configuration.
Its switch in the composer's Tools dropdown ([capabilities](../capabilities/README.md))
changes that configuration; it does not grant permission to
control an app. This page covers MiniClaw's integration, not the helper's internal
capture or input implementation. See the [sequence diagrams](sequence.md).

## Entry points

Routes are defined in [api.rs](../../server/src/api.rs), in `router`.
They require ChatGPT authentication. Mutations also require JSON, the
`X-MiniClaw-Request: 1` header, and the local origin checks enforced by `guard`.

[Defined agent chats](../agents/README.md) cannot enable this broader helper policy.
Both creation with `computerUse: true` and an enabling toggle request return HTTP 403
for an agent-bound chat. General MiniClaw chats retain the explicit opt-in below.

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/computer` | None | `available` reports whether the helper path was found, not whether access is granted. |
| `POST /api/threads` | Optional `computerUse`, with optional model and effort | Starts a chat with its initial tool configuration. |
| `POST /api/threads/{id}/computer` | `enabled`: boolean | Returns the same chat's snapshot after switching tools. |
| `POST /api/threads/{id}/approvals/{approval}` | `action`: `accept` or `decline` | Answers a valid pending approval and returns an empty JSON object. |

## Runtime and helper

[runtime::launch](../../server/src/runtime.rs) owns a Codex App Server connected over
private stdio. It applies process-local restrictions and disables inherited MCP
servers without writing shared Codex configuration.

[computer::helper](../../server/src/computer.rs) resolves `MINICLAW_HELPER_BIN` when
set; otherwise it looks under `CODEX_HOME`, falling back to `$HOME/.codex`. The path
must be absolute and point to a file. Discovery alone does not test native access.

`computer::configure` changes start or resume parameters to use `on-request` approval
with the user as reviewer. The helper registration below is an exact fragment from
[computer.rs](../../server/src/computer.rs), not a standalone program:

```rust
    config["mcp_servers.miniclaw_computer"] = json!({
        "command": std::env::current_exe().expect("MiniClaw executable path"),
        "args": ["--computer-helper", helper], "enabled": true, "required": true,
        "startup_timeout_sec": 10, "tool_timeout_sec": 120,
    });
```

[main](../../server/src/main.rs) dispatches `--computer-helper` to `computer::bridge`.
The bridge starts an input filter and replaces itself with the helper using `exec`,
keeping Codex as the helper's direct parent. The filter removes only the unsupported
`codex/auth-change` initialization capability. Helper output returns directly to
Codex. The [parent-preservation tests](../../server/tests/computer.rs) exercise this
process arrangement.

## Guest execution

In controller mode, [main](../../server/src/main.rs) asks the authenticated worker
for `miniclaw/computer/read`. It does not inspect local helper paths.
`computer::Target::Guest` adds only `miniclawComputerUse: true` to start/resume
parameters. The [guest policy](../../server/src/guest_computer.rs) removes that
marker and configures the helper using the worker's own executable and guest paths.
Controller-supplied MCP executable overrides are rejected before forwarding.
Restoring a normal chat sends `miniclawComputerUse: false`. The worker installs a
disabled entry with a guest-local transport shape, even if no helper is installed.

[worker::serve_with](../../server/src/worker.rs) handles capability replies locally
and forwards permitted frames to guest Codex. Local replies and Codex output share
a per-frame write lock. The existing frame limit, write deadline and child cleanup
apply to both. Approval decisions retain the existing controller checks.

The [worker entry point](../../server/src/bin/miniclaw-worker.rs) refuses non-VM or
root execution before dispatching helper/filter modes. Helper discovery reports
installation, not macOS permissions or native-service readiness. Defined agents
remain unable to enable the broader computer policy.
The worker sets its Codex child's `PATH` to the installed runtime directory followed
by macOS system directories. This does not change the user's login environment.

`prepareMasterMedia` accepts an optional local `computerApp`. It verifies OpenAI's
signing identity, archives application code and includes it on read-only setup media.
The guest installer verifies the extracted signature before installation. No host
credential store or permission database is copied. The archive avoids Finder
metadata being added inside the signed bundle by HFS media.

Policy and mounted-media tests pass. Live guest-native Calculator input produced391;
Safari input and canvas hit testing produced Passed:73 on the offline fixture.
Independent VM screenshots verified both results. Guest native permissions survived
a retained-pair restart. These checks do not establish app-level confinement.

On worker loss the backend keeps running. [monitor_worker](../../server/src/main.rs)
reconnects a VM's worker and reports `computerReady: false` until it is back, and the
provider supervisor ([providers.rs](../../server/src/providers.rs)) restarts a lost Codex.
With the retired Linux controller, the live worker-replacement test
verified reconnection with retained sign-in, unchanged history and working metadata
storage. No message is automatically resent.

## Computer Use in a VM

Each VM reports its own helper. The backend asks a VM's worker `miniclaw/computer/read`
when it attaches, keeps the answer per VM ([Backend::vm_computer_ready](../../server/src/chat.rs)),
and clears it when that worker closes. `assigned_computer` accepts any ready VM, Master or
project VM, and both VM status routes return `computerReady`. `GET /api/computer` adds
`host` for the Mac's own helper.

The page's toggle follows the machine the chat runs on: the Mac's helper for a Mac chat,
that VM's helper for a VM chat. When a VM lacks the helper the toggle is disabled and its
tooltip names `miniclaw update-vm --computer-use`, which puts the signed helper on the VM's
setup disk (see [Installation](../installation/README.md)).

A chat carries either Computer Use or `miniclaw_scoped_native`, never both. The guest
policy rejects the pair, so `configure_context` leaves the tool out when Computer Use is
being enabled:

```rust
// server/src/api.rs, configure_context
if !computer_use
    && let Some(assignment) = backend.agents.assignment(id.to_owned()).await?
```

Observed in a real guest on 2026-09-20: with the helper installed and Accessibility and
Screen Recording allowed for it in the VM, a VM chat opened Apple Notes.

## Switching an existing chat

[set_computer](../../server/src/api.rs) loads the chat, checks authentication, and
reserves it with `computer_changing`. Active, sending, uncertain, or already-changing
chats reject another switch. A request for the already-confirmed mode is a no-op
unless a previous switch left an error.

The installed Codex runtime keeps old tools when an already-loaded chat is resumed.
MiniClaw therefore archives, unarchives, then resumes the same task with new tool
parameters. This behavior is exercised by the [runtime probe](../../tests/computer-runtime.test.mjs).
`check_switch` verifies workspace/provider ownership, idle status, and absence of
unarchived descendants before archive. The descendant check prevents the workaround
from deliberately archiving child tasks; it is not a lock against independent clients.

`resume_params` supplies normal-chat restrictions. Enabling tools then applies
`computer::configure`; disabling keeps the reserved helper entry disabled. The
existing model, effort, task ID, and cached transcript remain in place.

## State and recovery

| Owner | State |
| --- | --- |
| [Svelte chat state](../../web/src/chat.svelte.ts) | Draft, confirmed `computerUse`, local `switching`, server `computerPending`, and `computerError`. |
| [Rust Chat](../../server/src/chat.rs) | Cached messages, tool mode, `computer_changing`, `computer_error`, and `restore_pending`. |
| [Recovery records](../../server/src/recovery.rs) | Empty UUID-named files under MiniClaw's `computer-recovery` directory. No transcripts or credentials. |
| Codex | Persisted task history and loaded tool configuration. |

The backend syncs a recovery record before archive and removes it after confirmed
unarchive. Restoration continues if the browser disconnects. A failed mutation keeps
messages cached and blocks sends while tool state is unresolved. Retry checks whether
the task is actually archived before repeating steps. After restart, `recover_tasks`
runs during first-page task listing and restores only recorded, ownership-checked tasks.
Saved chats subsequently load with computer tools off.

An account change prevents tool activation. If it occurs during resume, the backend
stops the owned Codex client. Automatic child replacement is not implemented; this
case requires restarting the backend.

`computer.updated` events carry tool state without replacing messages. The frontend
also applies the HTTP snapshot through this exact fragment of `applyComputer` in
[chat.svelte.ts](../../web/src/chat.svelte.ts):

```typescript
    this.computerUse = state.thread.computerUse === true;
    this.computerPending = state.computerChanging ?? false;
    this.computerError = state.computerError ?? '';
    if (this.active) this.active = { ...this.active, computerUse: this.computerUse };
```

[App.svelte](../../web/src/App.svelte) disables Send during switching but leaves the
draft editable. New chat creation is deferred until first Send because an empty
Codex task has no saved rollout to archive. Toggle changes on that empty composer
only select the initial mode.

## Approvals and cancellation

[Approval::new](../../server/src/computer.rs) accepts only empty confirmation forms
from `miniclaw_computer`. The [backend listener](../../server/src/chat.rs) also checks
the active task and turn, tool mode, switch state, existing approval, and Stop state.
Accepted requests reach the browser in a snapshot. Unsupported requests are declined.

`decide` in [api.rs](../../server/src/api.rs) rejects expired or mismatched approvals
and decisions after Stop. Browser decisions expire after 55 seconds; the
[RPC wait](../../server/src/rpc.rs) defaults to decline on timeout after 60 seconds or
when its answer channel closes. Stop drops the pending approval and requests
`turn/interrupt`; its acknowledgement does not mark the turn complete. Stop cannot
undo an action already performed.

An expired approval returns HTTP409 with `approval_expired`, including when Stop or
turn completion invalidated it. The browser displays the supplied error; it does
not automatically retry the decision or resend the message.

`Chat::notify` updates unfinished activity when the turn ends. An interrupted turn
sets its display status to `interrupted`; other missing tool results become `unknown`.
Already completed entries stay unchanged. A snapshot updates the browser immediately.
This does not establish a tool's physical effects or undo actions already performed.

A failed step says why ([`computer.rs`](../../server/src/computer.rs) `failure`). Codex's own
`error.message` is always shown. A failed result's text is shown only when it begins with
`Computer Use server error`, the helper's own error line, because any other text may describe a
window. The text is cut at 300 characters. Two codes get a plain explanation as `hint`:

| Code | Seen | Hint |
|---|---|---|
| `-10005` | 2026-09-21: the installed helper (26.913) starts its own `codex` from `/Applications/Codex.app`, which the ChatGPT desktop app had replaced | open the ChatGPT desktop app once so it updates the helper, keep it open, try again |
| `-10000` | 2026-09-11: `Sender process is not authenticated` | keep the ChatGPT desktop app open and try again |

That the desktop app updates the helper is an expectation, not an observation. The page opens
"Computer activity" by itself while an entry has an error.

## Regression coverage

- [Toggle API tests](../../tests/computer-toggle.test.mjs): identity/history preservation,
  concurrency, recovery, descendants, malformed requests, and account changes.
- [Approval API tests](../../tests/computer.test.mjs): decisions, Stop, expiry,
  cross-chat rejection, and activity without raw tool-result exposure.
- [Browser tests](../../tests/browser/computer.spec.ts): retained draft and transcript,
  unchanged scroll position, no navigation during toggling, and inline retry.
- [Runtime probe](../../tests/computer-runtime.test.mjs): executable tool inventory
  and fake-helper revocation. These fixtures do not establish native grant lifetime.
