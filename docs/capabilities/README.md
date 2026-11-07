# Capabilities, the main agent and the Tools dropdown

A capability has two levels. An agent's configuration says whether it is allowed at all. The
conversation switches what is allowed on or off. A conversation cannot switch on what its agent
does not allow.

Sequence: [sequence.md](sequence.md).

## Before the first message

The Tools dropdown works in an empty composer. `Chat.draftCapabilities` in
[`chat.svelte.ts`](../../web/src/chat.svelte.ts) holds explicit choices in page memory.
Opening or switching a tool creates no provider conversation. A new chat or a change of
agent, provider or machine clears these choices. A rejected creation preserves the prompt
and choices for an explicit retry.

`Chat.create` includes the choices in `POST /api/threads`:

```ts
...(Object.keys(this.draftCapabilities).length ? { capabilities: { ...this.draftCapabilities } } : {}),
```

[`api/capabilities.rs`](../../server/src/api/capabilities.rs) `prepare` validates each key
against the selected agent and project, the MCP server's provider, and VM search availability.
Unknown keys return 400; enabling a capability outside these bounds returns 403 before
provider creation. The preview route accepts `agentId`, `projectId`, `claude` and `vm` for
drafts; an existing `thread` supplies its own stored context.

[`api.rs`](../../server/src/api.rs) `create_chat` applies the web search choice to the initial
Codex configuration, because Codex takes that setting at start or resume. Other switches
are conversation permission rules. [`agents.rs`](../../server/src/agents.rs) `reserve` saves
them in the same transaction as the provisional bindings, before contacting the provider:

```rust
for target in tools {
    tx.execute("INSERT INTO permission_rules (scope_kind, scope_id, target, effect)
        VALUES ('conversation', ?1, ?2, 'allow')", params![thread, target.key()?])
        .map_err(Issue::storage)?;
}
```

[`agents/creation.rs`](../../server/src/agents/creation.rs) `check_creation` moves those
rules to the returned conversation ID with its other bindings. A failed finalization keeps
the receipt for recovery; a failed initial write prevents provider creation. The first prompt
therefore reads the saved choices. Both failure paths are covered by the creation recovery
test; provider fixtures and browser tests cover the first prompt.

## Enabled by default

Settings → Agents → an agent → Enabled by default lists the capability switches separately
from Permissions. [`AgentCapabilities.svelte`](../../web/src/AgentCapabilities.svelte) owns
the two tabs; [`CapabilityDefaults.svelte`](../../web/src/CapabilityDefaults.svelte) loads
and saves one switch at a time. An unavailable capability points to Permissions. This page
also works for the main agent. Save a new agent before configuring its permissions or defaults.

`GET /api/agents/{id}/capability-defaults` returns the switches and their allowed/default
states. `POST` accepts `key` and `on`; unknown keys return 400 and enabling a capability
without the agent's allowance returns 403. These routes use the same authorization and
agent-scope checks as permissions. Defaults grant no access and leave existing conversations
and running turns unchanged.

[`capability_defaults.rs`](../../server/src/capability_defaults.rs) keeps explicit on/off
preferences in the private metadata database, keyed by agent and capability. Named agents
inherit nothing from the main agent. The existing Full access by default control and this
page read and write the same `feature default_full_access` permission setting.

The draft preview uses these preferences. `prepare` in
[`api/capabilities.rs`](../../server/src/api/capabilities.rs) merges them before creation:

```rust
    for (key, on) in initial_defaults(backend, agent).await? {
        if choices.contains_key(&key) { continue; }
```

This fragment preserves an explicit Tools choice, including off. The following checks omit
saved defaults that exceed current permissions, belong to a different MCP provider, or enable
search in a VM. Deleted MCP servers are ignored. Explicit invalid choices still fail. The
resulting conversation rules commit with the initial bindings; web search's initial provider
configuration uses those resolved rules too. A default is not copied again on later messages.

