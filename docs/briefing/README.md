# What a model is told about where it is

Until 2026-09-21 a plain conversation was told "No tools are available" and a tool conversation
"Use only miniclaw_scoped_native", whatever it had really been given, and nobody was told the
date, the machine, the working folder or what was switched on. A model in that position asks
the user things it could have known. MiniClaw now writes a briefing from its own state
([`briefing.rs`](../../server/src/briefing.rs)). Nothing a model says can change it.

Sequence: [sequence.md](sequence.md).

## Two parts, because providers treat them differently

| Part | Where it goes | When it is written |
|---|---|---|
| Standing | `baseInstructions`; for Claude the system prompt | when a conversation starts, and again when it is loaded (`thread/resume`) |
| Current state | `additionalContext.miniclaw_environment`; for Claude the system prompt | with every message |

The state holds the date and never the clock time, so its text changes only when the state
does. The real Codex keeps an unchanged context key as one copy and adds a changed one again
([measured](../skill-groups/README.md#what-repeating-the-text-costs)), so sending it with every
message costs nothing until something changes.

## The standing part

[`api.rs`](../../server/src/api.rs) `brief` runs last, after every tool was offered, so the
text describes exactly the tools in `dynamicTools`. A tool the conversation was not given is
not described.

- **What MiniClaw is**, and that nothing the model writes changes a setting, permission or tool.
- **The machine:** on this Mac, the macOS version and architecture (`sw_vers`, `uname -m`) and
  MiniClaw's version. In a VM, that it is a separate computer and its tools act there.
- **The toolchain on this Mac:** the versions of `git`, `node`, `npm`, `python3`, `uv`, `cargo`,
  `go` and `swift` that answer `--version`. `detect` runs the checks once per backend start,
  side by side, each with a cleared environment and a 4 s limit; a conversation that starts
  before they are done waits for them. Only the name and the version number are kept. A VM
  conversation is not told the Mac's toolchain.
- **The working folder**, for a conversation with the file tool: a project's workspace on this
  Mac, or the VM's workspace. For the Mac, `survey` reads its top level (40 names at most,
  hidden entries left out, folders with a trailing slash) and its git branch with the number of
  uncommitted files. It goes through the sandboxed executor with a read grant for that folder
  only, and git runs with hooks, the fsmonitor and user configuration switched off, as in the
  [workspace panel](../workspace-panel/README.md). This is a picture from when the conversation
  was loaded, and the text says so.
- **What it can use:** one paragraph per tool it has, or, without the file tool, that it cannot
  read files or run anything and how the user gets that. A Computer Use conversation is told so.
- **How to work here:** look before you ask; never ask for permission in a message, because
  MiniClaw raises the popup itself; what "not allowed" means and where the user changes it;
  never ask the user to run a command the model can run; never ask for a secret in the chat.

```rust
    text.push_str("\n## How to work here\n\
- Look before you ask. If a file, a folder listing, a command, a search, list_tools or the environment note can answer a question, find the answer yourself. Ask the user only for what only they can know: a preference, a choice between real alternatives, a missing requirement, or consent to something risky.\n\
```
[`briefing.rs`](../../server/src/briefing.rs) `standing`

## The current state

[`api.rs`](../../server/src/api.rs) `environment_note` reads, for each message: the date; the
machine; the agent, or that the conversation belongs to the main agent; the project; each
[capability](../capabilities/README.md) as on, off, or not allowed for this conversation's
agent; the MCP servers that are switched on, by name; and the agents it may delegate to.

## The question tool

[`miniclaw_ask`](../questions/README.md) says the same in its own description: look first, and
it lists what is never a question (which files exist, what is installed, the branch, the date,
which tools and servers exist, what is switched on). The briefing tells the model to ask
through the popup rather than in its message.

## Limits

- A Computer Use conversation keeps the MCP gateway and the question tool; it never has the file
  and command tool, and its briefing says so.
- A VM conversation gets no folder listing, git state or toolchain; it is told its workspace
  path and to look with its tools.
- Web search is not available in a VM conversation. The VM's worker accepts only the bundled
  policy's configuration values, so `web_search: live` made it reject every new VM
  conversation on 2026-09-21 until search was limited to Mac conversations. The note and the
  Tools dropdown say "not allowed" there.
- The standing part is as old as the conversation's last load. Files added later are not in it.

## Tests

[`briefing.test.mjs`](../../tests/briefing.test.mjs) (4) with a real folder and a real git
repository, and unit tests in `briefing.rs`.

Observed with a real model on 2026-09-21
([`real-questions.test.mjs`](../../tests/experiments/real-questions.test.mjs), opt-in,
`gpt-5.6-luna` at low effort): asked for the date, the kind of computer and whether it could
read files, it answered all three without asking ("Monday, 21 September 2026", "Apple Mac,
arm64", "No, I can't read files here"). Asked to find out the user's language preference, it
opened the popup with three options, the recommended one first, took an answer in the user's
own words, and wrote the script. Not observed: Claude, and whether models ask less across
ordinary work.
