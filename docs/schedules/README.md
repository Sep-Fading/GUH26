# Scheduled runs

A schedule sends a saved prompt to one agent by itself. It is the only automatic trigger:
there is no file-change or event trigger.

## What the user sets

Settings, Schedules ([`Schedules.svelte`](../../web/src/Schedules.svelte)):

| Field | Values |
|---|---|
| Name, prompt | text; the prompt holds at most 8000 characters |
| Agent | one active agent |
| Project | optional. It decides the machine: a VM project runs in its VM, anything else on this Mac |
| Runs | every hour, day or week |
| Approval wait | 1, 2, 5, 15, 30 or 60 minutes, then the run is terminated. Default 5 |
| On/off switch | **off when saved.** "Run now" works either way |

Each row shows the last run's state, time and result, and links to its conversation. A run is
an ordinary conversation of the agent, so its transcript and [activity](../activity/README.md)
can be read like any other. At most 32 schedules exist.

## When a run starts

The background watcher checks every 20 s ([`schedules.rs`](../../server/src/api/schedules.rs)
`check`). A schedule is due when it is switched on, not already running, and its period has
passed since its last start. Schedules run only while MiniClaw runs; a missed run starts at
the next check. A schedule never starts a VM: a run whose VM is off fails with that reason.
One run of a schedule exists at a time.

## Nobody is there to approve

A scheduled run registers its conversation in `Backend::unattended` with the schedule's
approval wait. The tool handler ([`chat.rs`](../../server/src/chat.rs)) then keeps an approval
open for that long instead of the usual 60 s. The popup still appears if the conversation is
open, and an answer in time works as usual.

With no answer, the request is refused and the conversation is marked in
`unattended_expired`. The runner sees the mark, **interrupts the turn**, records the run as
`terminated` with the request it was waiting for, and saves a `scheduled_run_terminated`
notification. The run does not carry on past the refusal and is not left waiting. A failed
run saves `scheduled_run_failed`. Every run also ends at one hour.

To let a schedule do something without asking, add a permission rule to its agent.

## Restart

A run that a stopped backend left `running` is closed as failed at startup. It runs again
only when its period next passes.

## Limits of this version

- MiniClaw's Claude bridge now waits as long as the Codex path (3900 s), and Claude Code's idle
  limit for the MiniClaw server is raised to match; see
  [delegation](../delegation/README.md#claude-conversations). Whether Codex itself keeps a tool
  call open for an hour is **not verified**.
- The watcher's 20 s timer and a real model run have not been observed; tests use
  `POST /api/schedules/tick` and the fake Codex, with `MINICLAW_TEST_MINUTE_MS` shortening the
  approval wait.

## Routes

`GET`/`POST /api/schedules`, `POST /api/schedules/{id}`, `POST /api/schedules/{id}/delete`,
`POST /api/schedules/{id}/run`, `POST /api/schedules/tick`.

## Tests

`tests/schedules.test.mjs` (4): saved off and validated, "Run now", due and not due again,
termination after an unanswered approval with the interrupt and the notification.
`tests/browser/schedules.spec.ts`: create, last-run display, switch, delete.
