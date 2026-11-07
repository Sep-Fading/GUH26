# Skills and instruction editing

MiniClaw skills are reusable instruction documents attached to agent profiles.
They do not grant tools, application access, or delegation. Codex files are import
sources, not live dependencies. See the [sequence](sequence.md).

## Management

[api.rs](../../server/src/api.rs) provides authenticated, same-origin routes using
the existing JSON body limit and `X-MiniClaw-Request: 1` mutation header.

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/skills` | None | `skills`: owned copies, including inactive ones. |
| `POST /api/skills` | Definition without revision | HTTP 201 with `skill`, revision 1. |
| `POST /api/skills/{id}` | Complete definition and current revision | Updated `skill`; stale revision returns 409. |
| `GET /api/skills/sources` | None | Importable folder IDs and names. No copies are created. |
| `POST /api/skills/import` | `source`: one listed folder ID | HTTP 201 with a new inactive copy. |

[skills::Definition](../../server/src/skills.rs) requires `name` and `instructions`.
`description` defaults to empty, `active` to false, and `slash` to false; `slash` makes an active
skill callable for one message as `/its-name` ([slash commands](../slash-commands/README.md)). Limits are 80 bytes for names,
500 for descriptions, 16384 for instructions, and 128 saved copies. Unknown fields
are rejected. Edits increment the revision; there is no automatic source sync.

[Skills.svelte](../../web/src/Skills.svelte) creates, edits, activates, and imports
copies. [Skills state](../../web/src/skill-state.svelte.ts) preserves drafts on errors,
ignores responses after account changes, and prevents stale lists from replacing edits.

## Import boundary

[skills::sources](../../server/src/skills.rs) scans the configured `CODEX_HOME/skills`,
defaulting to `$HOME/.codex/skills`. It examines at most 1024 entries. Candidates use
visible folder names of at most 80 ASCII letters, digits, dots, underscores, or hyphens.
Hidden/system directories, nested skills, plugin caches, and other roots are excluded.

Import opens each segment relative to its parent with `NOFOLLOW`, rejecting symlinks.
`SKILL.md` must be a regular UTF-8 file within the instruction limit. No commands run.
The entire Markdown source, including frontmatter, is copied unchanged. Metadata is
not parsed: the folder name becomes the copy's name, and description starts empty.
The user can edit these fields after import.

Scripts, references, and other supporting files are not copied or made available to
the agent. A skill requiring those resources is not fully usable through this importer.
Original files are never written. Reading the import list does not activate skills.

## Agent context

[agents::Definition](../../server/src/agents.rs) accepts up to eight distinct `skillIds`.
All must exist; an active profile requires active skills. `Store::prepare` resolves
current copies before new chat creation. `Agent::configure` appends each selected
skill in profile order; this exact fragment supplies its name and source:

```rust
            instructions.push_str(&format!(
                "\n\n## Skill: {}\n\n{}",
                skill.definition.name, skill.definition.instructions
            ));
```

The existing private `agents.sqlite3` gains `skills` for current definitions and
`agent_chat_skills` for immutable snapshots. Profile and skill snapshots commit in
one transaction. Older chats without skills continue to load without a snapshot row.
Missing snapshots for a profile that references skills fail closed.

Edits apply to new chats. Resume uses pinned text, not current skill content.
Deactivation blocks subsequent sends and new chats using that skill and requests
cancellation of affected cached active turns. History remains readable. Reactivation
permits old chats to continue with their original text. Cancellation cannot undo work.
Agent and skill mutations share the admission lock; instructions still reach Codex's
model service. The [agent access restrictions](../agents/README.md) remain unchanged.

## Project context

Skills can also contribute through independently activated [skill groups](../skill-groups/README.md).
Groups use current instructions on the next message; individually attached copies
retain the pinned behavior described above.

[Projects](../projects/README.md) can also attach skills to their default MiniClaw
conversations. Named agents keep their independent skill selection. Owned copies
support [confirmed deletion](../deletion/README.md), preserving installed sources
and snapshots already stored with conversations.

## Markdown editor

[Instructions.svelte](../../web/src/Instructions.svelte) mounts one CodeMirror editor
for either manager. The editor module loads when the field mounts and releases its
listeners on unmount. Loading failure displays an error without changing the draft.

[instruction-editor.ts](../../web/src/instruction-editor.ts) parses Markdown and adds
display decorations. Emphasis, strong emphasis, headings, and code render in place;
their markers stay in the document but are hidden on screen. HTML, image references,
and URLs remain literal text, without navigation or resource loading. CodeMirror's
URL-free caret buffers are not source images. Input over 16384 characters is rejected;
the server separately enforces its 16384-byte limit.

`spacedItalic` converts a typed `* test *` shortcut into `*test*` outside literal code.
Other Markdown edits keep their source, including frontmatter and code. Consistent
CRLF line endings are retained. Undo and cursor movement use CodeMirror commands;
pending saves make the editor read-only without clearing history. Loading a different
source resets editor history. Chat message rendering is unchanged.

## Verification

- [API tests](../../tests/skills.test.mjs): revisions, import boundaries, source preservation,
  authentication, attachments, restart snapshots, deactivation, and cancellation.
- [Browser tests](../../tests/browser/skills.spec.ts): source round-tripping, rendering safety,
  import selection, saved copies, attachments, and responsive layout.
- [Editor tests](../../tests/browser/instructions.spec.ts): live typing, undo/redo,
  hostile text, exact source retention, and pending-save behavior.
- [State tests](../../web/tests/skills.test.ts): stale replies, account reset, CRLF, and input limits.
- [Runtime probe](../../tests/agent-runtime.test.mjs): pinned text reaches installed Codex
  before and after restart without changing the tested tool inventory; fake inference only.

## Where Active applies

`active` governs direct attachment: to an agent, a project, a conversation, the main agent, or
a slash command. A conversation with an inactive skill attached directly refuses its next
message with `skill_inactive`; the main agent instead leaves an inactive skill out. A
[skill group](../skill-groups/README.md) has its own Active box and one Enabled box per member,
and an active group runs its enabled members whatever their own `active` says. The user decided
on 2026-09-21 to keep these independent.

