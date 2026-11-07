# Projects that remember

Every conversation of a project starts with three things an ordinary conversation lacks: the
project's standing instructions, an index of the notes the project's conversations share, and a
digest of the project's latest conversations. One tool, `miniclaw_project`, reads and writes the
rest on demand. Nothing here calls a model in the background, and nothing leaves the project.

Sequence: [sequence.md](sequence.md).

## What Claude Code and Codex do, and what MiniClaw does

Read from their documentation on 2026-09-21. Claude Code starts each session fresh and carries
over instruction files (`CLAUDE.md`, or `AGENTS.md`) and notes the model writes, of which an
index of up to 200 lines or 25 KB is loaded. Codex reads `AGENTS.md` from the repository root
down, 32 KiB at most, and its guide describes no memory between sessions. Neither carries
transcripts over. The Claude CLI's own file loading is off inside MiniClaw
(`--setting-sources ""`), so MiniClaw reads the files itself, for both providers.

## Standing instructions

`brief()` in [`api.rs`](../../server/src/api.rs) adds a `## Project instructions` section to the
end of `baseInstructions` when a conversation starts and when it is loaded again
([`briefing.rs`](../../server/src/briefing.rs) `Rules`). It holds:

- `AGENTS.md`, then `CLAUDE.md`, from the top of the project's folder (`rule_files`). A file is
  read only when its real path lies inside the folder and outside the folders MiniClaw protects,
  so a link cannot bring in text from elsewhere. A `CLAUDE.md` that repeats `AGENTS.md` or only
  imports it (`@AGENTS.md`) is left out. Together they are cut at 32 KiB, and the text says so.
  Host projects only; files in subfolders or above the folder are not read.
- The project's own `instructions` (16000 characters), set in its settings. These reach projects
  without a folder and VM projects too.

`GET /api/projects/{id}/rules` names the files found and their sizes for the settings page.

## Memory

[`project_memory.rs`](../../server/src/project_memory.rs) keeps notes in `project_memory`, in the
metadata database, deleted with the project.

| Limit | Value |
|---|---|
| Notes per project | 64 |
| Name | one line, 80 characters; found in any letter case; the first spelling is kept |
| Description | one line, 200 characters |
| Text | 8 KB |
| Index told to a conversation | 4 KB; further notes are counted, not listed |

A save is refused when it looks like a secret (`looks_secret`): a private key header, a token of
a known shape (`sk-`, `ghp_`, `github_pat_`, `xoxb-`, `glpat-`, `AKIA…`), or
`password|secret|token|api key` followed by `:` or `=` and a value of 12 characters or more. The
same check guards the settings routes and summaries.

The user reads and changes notes under "Memory" in project settings
([`ProjectMemory.svelte`](../../web/src/ProjectMemory.svelte)):
`GET/POST /api/projects/{id}/memory`, `POST …/memory/{name}` (may rename), `POST …/memory/{name}/delete`.

## The tool

`miniclaw_project` ([`project_tool.rs`](../../server/src/project_tool.rs)) is one tool with an
`action`, so a conversation carries one schema (2341 bytes; a unit test holds it under 3200).

| Action | Does | Bound |
|---|---|---|
| `memory_read {name}` | returns one note | 8 KB |
| `memory_save {name, description, text}` | creates or replaces by name | the limits above |
| `memory_delete {name}` | removes one note | |
| `summary_save {text}` | this conversation's summary | 600 characters |
| `conversations_list {query?}` | other conversations: date, name, id, summary | 50 |
| `conversations_search {query}` | messages holding every word, any order and case | 10 passages of 300 characters |
| `conversation_read {id, page?}` | one page of a conversation of the same project | 8000 bytes a page |

It is offered to a project's conversations on this Mac, with Codex and with Claude, and not to a
delegated run. A VM project conversation gets the tool when its worker carries it, and the Mac
answers every call, because the notes and the copies live on the Mac. A VM whose worker was
installed before that is told the index and the digest as text and has no tool. See [worker transport](../worker-transport/README.md#tools-the-mac-answers). VM
conversations are Codex only. Every call is an Activity entry of kind `memory`
("Remembered", "Recalled", "Forgot", "Searched conversations for", "Read conversation"). The
switch "Project memory" in Tools (`no_project_memory`, on by default, listed only in a project)
stops both the text and the tool for one conversation.

A new tool name is registered in seven places: its module and `lib.rs`; `offer_project` at create
and resume in `api.rs`; the dispatch in `chat.rs`; the name filter in `rpc.rs`; `claude.rs`
(`offered_tool`, which accepts exactly MiniClaw's five names, `Thread.project`, the shim
condition, `MINICLAW_PROJECT_TOOL`, `--allowedTools`); `tool_bridge.rs`; and `briefing.rs`.

## What travels, and when

The design follows what was measured with the real Codex
(`tests/experiments/skill-context-cost.test.mjs`): an unchanged `additionalContext` key is kept
as one copy, an omitted key leaves the copy in place, and a changed key is added again in full.
For Claude a changed system prompt invalidates the cached conversation, so one-turn text goes
with the message.

`project_tool::note` therefore sends the key `miniclaw_project`:

- with the conversation's first message: the memory index and the digest;
- again only when the index differs from what the conversation last saw, and then the index alone;
- never because of the conversation's own writes (`advance`), and never because only a note's
  text changed: the index is ordered by name and holds names and descriptions only.

What each conversation last saw is a hash in `Backend::project_seen`, in memory. After a restart
the index is sent once more, which costs Codex nothing when it is unchanged.

## Earlier conversations

[`project_history.rs`](../../server/src/project_history.rs). The digest names the 8 latest other
conversations that have a finished response: date, name (the user's own name wins), id, and the
summary if one was saved. Summaries are written by the working agent through `summary_save`.

The look-up reads the copies MiniClaw keeps of a conversation's messages
([`transcripts.rs`](../../server/src/transcripts.rs), the newest 400). `Backend::keep_transcript`
kept them for VM conversations only; it now keeps them for project conversations as well. No
provider is called, and it works with a VM off. Attachments and tool steps are not in the copy.
A conversation of another project, or of none, cannot be read, listed or found.

## Checked by

`tests/briefing.test.mjs`, `tests/project-memory.test.mjs` and `tests/project-history.test.mjs`
with the fake Codex and the fake Claude CLI; unit tests for the secret patterns, the index size
and the schema size; `tests/browser/projects.spec.ts` and `capabilities.spec.ts`. Observed with
real models (`tests/experiments/real-project-memory.test.mjs`): `gpt-5.6-luna` and Claude
Haiku 4.5 each saved a note and a summary when asked, answered from them in a new conversation,
found a codename that existed only in the earlier conversation, and followed the project's
instructions.
