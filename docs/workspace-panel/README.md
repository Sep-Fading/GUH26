# The workspace panel

The right-hand side of a conversation is a panel with tabs
([`WorkspacePanel.svelte`](../../web/src/WorkspacePanel.svelte)).

| Tab | Shows |
|---|---|
| VM | The live desktop, only for a conversation that runs in a VM |
| Files | The working folder as a tree, with a read-only viewer (line numbers, syntax colours) |
| Changes | The git working tree against the last commit: each file with `+added −removed`, its diff on opening, and Discard |
| Activity | Everything the agent's tool did in this conversation |

On a wide window it is a side panel. Drag its left edge, or focus the edge and press the arrow
keys, to resize it between 320 and 900 px. "Collapse" folds it to a rail of icons; a dot on the
Changes icon means there are changes. Width, tab and fold are remembered in the browser. A VM
conversation opens on its screen; any other starts folded. On a narrow window the VM stays
above the chat as before, and a "Workspace" button opens the other tabs as a sheet. Only one
layout is mounted at a time, so the VM's stream is opened once.

## Where the files come from

The working folder is the project's workspace folder for a conversation on this Mac, and
`/Users/Shared/MiniClaw/Workspace` for a conversation in a VM. A Mac conversation without a
project workspace has no folder; Files and Changes say so.

## Git runs in the sandbox

[`workspace.rs`](../../server/src/api/workspace.rs) sends every listing, read and git command
through the same executor as an agent's own tool calls, with a grant for the working folder
only. This is a security decision, not a convenience: a repository's configuration can make
git run programs (`core.fsmonitor`, `diff.external`, filters, hooks), and the agent can write
to that folder. Outside the sandbox such a program would run as the backend and could read
anything the user can, including provider sign-ins. Inside it, the program reaches the folder
and nothing else. Git is also run with no user or system configuration and with
`core.fsmonitor`, `core.hooksPath`, `diff.external`, the pager and text conversion switched
off. A test plants both hooks and checks that nothing outside the folder was touched.

The same design makes VM conversations work with no new worker code: the requests go to the
VM's existing `miniclaw/scoped/execute`. That path is **not observed with a real VM yet**.

Paths from the page are relative, may not contain `..`, and may not name `.git`.

## Changes and Discard

With a repository, Changes lists `git status` with counts from `git diff HEAD --numstat`;
a new file counts its lines as added. Without one it falls back to the diffs in MiniClaw's
[activity log](../activity/README.md) and says so. The list reloads when a tool call finishes and when the window regains focus; it has no reload button or header of its own, except one line saying so when the folder is no git repository.

Discard asks first. A tracked file returns to the last commit (`git restore`). A new file is
moved where it can be recovered: the Trash on this Mac, `Transfers/discarded` in a VM. There
is no staging, commit or push.

## Tests

`tests/workspace.test.mjs` (5, on a real repository, including the planted-configuration
attempt) and `tests/browser/workspace-panel.spec.ts` (rail, Files, Changes, diff, Discard,
resize by keyboard, remembered state, the narrow sheet).
