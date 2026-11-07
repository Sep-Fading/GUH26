# Coding session: review every change before it is applied

A coding session is a switch on a conversation. While it is on, the agent cannot change a
file until the user has seen the change as a diff and applied it, and every shell command
asks first.

## Switching it on

The composer's Tools dropdown has a "Coding session" switch
([capabilities](../capabilities/README.md)). It restricts rather than grants, so it needs no
agent's permission. (A strip above the composer, and later a button of its own, were removed at
the user's request on 2026-09-21.) The switch is the feature rule `coding_session`: a
conversation rule switches it for that conversation, and a project's rule (in the project's
permissions) is the default for the project's conversations. The conversation can override
the project either way. The snapshot reports the result as `codingSession`.

## What is gated

| Call | In a coding session |
|---|---|
| `write_file`, `replace_in_file` | Held for review. Nothing is written before **Apply**. |
| `run_shell` | Asks the user every time, even where a saved rule allows it, because a command can change files too. |
| reads, listings, searches | Unchanged. |

The review happens in [`review_change`](../../server/src/chat.rs), after the ordinary
permission checks and before the agent gate is taken, so a review that waits half an hour
blocks nothing else. It reads the file's current content through the same executor and grants
the change would run under, builds the proposed content, and keeps both away from the disk.
An edit whose passage does not occur exactly once is not reviewed: it would change nothing,
and the executor tells the agent why.

A `write_file` to a path that names nothing yet is reviewed as a **new file** (`kind:
"create"`), against nothing; the file does not exist until Apply. A file that exists but cannot
be read as text cannot be shown, so a coding session refuses to replace it. Found with the
real VM on 2026-09-21: before that, `write_file` could not create a file at all, and the
model reported the workspace as read-only because Codex's own sandbox is. The tool's
instructions now say that the sandbox is read-only on purpose and that the tool is how it
writes.

## The review

[`ReviewDialog.svelte`](../../web/src/ReviewDialog.svelte) shows the file and a scrollable
diff ([`DiffView.svelte`](../../web/src/DiffView.svelte)): GitHub's diff colours, old and new
line numbers, syntax colours, and the exact characters that changed inside a rewritten line
([`diff.ts`](../../web/src/diff.ts) marks them when a removed and an added line share at
least 40% of their text). "Side by side" puts old and new in two columns; the choice is
remembered in the browser.

| Decision | Effect |
|---|---|
| Apply change (or Cmd/Ctrl + Return) | The proposal is written. |
| Apply all in this turn | This change and the rest of the current turn are written without asking. The next turn asks again. |
| Edit before applying, then Apply my version | The user's text is written instead. The agent is told: "The user changed your proposed content before applying it. Read the file again before you edit it further." |
| Reject, with an optional note | Nothing is written. The note goes to the agent as the reason. |
| No decision in 30 minutes | Rejected; the agent is told nobody reviewed it; notification `review_expired`. |
| Stop, or the turn ends | The review closes and nothing is written. |

One change is reviewed at a time per conversation.
`POST /api/threads/{id}/reviews/{review}` takes `{ action: apply | apply_all | reject, text?, note? }`.
The activity list records what was really written, including the user's own version.

## Tests

`tests/host-tools.test.mjs` (5 cases: nothing written before Apply, reject with note, edited
apply, apply all for the turn only, commands always ask, the snapshot flag),
`web/tests/diff.test.ts` (6), `web/tests/chat.test.ts`, and `tests/browser/chat.spec.ts`
("the coding session review..."), which drives a real tool call and checks the file on disk.

Observed with real models on 2026-09-21 (`tests/experiments/real-e2e.test.mjs`): Codex and
Claude each proposed "blue", were rejected with the note "Use green instead of blue", proposed
"green", and the file changed only after Apply.
