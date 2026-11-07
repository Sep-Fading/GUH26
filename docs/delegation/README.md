# Delegation between agents

An agent may hand one task to another agent that the user allowed. The user decides at every
request whether to wait for the result, let it run in the background, or decline. Nothing
starts before that answer. See the [progress and opening sequence](sequence.md).

## Setting it up

In Settings, Agents, "May delegate to" lists every other saved agent
([`Agents.svelte`](../../web/src/Agents.svelte)). Ticking one adds its id to `delegatesTo`.
Two limits are set per agent in `delegation`
([`DelegationLimits.svelte`](../../web/src/DelegationLimits.svelte)):

| Field | Meaning | Range, default |
|---|---|---|
| `maxPerTurn` | requests one user message may lead to, declined ones included | 1 to 10, 3 |
| `minutes` | how long one run may take | 1 to 1440, 10 |

The time limit is typed in minutes; from 60 the form also shows hours ("2 h 30 min"). The store
accepts only saved agents as recipients and never the agent itself
([`agents.rs`](../../server/src/agents.rs) `save`). A model cannot add a recipient.

A conversation with no agent belongs to the main agent, which has no list: it may hand a task
to every active agent. The user chose this on 2026-09-22 after a plain conversation could not
reach their Blender agent; every hand-off is still asked about, and an inactive agent is never
named. `recipients` takes the conversation's agent or none, and the environment note names the
recipients only where the tool is offered (this Mac, no Computer Use, not a delegated run).

The main agent has the same two limits, under Settings, Agents, Main agent, Delegation
(requirement 2026-10-11). They are kept in the `delegation` column of the `main_agent` row
([`main_agent.rs`](../../server/src/main_agent.rs)) and sent as `delegation` on
`POST /api/main-agent`; a request that leaves it out keeps what is saved. Nothing saved, as on an
install from before, reads as 3 per message and 10 minutes. A named agent's limits are pinned with
its revision when a conversation starts; the main agent's are read at each request, so a change
applies to old conversations from their next hand-off, as its skills do.

### Why a day, and why a waited run gets an hour

There is no "unlimited". A run that hangs, or waits for an approval nobody gives, must still end
and free its machine, so the limit stops at 1440 minutes (`Delegation::MOST_MINUTES`). The user
asked on 2026-10-11 not to be locked into the old 60.

**Run in background** gets the full limit: the parent's turn has already been answered and nothing
holds a provider call open. **Wait for it** keeps the parent's `miniclaw_delegate` call open until
the run ends, and MiniClaw's own bridges end any tool call after 3900 s (65 minutes): the Codex
App Server reader ([`rpc.rs`](../../server/src/rpc.rs)) and the Claude bridge
([`claude.rs`](../../server/src/claude.rs) `tool_call`; the CLI's MCP `timeout` and
`CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT` are set to the same 3900000 ms, and the shim's read waits
3960 s). So the parent waits at most `min(minutes, 60)` (`Delegation::wait_minutes`), five
minutes inside that window. A run whose limit is 60 minutes or less is simply waited for: it
stops at its limit and the call returns that it did not finish, as before.

A run with a longer limit (`Delegation::hands_over`) that is still going at the hour continues in
the background (lead decision, 2026-10-11; until then it was stopped there).
[`delegation.rs`](../../server/src/api/delegation.rs) `handle` runs it in a task of its own
with a `Handover` (the minutes to wait and a one-shot channel). At the hour, `attempt` calls
`Store::hand_over_run`, which turns the record's `mode` from `wait` to `background` and sets
`handed_over` (a column added to `delegated_runs`), publishes the run, and tells `handle`, which
answers the parent's call the way a background hand-off does: "Reviewer has not finished after
60 minutes, so it continues in the background. Its answer will appear in the conversation
later. Do not wait for it and do not repeat the request. To check on it, call miniclaw_delegate
with action=status and runId <id>." (A background hand-off says "is working on it in the
background" instead, with the same last three sentences.) From then on the run is a background
run in every way:

