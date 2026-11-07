# Activity: what the tool did in each turn

Every conversation shows what MiniClaw's own tool did, live while a turn runs and afterwards.
Provider shell and file tools are off, so this tool is everything an agent can do to files
and commands, and the list is complete for them.

## What the user sees

Above a turn's first reply sits a collapsible "Activity" list
([`ActivityList.svelte`](../../web/src/ActivityList.svelte)). Each row is one tool call:

| Kind | Row | Expanded |
|---|---|---|
| `command` | the command, its time, `exit N` when not 0 | working folder, output, refused connections |
| `edit`, `write` | the file, `+added -removed` | full path and a coloured unified diff |
| `search` | the text searched for | the folder and the matches |
| `list` | the folder | its entries |
| `read` | the file | full path only; file contents are not repeated |
| `question` | the questions the model asked | the user's answers, or why there were none ([questions](../questions/README.md)) |
| `mcp` | `server: tool`, or "List MCP tools" | the arguments that were sent and the text that came back; a listing keeps no output ([MCP servers](../mcp-servers/README.md)) |

A row is `Running` until it finishes. A call the user rejected, or that a rule blocked, shows
`Refused`. An edit whose passage was not found shows `No change` with the reason. A turn with
no reply yet shows its list at the end of the conversation.

## How it is recorded

[`Backend::scoped_tool`](../../server/src/chat.rs) wraps every call: it records a `running`
entry before consent is asked, and the finished entry after the reply went to the provider.
[`activity.rs`](../../server/src/activity.rs) shapes both. Output is cut at 16 KiB per entry.

Writes and edits get their diff from the executor
([`executor.rs`](../../server/src/executor.rs) `diff`), which reads the old content before it
writes and uses the `similar` crate (2.7, Apache-2.0) for a unified diff with three context
lines, cut at 64 KiB. In a VM the diff is made by that VM's worker, so an older worker reports
none until `miniclaw update-vm`.

Tool replies contain `status` and `text`, without the Activity diff. A test asserts the
diff is absent. The parent model can request bounded delegated-activity metadata through
[delegation status](../delegation/README.md#asking-the-parent-for-progress): operation names,
states and timestamps, with no raw outputs or diffs.

Entries reach the page as `activity` events (same id, first `running`, then finished) and in
the conversation snapshot as `activities`. `Backend::publish_activity` ends with `changed()`,
which wakes the open event streams; without it an entry waited for the next unrelated event.
A question from the model is an entry of kind `question`: the page words it "Asked", times it
from when it was asked to when it was answered, and shows "Not answered" when it was refused. Because providers do not return tool calls with
their history, the Mac keeps the log itself: `<data>/activity/<thread>.json`, mode 0600, newest
300 entries, deleted with the conversation. Messages now carry their `turnId`, which is how
the page places a list above the right reply.

## Tests

- `server/tests/executor.rs`: diffs for edit and write, empty diff, no diff on a refused edit,
  the size limit.
- `tests/host-tools.test.mjs`: live events, snapshot, a refused read without content, the
  model's reply without the diff, survival across a restart.
- `web/tests/chat.test.ts`: live update in place, expansion, diff colours, position above the reply.
- `tests/browser/chat.spec.ts`: "a turn shows what the tool did".

Not covered: Computer Use actions and Claude's web tools are not in this list.