Full access retains its existing limits; Coding session still reviews file changes. MCP
activation makes tools available but preserves their normal approval rules. Computer Use and
project memory are not agent-default switches on this page.

## The capabilities

These are the baseline values when no agent default or explicit Tools choice is saved.

| Key | Switch behind it | Allowed when | Default in a conversation |
|---|---|---|---|
| `web_search` | `feature no_web_search`, stored inverted | the agent and project have not set `no_web_search` | on |
| `network` | `feature network` | the agent or the project allows it | off |
| `web_fetch` | `feature web_fetch` (Claude: Web fetch) | the agent or the project allows it | off |
| `subagents` | `feature subagents` (Claude: Subagents) | the agent or the project allows it | off |
| `task_tracking` | `feature task_tracking` (Claude: Task tracking) | the agent or the project allows it | off |
| `mcp:<id>` | `mcp_server { server }` | the agent or the project allows it | off |
| `questions` | `feature no_questions`, stored inverted | always: asking grants nothing ([questions](../questions/README.md)) | on |
| `plan_mode`, `coding_session` | `feature plan_mode`, `feature coding_session` | always: they restrict, they do not grant | off |
| `ask_commands` | `feature ask_commands` | always: it restricts | off, so read-only commands run without asking |
| `full_access` | `feature full_access` | always: it stops the asking, never widens what a rule allows | off unless the agent enables its default |

Computer Use keeps its own route and rules ([computer use](../computer-use/README.md)); the
dropdown only shows it beside the others.

Web search runs at the provider, not on this Mac. Codex is launched with `web_search`
disabled ([`chat-policy.json`](../../server/chat-policy.json)), and `api.rs` sets `live` when a
thread starts or resumes unless search is off. By default a Claude turn names only `WebSearch` as a built-in tool. Safe mode would remove that tool, so such a turn loads no settings files
instead (`--setting-sources ""`); with search off and no other tool it runs in `--safe-mode`
as before ([`claude.rs`](../../server/src/claude.rs) `command`).

## Independent Claude tools

Web fetch, Subagents and Task tracking replace the former combined capability. Each has
its own permission rule and conversation switch. The UI offers them only for Claude.
`claude_tools` in [`api.rs`](../../server/src/api.rs) reads the effective choices on each
turn; `NativeTools::enabled_tools` in [`claude.rs`](../../server/src/claude.rs) builds the list:

```rust
        if self.search { tools.push("WebSearch"); }
        if self.fetch { tools.push("WebFetch"); }
        if self.subagents { tools.push("Task"); }
        if self.tracking { tools.extend(["TaskCreate", "TaskGet", "TaskList", "TaskUpdate"]); }
```

`command` adds host/domain allow and deny rules only with Web fetch. Claude Code's own
permitted documentation sites remain an exception to the host allow list. Subagents keep
the restricted tool set. Task tracking adds the checklist operations; Plan mode separately
adds `--permission-mode plan` without granting any tool. A changed choice applies to the
next Claude turn. At startup, `retire_web_tools` in [`permissions.rs`](../../server/src/permissions.rs)
removes the retired `web_tools` rules from saved permissions and session grants in one
transaction. Other rules stay intact; the new choices start off.

