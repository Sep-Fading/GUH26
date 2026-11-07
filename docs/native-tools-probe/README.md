# Native tools probe

Could Codex's own shell and Claude Code's own Bash, Read and Edit run inside the Seatbelt
profile MiniClaw generates? These probes ran on 2026-09-20 on macOS 26.5.1 with Codex
0.154.0 and Claude Code 2.1.278. No product code depends on them. See the
[sequence](sequence.md).

## Findings

| Question | Result |
| --- | --- |
| Does the Claude CLI run under `sandbox-exec` with its Keychain sign-in? | Yes. It also needs `/private/tmp/claude-<uid>`, its install and state folders, and the security mach services. |
| Are Claude's own Bash and Read tools confined by that profile? | Yes. Both failed on a file outside the grants: `Operation not permitted` and `EPERM`. |
| Can its network be limited without hostname rules? | Yes. With outbound traffic allowed only to a local allowlisting proxy and `HTTPS_PROXY` set, the CLI reached `api.anthropic.com`; the agent's `curl` to another host got `403` through the proxy and no route without it. |
| Can the agent's shell read Claude's sign-in token inside that profile? | **Yes, silently.** `security find-generic-password -s "Claude Code-credentials" -w` returned the credential JSON with no Keychain prompt. Removing the security mach services blocks it, and also stops the CLI from signing in. |
| Does Codex's own sandbox (`codex sandbox`) confine its shell? | Writes outside the working directory and all network are blocked. **Reads are not**: `~/.codex/auth.json` and `~/.ssh` were readable. |
| Can an outer MiniClaw profile contain Codex's inner sandbox? | **No.** Under any outer profile with a `deny` rule, the inner `sandbox-exec` fails with `sandbox_apply: Operation not permitted`. Nesting works only when the outer profile allows everything. |

## What this means

A provider CLI needs its own credentials, and a Seatbelt profile applies to a process and
everything it starts. So a profile wide enough for the CLI is wide enough for the agent's
shell to read that provider's sign-in. Codex has the same shape: its sandbox reads
everything, so a command can print `auth.json` into the conversation, and no outer
profile can be added around it.

MiniClaw's own tool does not have this problem because it runs in a separate process from
the provider. The executor's profile names no security service:

```rust
// server/src/seatbelt.rs, BASE
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))
```

`the_keychain_is_unreachable` in [`server/tests/seatbelt.rs`](../../server/tests/seatbelt.rs)
creates a throwaway Keychain item, shows it is readable outside the sandbox and unreadable
inside it, and deletes it.

## Recommendation

Keep providers toolless and keep the one MiniClaw-owned tool. Widen that tool (list a
folder, search, apply a patch) instead of enabling native tools. Native tools would need
the provider to run each tool in a process sandboxed separately from itself, with a
profile MiniClaw supplies. Neither CLI offers that today.

## Fixed-function tools (2026-09-20, second probe)

After the user ruled out the providers' shell and file tools for good, a second probe
asked whether the tools that cannot be pointed at a file or a command can be enabled
alone. It ran against Claude Code 2.1.278 with the real sign-in, started the way MiniClaw
starts it (`--strict-mcp-config --setting-sources "" --disable-slash-commands`).

| Question | Result |
| --- | --- |
| Does `--tools "WebSearch,Task,WebFetch"` give exactly those tools? | Yes. The CLI's `init` event lists `Task`, `WebFetch`, `WebSearch` and no MCP server. `TodoWrite` is not a tool name in this version. |
| Does a subagent get shell or file tools back? | No. A subagent reported `Agent`, `WebFetch`, `WebSearch`. Asked to read a marker file and run `pwd` by any means, including launching another agent, no tool outside that set was called and the marker's content never appeared. |
| Will `WebFetch` reach a local address? | **Yes, by IP address.** `http://127.0.0.1:<port>/` was upgraded to HTTPS and connected (the plain-HTTP test listener answered with a TLS error). `localhost` was refused by the tool ("hostnames without a dot"). A local or LAN service that speaks HTTPS would be readable. |
| Can `WebFetch` be confined? | Yes. With `--allowedTools "WebFetch(domain:example.com)"`, `example.com` was fetched and both `example.org` and `https://127.0.0.1:8443/` were denied. |

