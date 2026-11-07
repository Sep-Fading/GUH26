# Slash commands and /mcp-create

A message that starts with `/name` attaches one skill's instructions to that turn only. The
typed text is sent unchanged. A command adds instructions and nothing else: no tool, no
permission and no server comes with it.

Sequence: [sequence.md](sequence.md).

## Which commands exist

`GET /api/slash-commands` ([`slash.rs`](../../server/src/slash.rs) `commands`) lists the
built-in skills first, then the user's skills that are active and marked "Callable with a
slash" (`slash` on a [skill](../skills/README.md)). A skill's command name is its name in
lowercase with every run of other characters as one `-`, 40 characters at most. A name that is
already taken is not listed twice, so a built-in cannot be shadowed.

```rust
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); }
        else if !out.ends_with('-') && !out.is_empty() { out.push('-'); }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}
```
[`slash.rs`](../../server/src/slash.rs)

## What a command does to a turn

In `send` ([`api.rs`](../../server/src/api.rs)), a text that starts with `/` is given to
`slash::section`. Only a leading `/name` followed by a space or the end counts, so
`/Users/me/file` and `/release-notes/old` are plain text. A match sends a section `## Skill called with /name: Title` under its own context key,
`miniclaw_slash`, beside the conversation's skills in `miniclaw_skills`. It has its own key
because the real Codex adds a changed key again in full
([measured](../skill-groups/README.md#what-repeating-the-text-costs)); for Claude the section
goes in front of the message the CLI reads, not into the system prompt. The next turn is an ordinary one unless the command is typed again.
An unknown command is sent as plain text.

## The menu

[`SlashMenu.svelte`](../../web/src/SlashMenu.svelte) opens above the composer while the draft
is a slash and a partial name with nothing after it. Arrow keys move, Enter or Tab fills in
`/name ` without sending, Escape closes it and keeps the text. The list is fetched on first
use. The message box names the list and its active option (`aria-controls`,
`aria-activedescendant`) only while the list exists.

## /mcp-create

The one built-in skill (`MCP_CREATE` in [`slash.rs`](../../server/src/slash.rs)). Its
instructions tell the model to:

1. find out what the server should do and propose a name;
2. ask which language, TypeScript or Python, every time, and wait;
3. build it in a new folder under the working folder with the official SDK and the stdio
   transport, never writing to stdout except protocol messages;
4. test it with `initialize`, `tools/list` and one `tools/call` over stdin, and say what was
   and was not tested;
5. hand it over in a fenced `miniclaw-mcp-server` block, with names of secrets only.

The package names in the skill (`@modelcontextprotocol/server` with `zod`; `mcp[cli]` with
`MCPServer`) were read from the official quickstart on 2026-09-21. They have changed before, so
the skill also tells the model to read that page when an import or install fails.

The skill works through the conversation's ordinary tool. Installing packages needs the
`network` switch and the registry hosts approved, which the skill tells the model to say.

### The hand-over card

[`message-markdown.ts`](../../web/src/message-markdown.ts) `serverProposal` reads the block
with the same rules the backend applies to a server: name, absolute `command`, `args`,
`folder`, `hosts`, `env` names. Anything else in the block is ignored, which includes
`unrestricted`. A block that fails a check stays an ordinary code block.

A valid block renders as a card with "Add this server to MiniClaw". The button opens Settings,
MCP servers, with a new draft filled in ([`McpServers.svelte`](../../web/src/McpServers.svelte)).
Nothing is saved until the user presses Save. A proposed secret name the user has not given a
value is saved in `needs`, and the row then says "Enter NAME". The saved server is sandboxed and
switched on nowhere ([MCP servers](../mcp-servers/README.md)).

No tool adds a server, so a model cannot add one. Because the card is message text, it is
still there after a reload.

## /skill-create and /agent-create

Two more built-in skills, with the same rule as `/mcp-create`: the model drafts and hands over,
and only the user saves. Each asks what it needs through `miniclaw_ask`, checks what exists, and
ends its reply with one fenced block that the page shows as a card.

What the model is told about the user's setup ([`slash.rs`](../../server/src/slash.rs)
`inventory`) travels in the same one-turn `miniclaw_slash` section:

| Told | When |
|---|---|
| Every skill's name and description | always |
| Every agent's name, and the model ids with their efforts | `/agent-create` only |
| The current instructions of a skill or agent | only when the user's message contains its name, in any letter case; three at most |

Nothing else is handed out, so a model cannot read a skill the user did not name.

The blocks ([`message-markdown.ts`](../../web/src/message-markdown.ts) `skillProposal`,
`agentProposal`):

```text
miniclaw-skill  {name, description, instructions, slash, replaces}
miniclaw-agent  {name, instructions, model, effort, skills: [names], permissions: [...], replaces}
```

Only these fields are read; `active`, ids and delegation never travel. A name is one line of at
most 80 characters and instructions at most 16000. A block that fails any check stays an ordinary
code block, and one bad permission spoils the whole block, so nothing is dropped silently.

"Review this skill" and "Review this agent" open Settings with the form filled in
([`Skills.svelte`](../../web/src/Skills.svelte), [`Agents.svelte`](../../web/src/Agents.svelte)).
A note at the top says where the draft came from. With `replaces` naming an existing item, that
item opens with the new text and keeps its name, revision and active state; otherwise the draft is
new and inactive. A model the catalog does not have leaves the model unchanged, and a skill name
that matches no active skill is left unticked; the note says both.

### Suggested permissions

A proposed agent may carry up to 12 suggestions, each with a reason:

| Kind | Fields |
|---|---|
| `feature` | `name`: `network`, `web_fetch`, `subagents` or `task_tracking` |
| `folder`, `file` | absolute `path` without `..`, `action`: `read` or `write` |
| `host`, `domain` | a DNS name |
| `mcp_server` | `name` of a saved server, found by that name when approved |
| `application` | `id`, `action`: `observe` or `control` |

Commands, plan mode and the coding session cannot be suggested. None is granted by saving. The
form lists them under "Suggested by the agent that wrote this"; the buttons are disabled until
the agent is saved, and then each "Allow" saves exactly one rule through the ordinary
`POST /api/permissions/agent/{id}`, while "No" only marks it refused. The rules appear in the
agent's permission settings below, where they can be removed.

## Tests

[`slash-commands.test.mjs`](../../tests/slash-commands.test.mjs),
[`slash-commands.spec.ts`](../../tests/browser/slash-commands.spec.ts), and the card's checks in
[`message-markdown.test.ts`](../../web/tests/message-markdown.test.ts).

Not observed: a real model following `/mcp-create` from start to finish.
