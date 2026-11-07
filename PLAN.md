# MiniClaw Mini plan

Written 2026-11-07. **Linux runs** (2026-11-07, see "Status on Linux"); macOS and Windows are
untouched copies of MiniClaw's code and unchecked in this repo. Status words, as in
MiniClaw: *Observed* (ran against the real thing), *Tested* (automated, against fixtures),
*Planned*.

## What it is

A local harness for AI agents, for one person on one machine running Linux, macOS or Windows. It
gives Codex and Claude Code one chat page, saved agents with their own instructions and permissions,
one sandboxed tool for files and commands, MCP servers, skills, and delegation between agents.

It has no account of its own and listens on `127.0.0.1` only. It is a reduced copy of MiniClaw
(`../miniclaw-windows`), with code ported from there.

## Status on Linux (2026-11-07)

Approach changed on the user's "get a linux version running": the whole MiniClaw 0.1.8 source
was copied in (not ported piece by piece), the Roblox kit removed, and Linux added. The left-out
features below are still in the code and are to be cut next.

*Observed on Arch Linux (kernel 7.2.14, x86_64)*: chat with real Codex and Claude, an agent edit
and an approved sandboxed command, a refused read, delegation in every provider pairing (wait,
background, a delegated agent's approval in the parent, status). *Tested*: the Linux sandbox
suite, MCP servers with local and listening ports, the shared shell under zsh and bash.

Linux-specific decisions, all *tested*:

- Sandbox: `server/src/sandbox_linux.rs`, behind the existing `seatbelt` interface.
- Codex is found in `~/.local/bin`, then on `PATH`, as Claude already was.
- Data folder `$XDG_DATA_HOME/miniclaw` (`~/.local/share/miniclaw`); MCP secrets in its private
  `mcp-secrets.json`; deleted items in its sibling "Recovered files" folder, as on Windows.
- Shell: zsh where installed, else bash (`--noprofile --rcfile`), else no shared shell.
- The page and the model's briefing name "this computer", Linux, and the bubblewrap sandbox.

## In scope (user decision, 2026-11-07)

1. Chat with Codex and Claude Code through one page, using their existing sign-ins.
2. The sandboxed shell and file tool, with user approvals.
3. Agents with instructions, model and permissions.
4. Delegation between agents.
5. MCP servers and skills.
6. Linux, macOS and Windows.

## Left out

The Roblox development bundle, VMs, Computer Use, the setup wizard, schedules, backups, the
terminal, the workspace panel, analytics, projects, attachments, the message queue, slash commands
and timed unsandboxed shell grants. Any of these can be brought in later as its own milestone.

## Rules carried over from MiniClaw

1. **A model's command can never read a provider sign-in.** The providers' own shell and file tools
   are off. Everything goes through MiniClaw's tool, and its sandbox always hides `~/.codex`,
   `~/.claude`, `~/.claude.json`, the OS credential store and MiniClaw's own data, whatever was
   granted.
2. **Existing sign-ins are reused.** Tokens never reach the page, events, logs or storage.
3. **No network by default.** A command gets network only when the conversation's switch is on and
   the user approved the exact hosts: HTTPS on 443 through MiniClaw's proxy, private addresses
   refused.
4. **The user approves; models cannot.** No model message can create, edit or answer a permission.
5. **A partial result is never called finished.**
6. **If the sandbox is unavailable, the tool is unavailable.** A command never falls back to
   running unsandboxed.
7. **Tests come first.**

## Layout

| Part | What it is | Ported from |
|---|---|---|
| `server/` | Rust, Axum. Owns all state (SQLite) and every decision | `server/src` |
| `server/src/sandbox/` | One interface, three backends: `linux.rs`, `macos.rs`, `windows.rs` | `seatbelt.rs`, `executor*.rs`, `windows_*.rs`; Linux is new |
| `web/` | Svelte 5, TypeScript, Tailwind v4, Bits UI, Lucide | `web/src`, reduced |
| CLI | `miniclaw start`, nothing else at first | `scripts/cli.mjs`, reduced |

## Providers

*Planned. Codex 0.166.0 and Claude Code 2.1.319 are installed on the Linux development machine.*

- **Codex:** the backend launches `codex app-server` as its own child and speaks JSON-RPC over
  stdio (port `runtime.rs`, `rpc.rs`, and `history.rs` with its paged reads and oversized-frame
  handling from 0.1.8). Codex's own shell and file tools are off.
- **Claude Code:** one CLI run per turn with stream-json (port `claude.rs`, `tool_bridge.rs`). Its
  native tools are off; MiniClaw's tools reach it through the MCP bridge.
- **Turn endings** keep the provider's reason, not just a status. Codex gives `turn.status`
  (`completed`, `interrupted`, `failed`) and `TurnError.codexErrorInfo`; Claude gives the result
  `subtype`, `is_error`, `terminal_reason`, `api_error_status`. Each failed turn records
  `{provider, code, httpStatus}` beside its message. This is new; MiniClaw keeps only
  `usageLimitExceeded`.

## The sandboxed tool

*Planned.* One tool, `miniclaw_scoped`, with `read`, `write`, `list` and `run_shell`. The
permission store and approval popup are ported from `permissions.rs` and `scoped.rs`. Every
command runs under the OS backend with the grants the user approved. The egress proxy
(`egress.rs`) is shared by all three backends.

### Linux backend (new)

Built on bubblewrap, the approach Codex itself uses on Linux ("bubblewrap for filesystem and
namespace isolation, and seccomp for syscall filtering", codex-rs/linux-sandbox README). Unlike
Codex's default `--ro-bind / /`, the view is built from an empty root so that nothing outside it is
readable:

```
bwrap --unshare-all --die-with-parent --new-session --cap-drop ALL --clearenv
      --ro-bind /usr /usr  (plus /bin, /lib, /lib64, /sbin as symlinks or binds, as the host has them)
      --ro-bind /etc /etc  --proc /proc  --dev /dev  --tmpfs /tmp  --tmpfs $HOME
      --ro-bind <read grant> <same path> ...  --bind <write grant> <same path> ...
      --tmpfs ~/.codex  --tmpfs ~/.claude  --ro-bind /dev/null ~/.claude.json  (masks re-applied last)
      --chdir <cwd>  -- /bin/sh -c <command>
```

- No network: `--unshare-all` includes `--unshare-net`, leaving only a loopback device.
- Approved hosts: the egress proxy listens on a Unix socket bound into the sandbox. A small relay
  (a subcommand of the MiniClaw binary, run first inside the sandbox) listens on `127.0.0.1:<port>`
  in the sandbox's network namespace and forwards to that socket; the command gets `HTTPS_PROXY`.
  This is the same shape as Codex's "TCP->UDS->TCP routing bridge".
- Requirements: `bwrap` at `/usr/bin/bwrap` or `/bin/bwrap` (never looked up on `PATH`) and
  unprivileged user namespaces, probed once per start. If either is missing, the tool reports why
  and refuses (rule 6).
- Also in place: `--disable-userns` (no nested namespaces), `--cap-drop ALL`, `--new-session`
  (none for the terminal, which needs its tty), `--die-with-parent`.
- MCP servers: `Bridges` forwards each declared local port through a Unix socket, and each
  declared listening port from this machine's loopback into the sandbox. An undeclared listener
  binds inside the sandbox's own network only, so nothing on the machine can reach it.
- A granted folder that is deleted and made again restarts the shared shell, because bubblewrap
  binds the folder itself, not its name.
- Known gap: a protected path that does not exist yet (for example `~/.codex` before Codex was
  ever signed in) is not masked, so a command with a write grant on its parent could create it. *To verify:* Ubuntu 24.04's AppArmor restriction on
  unprivileged user namespaces.
- Later layer, not in the first version: a seccomp filter (block `io_uring`, `AF_VSOCK` and new
  `AF_UNIX` sockets), and Landlock where the kernel has it.

The development machine has bubblewrap 0.13.1, kernel 7.2.14 with Landlock enabled, and
`kernel.unprivileged_userns_clone = 1` (checked 2026-11-07).

### macOS backend

Port `seatbelt.rs` and the Mac half of `executor.rs` unchanged in behavior.

### Windows backend

Port the Windows sandbox from `miniclaw-windows` (about 13,500 lines across `windows_*.rs`,
`executor_windows.rs`, `secrets_windows.rs`, `platform_fs_windows.rs`, `powershell.rs`). It is the
largest piece, and it can only be checked on a Windows machine, so it gets its own milestone.

## Agents, skills, MCP servers

*Planned.* Port `agents.rs` (instructions, model, permissions, pinned revisions), `skills.rs`,
`mcp_servers.rs`, `mcp_client.rs` and `mcp_gateway.rs`. An MCP server runs sandboxed by default, as
in MiniClaw.

## Delegation

*Planned.* Port `api/delegation.rs` and `delegations.rs`: `miniclaw_delegate`, the
Wait/Background/Decline prompt, per-agent limits, `status`, no nesting, the restart rule ("closed as
failed, not run again"). New: a failed run's record carries the provider's reason code from
"Turn endings" above, and `status` returns it, so the lead agent can tell a temporary failure
(overloaded, stream disconnected, rate limit) from a permanent one.

## Milestones

| # | Milestone | Checked on |
|---|---|---|
| 0 | Repo, backend health route, page shows "Connected" | Linux |
| 1 | Codex and Claude chat with saved history and turn-ending reasons | Linux |
| 2 | Sandboxed tool, permissions, approvals; Linux backend; egress proxy | Linux |
| 3 | Agents | Linux |
| 4 | Skills and MCP servers | Linux |
| 5 | Delegation | Linux |
| 6 | macOS backend | a Mac |
| 7 | Windows backend | a Windows machine |

The code is written for all three from milestone 0 (platform code behind `cfg` and the sandbox
interface), and CI builds it on all three. Running it on macOS and Windows waits for milestones 6
and 7.

## Decisions, by date

| Date | Decision |
|---|---|
| 2026-11-07 | New sibling repo `miniclaw-mini`; scope as above; Linux, macOS and Windows (user) |
| 2026-11-07 | Linux sandbox: bubblewrap from an empty root, network namespace, proxy reached through a Unix socket relay (built and tested) |
| 2026-11-07 | "Get a linux version running, dont worry about the rest for now" (user): copy MiniClaw whole, add Linux, cut features afterwards |
| 2026-11-07 | Failed turns and delegated runs keep the provider's reason code (recommendation, awaiting review) |