So for Claude: web search and restricted subagents can be enabled as they are. Web fetch
may only be enabled with per-domain allow rules, never unrestricted; the natural source for
those is the host and domain rules the user already approves for shell commands. A denied
fetch cannot raise a MiniClaw popup, because the decision happens inside the CLI; the model
reports the refusal and the user adds the host in the permission dialog. The fetch is made
by the Claude process, not through MiniClaw's proxy, so an approved domain that resolves to
a private address is not caught by MiniClaw.

Further findings from the same day, each from a real call:

| Question | Result |
| --- | --- |
| Does Claude Code have planning tools? | Yes, under other names. `--tools default` lists `TaskCreate`, `TaskGet`, `TaskList` and `TaskUpdate`, a to-do list. Named alone they ran without any permission prompt. The earlier "no planning tool" conclusion was wrong: `TodoWrite` is simply not the name in this version. |
| Does plan mode work in MiniClaw's non-interactive mode? | Yes. `--permission-mode plan` is accepted with `-p`; the model described its plan and called no tool. |
| Does `WebFetch(domain:python.org)` cover subdomains? | **No.** It matched `python.org` and refused `www.python.org` and `wiki.python.org`. |
| Is there a wildcard form? | Yes. `WebFetch(domain:*.python.org)` allowed `wiki` and `www` and still refused `wiki.notpython.org`. |
| Can fetch reach hosts with no rule at all? | **Yes, a built-in list.** `developer.mozilla.org` and `docs.python.org` were fetched with no matching rule. Claude Code pre-approves a set of documentation sites; MiniClaw cannot switch that off. |
| Codex with `web_search = "live"` and MiniClaw's policy otherwise unchanged (`gpt-5.6-luna`, low effort)? | It searched the web and answered. Asked to read a marker file and run `pwd` by any means, it reported having no file or shell tool, and the marker never appeared in the transcript. |
| The same Codex run without disabling inherited MCP servers? | **It read the marker file**, through the `node_repl` server from the user's own `~/.codex/config.toml`. MiniClaw is not affected: [launch](../../server/src/runtime.rs) reads Codex's effective configuration and relaunches with every inherited server disabled by name. The run shows what that step prevents. |

MiniClaw acts on this. Since 2026-09-21 web search is its own capability and is on by default
(the inverted switch `no_web_search` turns it off), so a plain Claude turn names `WebSearch`
alone. Since 2026-09-22, Web fetch (`web_fetch`), Subagents (`subagents`) and Task tracking
(`task_tracking`) each need their own agent/project allowance and conversation switch
([capabilities](../capabilities/README.md)). The former `web_tools` bundle is no longer used.
[Bridge::command](../../server/src/claude.rs) includes only the selected tools: `WebFetch`,
`Task`, and `TaskCreate,TaskGet,TaskList,TaskUpdate`, respectively. Search remains independent.
A host rule becomes one exact `WebFetch(domain:host)` rule; a domain rule becomes that and
its `*.` form; blocks become deny rules ([`claude_tools`](../../server/src/api.rs)). Fetch
rules are passed only when fetch is on. A separate `plan_mode` switch adds
`--permission-mode plan` and names no tool. Safe mode removes built-in tools, so a turn
that names any of them loads no settings files instead (`--setting-sources ""`). With all
built-in tools off, and no MiniClaw tool, the CLI runs with `--tools ""` and `--safe-mode`.
The earlier real CLI check inside MiniClaw answered a question using web search.

For Codex chats the Web search switch sets `web_search` to `"live"`. Codex takes configuration
only when a thread starts or is resumed (its `TurnStartParams` has no `config`), so the
switch applies to a new conversation from its project's or agent's default, and to an
existing one the next time it is loaded. `multi_agent` stays off; it was not probed.

## Rerunning

```sh
MINICLAW_NATIVE_PROBE=1 node --test tests/experiments/native-tools.test.mjs
```

This runs the two checks that need no model call and touch no secret: nested profiles, and
what `codex sandbox` allows (only a byte count leaves the sandbox). The first probe's
Claude CLI checks were run by hand because they use the real sign-in and a model call.

```sh
MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/native-fixed-tools.test.mjs
```

This runs the fixed-function probe. It makes real model calls on the signed-in Claude
account, about 0.15 USD in total, and is skipped without the variable. With the same
variable, `tests/experiments/claude-web-tools.test.mjs` runs one real Claude turn with web
tools on through a real backend. `MINICLAW_REAL_NETWORK=1 cargo test --test seatbelt
a_real_download` downloads real package metadata through the command proxy.