- it keeps going to its full limit and stops there ("It was stopped at its 240 minute limit.");
- the parent's turn carries on with that answer, and its end does not stop the run;
- stopping the parent no longer stops it either (open the run's conversation to stop it there);
  a parent the user stopped does not carry on by itself, and the result waits for the next
  message;
- when it ends, its result goes back as a background result: the idle parent carries on with it
  (see below), and `status` shows it with `mode: background`, `handedOver: true`;
- a restart closes it like any unfinished run, and the closing reason reaches the parent with
  the next message, as for a background run.

Its block and card read "Running, the conversation waits", then "Continues in the background
after the 1 h wait", then "Finished" or "Did not finish". The settings say so from 61 minutes, and
the prompt shows both times: "In the background it may take up to 2 h. If you wait, after 1 h it
continues in the background." Whether Codex itself keeps a tool call open that long is not
verified (see Limits).

A scheduled run is unchanged: it ends at its own one-hour limit
([`schedules.rs`](../../server/src/api/schedules.rs) `LIMIT`), which stops a waited run with it,
and nobody is there to answer a hand-off prompt, so a scheduled agent's request is declined after
120 s. A run past its limit is stopped and reported.

### What the model is told

So the model can plan for them (lead decision, 2026-10-11), the limits are in three places, all
worded by [`briefing.rs`](../../server/src/briefing.rs) `delegation_limits`:

- the `miniclaw_delegate` description: "Limits: at most 4 delegations per user message; each run
  may take up to 150 minutes; a run the user chose to wait for that is still going after 60
  minutes continues in the background." The last part appears only for a limit over 60;
- each message's environment note: "You may delegate to: Blender (at most 4 delegations ...)";
- the standing briefing, once: the user chooses wait or background, the 60-minute rule, and that
  the environment note has the current limits.

The limits are read as a hand-off reads them (`delegation::limits`): a named agent's pinned
ones, or the main agent's saved ones. The note is written for every message, so a change to the
main agent's limits reaches it at once. The tool description is written when the conversation
starts or is loaded again, like every tool's, so an open conversation's description keeps the
earlier numbers until then; the note beside it is current. The model does not choose wait or
background: the user does, in the prompt.

## One request, step by step

1. The agent's conversation is offered the tool `miniclaw_delegate` with the exact names of
   its active recipients ([`delegation.rs`](../../server/src/api/delegation.rs) `offer`). A
   conversation that is itself a delegated run is never offered it, so there is no nesting.
2. A call is refused, without asking the user, when the turn is no longer running, the name
   is not a permitted recipient, the task is empty or over 8000 characters, or the turn's
   limit is used up.
3. The run is recorded in `delegated_runs` under `(parent, callId)`. A repeated tool call
   finds the record and starts nothing.
4. The page shows "Delegate to <agent>?" with the source conversation, task and three buttons,
   even when another chat or the empty composer is selected. With no answer in 120 s the
   request counts as declined.
5. **Wait for it:** the recipient runs; the tool call returns "Answer from the agent <name>"
   with its final message, and the parent carries on. Stopping the parent stops the run unless the user has opened it as its own conversation.
   A run with a limit over 60 minutes that is still going at the hour continues in the
   background, and the call returns then (above).
6. **Run in background:** the tool call returns at once and tells the agent not to wait. The
   run carries on after the parent's turn ends.
7. The run stops at its time limit ("It was stopped at its N minute limit.") and is recorded as
   failed, shown as "Did not finish". The parent waits for at most 60 minutes of it (above).

## Pictures and files with the task

`miniclaw_delegate` takes `attachments`, at most 10 attachment ids of this conversation: a file
the user attached (the attachment note gives the model each id) or a picture an MCP tool
returned (its result names the kept picture's id). `handle` checks each before the user is
asked: an id the conversation does not hold, or an upload no message carries yet, refuses the
call and starts nothing. The prompt lists them (`attachments: [{id, name, kind, size}]`), the
popup shows them as "Files handed over", and the run records them (`delegated_runs.attachments`)
for its card. When the run starts, `attempt` copies each into the run's own conversation
(`attachments.rs` `copy_to`) and its first message carries the copies, so they travel as any
upload does: placed in the working folder, and pictures shown to the model, for Codex and
Claude alike. The parent's files are untouched.

`publish` and the forwarded approval now wake the parent's open pages (`backend.changed()`):
before, a prompt or a run update waited for the next unrelated change to reach the page.

## How a run executes

A run is a real conversation of the recipient agent, created and sent through the same
`create_chat` and `send` paths the page uses. It gets the recipient's pinned revision,
instructions, skills and permissions, and the parent's machine and project. Nothing of the
parent's access is lent to it.

The run's thread is initially hidden from the conversation list. "Open conversation with
<agent>" is available while it runs; "Continue with <agent>" is available afterwards.
Both set `detached` and open the same thread, without another provider creation or send.
Opening a running task makes it independent of the parent's Stop button, while keeping its
own time limit and delegated-tool restrictions until the original task ends. Its activity
continues to reach the parent during that task. A finished opened conversation can be used
normally, including starting its own delegations.

## What the user sees

[`DelegationBlock.svelte`](../../web/src/DelegationBlock.svelte) shows "Delegated to <agent>"
above the parent's reply, with the state, the time taken, the task, the delegated agent's own
[activity](../activity/README.md), and its answer under **its own name**. The answer is never
rendered as the parent speaking. "Show the full run" refreshes the child snapshot once per
second while expanded and running, including partial assistant messages and approval/file
review waits. Requests run sequentially; closing the block, changing its run or signing out
stops refreshes and discards late replies. A final refresh reads the completed transcript.

A delegated agent's tool permissions, computer approvals, file-change reviews and questions
appear in the parent conversation with the agent's name. Each child retains its own request;
answering one never clears another child's request or the parent's own approval. The parent's
own prompts are shown first, then child requests one at a time. The existing expiry and run
time limits still apply. No approval is automatic.

[`pending_requests`](../../server/src/api/delegation.rs) reads only live requests on active,
non-stopping child turns. Parent snapshots expose `delegatedRequests`; each run emits
`delegation.requests` containing its own queue. The existing `/api/threads/working` poll also
returns these requests so opening an idle parent restores them without opening the child.
The response carries source ownership beside the original request:

```rust
            requests.push(json!({ "runId": run.id, "parent": run.parent, "thread": thread,
                "agent": run.agent, "kind": kind, "request": request,
                "expiresAt": chrono::Utc::now().timestamp_millis() + seconds * 1000 }));
```

[`Chat`](../../web/src/chat.svelte.ts) selects requests for the open parent and routes answers
to the child thread's existing approval, review or question endpoint. Those routes keep all
expiry, active-turn and permission checks. New stream events take precedence over polls
started earlier. Answered IDs suppress stale responses. Ordinary parent snapshots that omit
the child queue do not clear it, and completing the parent's turn leaves child requests alone.

Questions can be answered with options or text in the parent. The "Answer in chat" shortcut
is hidden for a child question because it would require a message in the child's chat.
The parent draft is preserved. Tests cover simultaneous parent/child requests, child questions
and file reviews, navigation, stale polls, expiry and all four UI answer routes.

### The Agents tab

The workspace panel has an Agents tab ([`AgentCards.svelte`](../../web/src/AgentCards.svelte))
with one card per run of the conversation, running and waiting ones first, and a count of those
on the tab. A card shows the agent's name, the run's `summary`, its state in the same words as
its block ([`delegation-words.ts`](../../web/src/delegation-words.ts)), and a live line:

- asking: "Waiting for your answer"; running before anything happened: "Starting…";
  its state reads "Continues in the background after the 1 h wait" once a waited run is handed
  over;
- running: the newest step, worded as Activity words it ("Called roblox_studio: screen_capture",
  "Ran npm test", "Read /etc/hosts"), or the last line of the answer it is writing, whichever
  changed last;
- ended: the first line of its answer, or of why it did not finish.

The run loop in `delegation.rs` `attempt`, which already polls the run every 100 ms, compares
the run's newest Activity entry (`activity::Log::last`, which does not copy the log) and its
answer text with the last poll, clips the line to 160 characters, and when it changes publishes
`delegation.live` `{id, line}` to the parent's pages and keeps it in memory (`live()`), so a
snapshot's running runs carry `live` for a page that opens mid-run. The line is not stored: it is
how the run looks now. "Show in conversation" scrolls to the run's block (`data-run-id`).

`summary` is required when delegating: one line, at most 80 characters, no control characters.
It is checked after who-may-delegate, so a delegated agent that tries to delegate still hears
that it cannot. The prompt shows it above the task, and runs recorded before it existed show
their task's first line instead.

## Background results and the parent agent

A finished background run appears in its block at once, and the parent carries on with it by
itself (requirement 2026-09-23): `execute` runs the queue's `drain` for the parent when a
background run ends, and `drain`, finding nothing queued, calls `carry_on`. If the parent is
idle, `carry_on` sends MiniClaw's notice as the parent's next message, with the conversation's
model, effort and speed; if it is responding, nothing happens now, and the drain that runs when
that response ends does it. Results that end together go in one turn. The notice reads
`[MiniClaw notice] Blender finished "Build the tile set". The result is in this message's
MiniClaw context; carry on with the user's request.`; the page shows a message that begins with
`[MiniClaw notice] ` as a small line, not as the user's words, and leaves it out of prompt
navigation. No parent turn starts after the user stopped a response (`stopping` lasts until
their next message), while the queue is paused, or while the outcome of a send is unknown; the
result then waits for the user's next message as before.

The result itself travels as before: `take_results` hands each result out once, and it goes in
`additionalContext` under its own key, `miniclaw_delegations`, so no message is rewritten and
the skills block does not look changed
([why a separate key](../skill-groups/README.md#what-repeating-the-text-costs)). For Claude it
goes in front of the message the CLI reads. A `wait` run's answer is the tool's result, as ever.

## Asking the parent for progress

`miniclaw_delegate` accepts `action: "status"`, with an optional `runId`. Starting work
uses `action: "delegate"` or the existing `agent` and `task` fields without an action.
The tool description and [briefing](../../server/src/briefing.rs) tell the model to check
status when asked for progress. Codex uses the dynamic tool; Claude uses the existing MCP
bridge and the same handler. This follows the [MCP tool schema](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).

[`status`](../../server/src/api/delegation.rs) checks the caller's active turn, then reads
only that conversation's runs. It needs no approval, starts no task, spends no delegation
allowance and does not consume a pending background result. Even if no recipients remain
available, a parent with recorded runs retains the tool to check them.

[`Store::recent_runs`](../../server/src/delegations.rs) bounds the database read:

```rust
            let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE parent = ?1
                AND (?2 IS NULL OR id = ?2) ORDER BY started DESC, rowid DESC LIMIT 10"))
                .map_err(Issue::storage)?;
```

The response includes run ID, agent, task, state, start/end times, approval or file-review
wait, latest available assistant update, result and five recent activity summaries. Task
text is limited to 1,000 characters, latest update to 2,000, result to 6,000 and each activity
title to 256. Activity outputs, diffs and attachment contents are excluded. No completion
percentage is invented. A completed run reports its original result; later conversation
messages are not used as progress for that run. An account change discards the response.

Provider completion status determines success. Interrupted or failed turns with partial
text are recorded as failed rather than presenting the partial response as a completed task.
A missing retained terminal status is reported as unknown failure.

## Restart, deletion, duplicates

- At startup every run still `asking` or `running` is closed as failed with "MiniClaw stopped
  before this run finished. It was not run again." A background run, a handed-over one included,
  then reaches the parent with its next message as any background result does.
- `update_run` changes only an unfinished run, so a late or duplicate event cannot rewrite an
  outcome.
- Deleting a conversation deletes the runs inside it at their provider. A detached run stays.

## Claude conversations

A Claude conversation delegates through the same tool. Claude Code has no dynamic tools, so
MiniClaw's MCP bridge ([`tool_bridge.rs`](../../server/src/tool_bridge.rs)) lists
`miniclaw_delegate` beside the file tool when the backend offered it, and forwards the call
with its tool name. The backend re-emits it as the same `item/tool/call` a Codex conversation
sends, so one handler serves both providers. A Codex agent may delegate to a Claude agent and
the other way round, because a run is just the recipient's own conversation.

Facts from the official documentation (code.claude.com/docs/en/env-vars, read 2026-09-21):
`MCP_TOOL_TIMEOUT` defaults to about 28 hours and stdio servers have no per-request timer, but
a stdio tool call with no response and no progress notification is aborted after 30 minutes
(`CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`, Claude Code 2.1.203 or later). A per-server `timeout`
raises that window. MiniClaw therefore sets `timeout: 3900000` on its MCP server and the idle
variable to the same value, and its own bridge waits 3900 s instead of the earlier 190 s. The
same change lets a scheduled Claude run wait its full approval time.

Measured with the real CLI (Claude Code 2.1.278, Haiku 4.5,
[claude-delegation.test.mjs](../../tests/experiments/claude-delegation.test.mjs), opt-in):

| Case | Result |
|---|---|
| Wait, delegating in the middle of a conversation, user answers after 20 s | The tool call stayed open; the recipient ran under its own instructions; the lead quoted its answer |
| Background | The lead answered at once that the review was under way |
| Background result with the next message, first wording | **Failed:** the lead said it was still waiting, trusting its own earlier words over the delivered result |
| Same, with wording that says the run has ENDED and is final | Passed |

Not measured: a wait of more than half a minute, so the 30 minute idle rule and the raised
window are configured from the documentation, not observed.

### Why not Claude's own subagents

The CLI can define subagents (`--agents <json>`) and run them with its `Task` tool. MiniClaw
does not map its agents onto them, for three reasons. A native subagent runs inside the
parent's process with the parent's MiniClaw token, so it would act with the **parent's**
permissions, which the delegation rules forbid. MiniClaw would see none of it: no wait or
background prompt, no named block, no time limit, no record. And it could not reach a Codex
agent. `Task` is controlled by the separate Subagents switch (`subagents`). These helpers
keep the parent's restricted tools; they are distinct from MiniClaw's named-agent delegation.

## Limits of this version

- Whether **Codex** keeps a tool call open for a long wait-mode run is not verified; MiniClaw's
  own timeout allows 65 minutes, and the parent waits at most 60, then a longer run continues in
  the background. The hand-over has been tested with the fake Codex only.
- One request is asked about at a time per conversation.
- A parent turn for a finished background run does not start after the user stopped a response,
  while the queue is paused, or while a send's outcome is unknown; the result then waits for the
  user's next message ([Background results](#background-results-and-the-parent-agent)).

## Tests

`tests/delegation.test.mjs` (including the main agent's own limits and a run stopping at its limit,
waited or in the background, in shortened test minutes; the hand-over at the hour: the parent
carrying on, the later result, stopping the parent after it, a restart during it; the limits in
the tool description and the note, following a change to the main agent's),
`tests/main-agent.test.mjs` (the main agent's limits saved and kept), `server/src/agents.rs`,
`server/src/main_agent.rs`, `server/src/briefing.rs` and `server/src/api/delegation.rs` unit tests
(bounds, older records, the wording of the limits), `web/tests/delegation-limits.test.ts`,
`tests/browser/main-agent.spec.ts` ("the main agent's delegation limits..."),
`tests/browser/agent-cards.spec.ts` ("a waited run still going at the hour..."),
`tests/host-tools.test.mjs` ("a Claude agent with recipients keeps its file tool..."), `tests/agents.test.mjs` ("an agent may name existing
agents..."), `web/tests/chat.test.ts` ("a delegation asks wait or background..."),
`tests/browser/chat.spec.ts` ("a delegated run shows as a named block..."), and
[`delegation-progress.spec.ts`](../../tests/browser/delegation-progress.spec.ts) (live messages
and opening a running task on desktop/mobile). Real Codex and Claude follow-up status checks
in [`real-e2e.test.mjs`](../../tests/experiments/real-e2e.test.mjs) returned the correct run ID
and completed status without a second delegation. Running/approval states use fixtures.

## Approvals while viewing another chat

`working_tasks` in [`api.rs`](../../server/src/api.rs) includes live `delegationPrompts`
in the existing background-status response without loading any transcripts. Each entry
carries its owning `thread`, `threadName`, prompt ID and absolute `expiresAt`. Expired
prompts and chats that stopped are omitted. The answer route also refuses expired IDs.

[`chat.svelte.ts`](../../web/src/chat.svelte.ts) polls this endpoint every two seconds
while the page is visible and when the window regains focus. `pendingDelegation` merges
the current chat's prompt with that global list; answered IDs are suppressed so an older
in-flight status response cannot reopen them. The popup in [`App.svelte`](../../web/src/App.svelte)
uses the prompt's source thread for answers and attachments, preserving the selected
conversation and draft. Each pending request still requires its own explicit answer.

Backend and frontend regression tests cover background discovery, source ownership,
queue progression, expiry filtering and preventing an answered prompt from reappearing.
