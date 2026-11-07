# Swarm

Several agents work towards one shared goal. A lead agent splits the goal into tasks on a shared
ledger; development and testing agents claim tasks from the ledger and never talk to each other.
When an agent's link drops, the rest of the swarm keeps going.

Code: [`server/src/api/swarm.rs`](../../server/src/api/swarm.rs) (backend),
[`web/src/SwarmView.svelte`](../../web/src/SwarmView.svelte) (mission control page).

## Run it

```
npm ci
npm start          # builds the page and the backend, serves http://127.0.0.1:8787
```

You need Codex or Claude Code installed and signed in, and bubblewrap on Linux. Open the page and
choose **Swarm** in the sidebar.

## Platforms

| System | How to run | Sandbox for agents' commands |
|---|---|---|
| Linux | `npm ci && npm start`; needs Rust, Node 24, bubblewrap | bubblewrap. Tested |
| macOS (Apple silicon) | `npm ci && npm start`; needs Rust, Node 24, Xcode command-line tools | Seatbelt, built into macOS. MiniClaw's original platform; the swarm itself is not run on a Mac yet |
| Windows, via WSL2 (recommended) | Inside an Ubuntu WSL2 shell, exactly as Linux (`sudo apt install bubblewrap`); open http://127.0.0.1:8787 in the Windows browser | bubblewrap inside WSL2 |
| Windows, native | `npm ci && npm start`, and switch on **Roles:** a lead plans the tasks. Development agents build. Code review agents review every
finished development task; a reply starting with `CHANGES:` files a fix, and the fix is reviewed
again. Testing agents run tests; a reply starting with `FAIL:` files a fix and a re-test. Each role
gets Anthropic skills (Apache 2.0), listed in
[`server/swarm-skills/NOTICE.md`](../../server/swarm-skills/NOTICE.md).

**Each swarm is a MiniClaw project** with the swarm's folder as its workspace, so all its agents'
conversations belong to it. The sidebar lists swarms instead of projects. Swarms are saved under the
data folder (`swarms/<id>.json`), so the list survives a restart. One that was running when the
backend stopped shows as interrupted.

**Unsandboxed shell** before launching. Without an installed release, MiniClaw's file and command tools are refused, so agents work through the unsandboxed shell | None: agents' commands run as you. The sandbox exists only after installing a release package (UPSTREAM-README.md, Part 2) |

Codex or Claude Code must be installed and signed in on the system that runs the backend (inside
WSL2 for WSL2).

## How it holds up when a link breaks

- **Claims have leases.** An agent claims a task for 40 s and renews the lease with each heartbeat
  (every 5 s). Each claim bumps the task's `version`.
- **A cut link loses no work.** The agent keeps working. Its heartbeats stop reaching the ledger,
  and its result waits in a local queue.
- **Leases run out.** After 40 s without heartbeats the task opens again, and another agent claims
  it. Three missed heartbeats (15 s) count the agent as unreachable on the status page.
- **Queued results are checked when the link comes back.** The queue replays, and a result counts
  only if the agent still holds the task at the version it claimed. A late result never overwrites
  finished work or duplicates it.
- **Failures are retried.** A failed model turn reopens the task; after 3 failures the task is
  given up.
- **Failed tests file fixes.** A test reply starting with `FAIL:` files a fix task and a re-test
  after it, up to 2 rounds.

**Unsandboxed shell** (composer toggle, off by default; `"unsandboxed": true` in the API): while the
swarm runs, every agent's `run_shell` runs as you, outside the sandbox, with your files and network.
It doesn't expire, and the grant is removed when each agent's turn ends. Use it on native Windows,
or when agents need the network.

Every agent is a real MiniClaw conversation, on Codex or Claude depending on its model. All agents
work in one folder, `~/swarms/<goal>-<id>`, through MiniClaw's sandbox. Full access is on, so
commands don't ask for approval, and there is no network. The lead uses the dev model.

**Limits:**
- Swarms live in memory, so restarting the backend ends them.
- The link that gets cut is simulated: it's the agent's connection to the ledger, not to its
  provider.

## API

Every POST needs `Content-Type: application/json` and `X-MiniClaw-Request: 1`. The backend listens
on `127.0.0.1` only.

| Method and path | Body | Answer |
|---|---|---|
| `POST /api/swarms` | `{goal, leadModel?, dev: {count 1-6, model}, review?: {count 0-4, model}, test: {count 0-4, model}, unsandboxed?: false}` | 201, snapshot |
| `GET /api/swarms` | | `{swarms: [{id, goal, status, created, counts}]}`, newest first |
| `GET /api/swarms/{id}` | | snapshot |
| `POST /api/swarms/{id}/link` | `{agent: "Dev-2", up: false}` | snapshot. Cuts or restores a link; restoring replays the queue |
| `POST /api/swarms/{id}/tasks` | `{text, kind?: "dev" or "test"}` | snapshot. Adds a task while the swarm runs |
| `POST /api/swarms/{id}/stop` | `{}` | snapshot |

Model ids come from `GET /api/models` (field `model`).

### The snapshot

```jsonc
{
  "id": "…", "goal": "…", "workspace": "/home/you/swarms/…",
  "status": "planning" | "running" | "done" | "failed" | "stopped" | "interrupted",
  "projectId": "…",                                       // the swarm's MiniClaw project
  "created": 1794066415544, "finished": null,            // ms since the epoch
  "counts": { "done": 1, "total": 3 },
  "members": [{
    "name": "Dev-2", "role": "lead" | "dev" | "review" | "test", "model": "gpt-5.6-luna",
    "linkUp": true,          // the simulated link
    "reachable": true,       // link up and a heartbeat within 15 s
    "lastSeen": 1794066420000, "uptime": 98.7,           // percent since it joined
    "status": "Working on #2 Web page", "task": 2, "queued": 0, "done": 1,
    "thread": "…"            // its current MiniClaw conversation
  }],
  "tasks": [{
    "id": 2, "title": "…", "detail": "…", "kind": "dev" | "review" | "test",
    "state": "open" | "claimed" | "done" | "failed",
    "owner": "Dev-2", "leaseUntil": 1794066460000, "after": [1],
    "attempts": 0, "version": 3, "fixRound": 0, "result": "…"
  }],
  "events": [{ "at": 1794066415544, "kind": "info" | "ok" | "warn" | "fail" | "done", "text": "…" }]
}
```

Building on it:
- **Status page:** poll `GET /api/swarms/{id}` and use `members[].reachable`, `lastSeen`, `uptime`,
  and the `warn` events for incidents.
- **Issue board:** `tasks` is the ledger. `owner`, `leaseUntil` and `version` give claim state;
  `result` holds the agent's summary or the failure.