The same names appear in permission settings and `/agent-create` suggestions. Tests in
[`claude.test.mjs`](../../tests/claude.test.mjs) check exact tool selections and fetch rules;
[`capability-toggles.test.mjs`](../../tests/capability-toggles.test.mjs) checks independent
allowances, persistence and revocation. See the [sequence](sequence.md#claude-tool-selection).

## The ceiling

[`permissions.rs`](../../server/src/permissions.rs) `ceiling` names the bounded switches and
`configured` finds the project or agent that sets one. `decide_exact` reads them first:

```rust
return Ok(match (ceiling, set, own) {
    (Ceiling::Needs, None, _) => Decision::Ask,
    (Ceiling::Needs, Some(_), Some(Effect::Allow)) => Decision::Allow(Scope::Conversation(context.conversation)),
    (Ceiling::Needs, Some(_), _) => Decision::Ask,
    (Ceiling::Unless, Some(scope), _) => Decision::Allow(scope),
    (Ceiling::Unless, None, Some(Effect::Allow)) => Decision::Allow(Scope::Conversation(context.conversation)),
    (Ceiling::Unless, None, _) => Decision::Ask,
});
```
[`permissions.rs`](../../server/src/permissions.rs) `decide_exact`

The configuration is the bound, never the switch (requirement 2026-09-22): allowing network or
a server on an agent makes the row available in every conversation of that agent and moves
none of them; a separate saved default can start a new conversation with it on. Each conversation keeps its own choice
whatever the agent does later, except that taking the allowance away takes the capability from
every conversation that had it on (and gives it back, still on, if the allowance returns).
`set_permission` applies the same rule to a conversation's own rules: allowing a bounded switch
that is not configured answers 403 `capability_not_allowed`, "on" is the conversation's own
allow, and "off" is stored as no rule at all. Flipping a switch from Tools never ends the running
turn (`api/capabilities.rs` `toggle` saves the rule without the cancel that `change_permission`
does for a removed resource rule), and the page no longer greys the switches while a response
runs: the next tool call reads the switch, so a server switched off
mid-turn answers "not switched on" at its next call and works again once it is back on. Removing
an allowance in Settings still ends the turn, as every removal does.

## The main agent

A conversation with no agent belongs to the main agent
([`main_agent.rs`](../../server/src/main_agent.rs)). Its capabilities are permission rules under
the agent scope with the nil id, `00000000-0000-0000-0000-000000000000`, which no saved agent
can have. That scope accepts `feature`, `mcp_server` and `mcp_tool` targets only; a file,
folder, command, host or application rule answers 400. A named agent inherits nothing from it.

`GET` and `POST /api/main-agent` hold its default model and effort, its skills (8 at most) and
its skill groups (16 at most), in the table `main_agent`. A new conversation without an agent
starts with the model unless the request names one; a delegated run does not use it. In a
`POST`, a list that is left out is kept, so saving the model alone does not empty the skills.
The same row holds its delegation limits, `delegation: { maxPerTurn, minutes }` (defaults 3 and
10, bounds 1 to 10 and 1 to 1440), also kept when left out; see
[delegation](../delegation/README.md#setting-it-up).

The skills and groups are not pinned to a conversation. `turn_skills`
([`skill_groups.rs`](../../server/src/skill_groups.rs)) reads them when a message is sent in any
conversation with no agent, old ones included, ahead of the project's and the conversation's
own groups. An inactive or deleted skill, and an inactive group, are left out and never block
the message. What that costs at each provider is measured in
[skill groups](../skill-groups/README.md#what-repeating-the-text-costs).

Settings, Agents, Main agent ([`Agents.svelte`](../../web/src/Agents.svelte)) edits both. A
named agent's editor shows the same Capabilities group above its resource rules
([`PermissionSettings.svelte`](../../web/src/PermissionSettings.svelte)). The old read-only
list of `capabilities` and `access` fields appears only for an agent that still carries such
fields, which no release can enforce.

## Commands that do not ask

Two conversation switches decide how much a shell command asks
([`chat.rs`](../../server/src/chat.rs) `scoped_tool_result`, [`harmless.rs`](../../server/src/harmless.rs)):

- **Read-only commands** run without a popup when the whole command line is made of programs
  from a fixed list (`ls`, `cat`, `head`, `tail`, `wc`, `grep`, `rg`, `find`, `pwd`, `echo`,
  `which`, `file`, `stat`, `du`, `df`, `date`, `sort`, `uniq`, `cut` and a few more, plus
  read-only subcommands of `git`, `npm`, `cargo` and `node`), joined only by `;`, `&&`, `||` or
  `|`, with plain words as arguments. Anything the shell would expand, redirect, background or
  quote asks as before, as do write flags such as `find -delete`, `sort -o` and `tail -f`, a
  program named by path, and an assignment. The list is the rule; nothing is inferred from the
  words. "Ask about every command" in Tools turns this off for one conversation. A coding
  session outranks it.
- **Allowed programs.** When the popup asks about a plain shell line (one `harmless::programs`
  can split: programs joined by `;`, `&&`, `||` or `|`, no redirect, quote, substitution or
  background), it offers "Always allow sed, awk". That saves a lasting rule `program: <name>` for
  each program on the conversation's agent, or on the main agent for a plain conversation, and
  runs the command now. From then on any plain line whose every program is allowed runs without
  asking; a line with one unknown program, or one the shell would do more with, asks as before. The
  rules are listed and removable in Settings, Agents, and removing one interrupts the running turn
  as every removal does. The read-only list and allowed programs are the same mechanism with two
  sources: MiniClaw's fixed list, and the user's own choices.
- **Full access** is a conversation switch, off by default, that stops the asking: every
  command runs, every file and folder a rule allows is read or written, every MCP tool runs, and
  a host a rule names is reached, with no popup. It grants nothing: a block still blocks, and a
  path or host no rule names is refused instead of asked. The composer shows a red "Full access"
  badge while it is on, the environment note tells the model, and questions through the popup
  still work. With a coding session on beside it, the session keeps one thing: every file write
  or edit still waits for its review with the diff (`chat.rs` `reviews_changes`), while commands,
  reads, hosts and MCP tools run as full access says (`chat.rs` `coding` is the session without
  full access; `mcp_gateway.rs` reads full access first). So the pair means "ask me about file
  changes only". The sandbox is unchanged: MiniClaw's own files
  and the sign-ins stay closed whatever the switch says.

## The Tools dropdown

`GET /api/capabilities?thread=` lists each capability with `allowed` and `on`
([`api/capabilities.rs`](../../server/src/api/capabilities.rs)); with `agentId` and `projectId`
instead it answers for the conversation that choice would start. `POST
/api/threads/{id}/capabilities` with `key` and `on` switches one.

[`Capabilities.svelte`](../../web/src/Capabilities.svelte) is the one button in the composer,
"Tools", with a count of what is on. Each row has an icon, a name, one line of explanation and
a switch. A row that cannot be switched is greyed and says why on hover and to a screen reader:
not available for this agent (or the main agent), Claude conversations only, settings still
saving, or wait for the response to end (Computer Use only). The conversation's permissions dialog no
longer has switches; it keeps resource rules.

## Image generation

Host Codex conversations can use native image generation through the existing ChatGPT OAuth
session. Its usage counts against the subscription's Codex limits ([official documentation](https://learn.chatgpt.com/docs/image-generation)).
No API key is requested. Permissions exposes Image generation; Enabled by default seeds
new conversations, and Tools controls an existing conversation. Claude and VM conversations
cannot enable it. Computer Use and image generation are mutually exclusive.

[`api.rs`](../../server/src/api.rs) `configure_context` adds the native flag only when the
conversation's allowance and switch both permit it:

```rust
    if !backend.is_claude(id) && on_mac && image_generation(backend, id).await {
        params["config"]["features.image_generation"] = json!(true);
    }
```

An existing Codex thread keeps its loaded configuration, so
[`toggle_image_owned`](../../server/src/api/capabilities.rs) archives, unarchives and resumes
the idle thread. Failure blocks sends until the image-setting retry succeeds. Agent,
project or conversation permission changes that alter effective image access also require
this reload; removing access cancels affected active work.

[`chat.rs`](../../server/src/chat.rs) normalizes image notifications and strips raw base64
before conversation replay. [`attachments.rs`](../../server/src/attachments.rs) `generated`
accepts bounded PNG bytes, stores them privately, and deduplicates by conversation and item.
It ignores provider file paths. [`history.rs`](../../server/src/history.rs) restores the
same attachment when reopening. The page shows progress, the image, or a generation/save
failure. The20 MiB image limit fits within the32 MiB RPC line bound; a history page holding
many large images can still exceed that transport bound.

The [image sequence](sequence.md#image-generation) follows the native event path.
[`image-generation.test.mjs`](../../tests/image-generation.test.mjs) covers switches,
revocation, failure and restoration. Desktop/mobile browser tests and one real OAuth
PNG generation passed on2026-09-25. The opt-in
[smoke test](../../tests/experiments/real-image-generation.test.mjs) uses subscription quota.

## Tests

[`capability-toggles.test.mjs`](../../tests/capability-toggles.test.mjs),
[`creation-recovery.test.mjs`](../../tests/creation-recovery.test.mjs),
[`mcp-provider.test.mjs`](../../tests/mcp-provider.test.mjs),
[`main-agent.test.mjs`](../../tests/main-agent.test.mjs),
[`capabilities.spec.ts`](../../tests/browser/capabilities.spec.ts),
[`agent-capability-defaults.spec.ts`](../../tests/browser/agent-capability-defaults.spec.ts),
[`main-agent.spec.ts`](../../tests/browser/main-agent.spec.ts),
[`capabilities.test.ts`](../../web/tests/capabilities.test.ts). These use fixtures.

The initial-tool cases in [`real-e2e.test.mjs`](../../tests/experiments/real-e2e.test.mjs)
also passed with real Codex and Claude models: a first-prompt file write ran with full access
already enabled, without an approval popup. Search and questions were disabled at creation.

Web search is not available in a VM conversation: the VM's worker accepts only the bundled
policy's configuration values ([briefing, limits](../briefing/README.md#limits)).

Not observed: real Codex live search or real Claude web search through MiniClaw.

## Timed unsandboxed shell

`unsandboxed_shell` is a host-only exception to the normal shell sandbox. An agent or project
first allows it in Permissions. In any existing host chat, including the main agent, Tools accepts a
duration in minutes (1–1,440, initially 15) and a switch to grant access. It is excluded from
agent activation defaults and rejected in draft creation.

`toggle_shell` in [`api/capabilities.rs`](../../server/src/api/capabilities.rs) accepts
`{ "key": "unsandboxed_shell", "on": true, "durationSeconds": 900 }`. The duration is required
and bounded to 1–86,400 seconds. Off deletes the grant. A grant stores its original permission
context, unique ID and deadline in `Backend.shell_grants`; it is never saved to the database.
The list returns `expiresAt` in Unix milliseconds and `expired`. Restart clears grants.

[`shell_access.rs`](../../server/src/shell_access.rs) verifies the original grant, context,
agent/project allowance and active turn while running. A backend timeout bounds the whole
execution, including permission checks. Revocation is checked every 100 ms. The unrestricted
branch releases the permission gate so changing permissions cannot wait behind the command.

[`executor.rs`](../../server/src/executor.rs) runs a fresh zsh process without Seatbelt, with
a cleared environment and selected user/path variables. It never uses the shared terminal.
A process-group guard kills the shell and descendants in its group on completion or cancellation.
Output stays bounded. Existing command rules and Full access still control command approval;
read-only and program shortcuts do not skip approval in this mode. Command approvals identify
the unrestricted mode. File-tool operations retain their existing restrictions.

[`Capabilities.svelte`](../../web/src/Capabilities.svelte) polls the backend while a chat is
selected, displays the deadline, and shows a renewal notice outside the closed Tools popup.
Review opens the controls; only explicitly enabling the switch grants another duration.
Dismiss clears the expired record. Browser timers do not enforce access.

This grants the macOS user's file, credential and network access. Expiry stops MiniClaw's
managed process group; it cannot undo changes or revoke work handed to another service or
a process deliberately detached into another group. It is not containment for untrusted code.

Verification: host-tool integration tests cover expiration during execution, renewal, manual
revocation, permission removal, restart, duration validation and late approval. Frontend tests
exercise the expiry notice and duration sent by explicit renewal.
