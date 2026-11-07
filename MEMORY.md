# Memory

## Swarm: failed turns retry after a delay (2026-11-07, branch david-branch)

User asked that a failed task wait a few seconds and retry, with the agent's tag showing it. Done in server/src/api/swarm.rs: `worker` retries a failed turn on the same issue by the same agent after RETRY_DELAY_SECS (5), heartbeating meanwhile so the lease holds; `Ledger::failed` counts each failure and allows a retry only while the agent holds the claim at its version and the issue has failed fewer than MAX_ATTEMPTS (3) times; `deliver` no longer counts failures itself, it reads the count (release for another agent, or Human Review at 3). Member has `retrying` (snapshot `members[].retrying`); status reads "Failed · retrying #N in Xs (attempt a of 3)" then "Retrying #N title (attempt a of 3)"; RunningAgents.svelte shows an amber dot and text. Docs in docs/swarm/README.md. New unit test `a_failed_turn_is_retried_while_the_claim_holds`. Not compiled or run: written from a MiniClaw sandbox on macOS without a Rust toolchain or node_modules. Next: `cargo test --lib swarm` and `npm run check`, then try it with a swarm.

## Project issues and Board (2026-11-07, branch issues-board)

Hackathon feature, built on WSL Linux (Rust 1.99.1 and Node 24.21.0 installed in ~/.cargo and ~/.local/node). Per-project issues (bugs/features) in agents.sqlite3 (server/src/issues.rs): one owning agent at a time; only the owner moves (issue_stage: dev, qa, agent_review), finishes (issue_done) or releases (issue_release, handover required); agents may only raise severity. Stages: backlog, dev, qa, agent_review, review (Human Review), closed. issue_done closes when severity <= the project's level (issue-settings closeWithoutReview, default low), else review until the user closes. Agent actions ride the miniclaw_project tool (server/src/issue_tool.rs; schema budget raised 3200 -> 3600 bytes); briefing line and per-message environment note ("Issues: ...") tell agents. User routes in server/src/api/issues.rs, live SSE at /api/projects/{id}/issues/live. UI: Issues and Board tabs in WorkspacePanel (IssuesPanel.svelte, BoardPanel.svelte, one live subscription via watchIssues), IssueSettings.svelte in project settings, 'issue' Activity kind. Design mockups: claude.ai artifact TohN2A13F2j8Pd8CcFfHHg.
Checks on Linux: tests/issues.test.mjs 7/7, project-memory 5/5, web vitest 179/179, svelte-check 0 errors, cargo test --lib 236/236, web build ok, browser projects.spec 13/13. Known Linux-only failures not caused by this: briefing.test.mjs 2 (macOS line, Mac sandbox gate), claude.test.mjs safe-mode test (fails on unchanged backend too). Unverified: workspace-panel.spec.ts fails at the Changes view "+1 −1" (no-project chat; not compared against unchanged code); nothing run on Windows; file tools do not work on Linux (no sandbox). Next: build a Windows package from this branch (rustup + NASM needed on the user's x64 PC; x64 never run before), update the 0.1.7 install at C:\Users\adamm\AppData\Local\MiniClaw Install, try with a real agent.


## Start (2026-11-07)

User asked for a minimal MiniClaw without the Roblox kit, cross-platform with Linux. Decisions:
new sibling repo `~/miniclaw-mini`; scope = chat (Codex + Claude), sandboxed tool, agents,
delegation, MCP servers and skills, no VM; Linux + macOS + Windows.

Done: repo initialized; AGENTS.md, PLAN.md, TESTING.md written; CODING_STYLE.md copied from
`../miniclaw-windows`. No code yet, nothing committed.

Research behind the Linux sandbox choice: codex-rs/linux-sandbox README (bubblewrap + seccomp,
Landlock legacy, `--unshare-net` plus a TCP->UDS->TCP bridge for proxy mode), the bubblewrap
README and man page, docs.kernel.org Landlock. Dev machine: Arch, kernel 7.2.14, bwrap 0.13.1,
Landlock in the LSM list, unprivileged userns on, Codex 0.166.0, Claude Code 2.1.319, Node 26.13,
cargo 1.99.1.

Probe on Linux (hand-run bwrap, not a test): with `$HOME` granted read-only and the masks applied
last, the command saw `~/.codex` and `~/.claude` empty (host has 38 entries in `~/.codex`), got
"Permission denied" reading `~/.claude.json`, could write only inside the bound grant, and could
not connect to 1.1.1.1:443. Lesson: `--tmpfs /tmp` hides grants under `/tmp` unless they are bound
after it, so mount order is part of the contract. Why `~/.claude.json` reads as denied rather than
empty is not explained yet.

## Linux version running (2026-11-07, later)

User: "get a linux version running, dont worry about the rest for now". The whole miniclaw-windows
source (0.1.8, `git archive HEAD`) was copied in; the original README is `UPSTREAM-README.md`.
A subagent removed the Roblox bundle from `scripts/` and the JS tests. The backend half is still
there: `server/src/bundles.rs`, `--add-bundle` in `main.rs`, `server/tests/bundles.rs`. Linux work
is listed in PLAN.md "Status on Linux"; the results and the test changes, with evidence, are in
TESTING.md "Results on Linux". Nothing is committed.

Trial instance: `MINICLAW_PORT=8799 MINICLAW_DATA_DIR=<scratchpad>/run1/data node scripts/start.mjs`,
health `{"codex":"connected","claude":"connected","platform":"linux"}`. Normal use: `npm start`
(port 8787, data in `~/.local/share/miniclaw`), or `miniclaw` from anywhere: the user asked for it on
PATH, so `~/.local/bin/miniclaw` (outside the repo) runs `node scripts/start.mjs` in this checkout.
Checked from /tmp on port 8798: both providers connected. The 8799 trial instance has been stopped.
The launcher's commands are `start` (the default), `start --watch`, `setup` and `help`. On Linux, `setup`
only checks what is there and installs nothing: bubblewrap and a sandbox probe, Node, cargo,
node_modules, at least one of Codex or Claude, and the shell. The repo's setup wizard refuses Linux.

Open:

- Cut the left-out features: VMs, Computer Use, schedules, backups, terminal UI, workspace panel,
  analytics, projects, attachments, queue, slash commands, timed unsandboxed shell, the wizard
  and bundles.
- Unmasked missing protected paths (PLAN.md known gap).
- No seccomp or Landlock layer yet.
- Turn-ending reason codes are not built.
- Commit when the user asks.

## GUN26 hackathon fork (2026-11-07)

This repo (github.com/Sep-Fading/GUN26) is a copy of miniclaw-mini, made for the GreatUniHack track
"multiple agents working towards a shared goal without halting when communication fails". Teammates
build the status page and the issue board on top of it.

Added:
- `server/src/api/swarm.rs`: the ledger with leases, versions, heartbeats, a sweeper, offline queues
  and replay, plus the lead's plan and the workers. Routes are under `/api/swarms`.
- `web/src/SwarmView.svelte`: mission control, matching the approved design, reached through
  Swarm in the sidebar.
- `docs/swarm/README.md`: the API for teammates. Swarm agent conversations are hidden from the
  sidebar (in memory, so only for this backend run).

Checked on Linux:
- 5 ledger unit tests pass.
- A real swarm built greet.py and tests in 40 s; the tests pass.
- A real run through the page with Dev-1's link cut: the lease ran out after about 37 s, Dev-2 took
  over #1, the link was restored, and all 3 tasks were done in 3:54.
- svelte-check is clean and vitest passes 143/143.
- The swarm ledger tests were written after the code, under hackathon time.

Not committed or pushed: that waits for the user.

Unsandboxed shell toggle (user decision, 2026-11-07: "add a toggle without expiry"): `unsandboxed`
on `POST /api/swarms`, plus a composer toggle that is off by default, with a red warning and a badge.
`shell_access::Grant` gained `swarm: bool`: it has no expiry, does not need the agent's switch, and is
removed when each agent's turn ends. It is meant for native Windows without an installed sandbox.
Checked on Linux: an agent ran `ls ~ | wc -l` and got the real home count (23), so it was outside
the sandbox. Swarm conversations are also hidden by their opening words, which survives restarts.

Review agents, swarm projects, skills, setup guide (2026-11-07, user requests):
- Role::Review: every finished dev task files a review, and `CHANGES:` files a fix that is reviewed
  again (at most 2 rounds). Verdicts in Markdown bold are understood.
- Each swarm creates a MiniClaw project (workspace = swarm folder), and agent conversations are
  created in it. The sidebar's Projects section is replaced by a Swarms list (TaskNav). Snapshots
  are saved to `<data>/swarms/<id>.json`; ones that were running show as interrupted after a restart.
- Skills by role, from anthropics/skills and anthropics/claude-plugins-official (Apache 2.0, copied
  unchanged into `server/swarm-skills` with LICENSE and NOTICE.md). They are saved once as
  "Swarm · <name>" skills and selected per role through `create_chat`'s `skill_selection`.
  webapp-testing's `with_server.py` is copied into each swarm folder.
- SETUP.md: steps from source for Linux, Windows with WSL, and macOS.

Checked on Linux:
- A real run through the page (1 dev on GPT-5.6-Luna, 1 review and 1 test on Haiku 4.5): Dev-1 built
  slugify.py, Review-1 approved it, Test-1 ran 37 passing tests, and the goal was met in 1:05.
- The project exists with the right workspace, and the reviewer conversation had both review skills
  selected.
- Rust: 246 lib tests pass, with no build warnings. Web: vitest 143/143, svelte-check clean.
- Not run: the browser specs about projects in `tests/browser`, which cover the removed sidebar
  section. macOS and Windows have not been run.

Still not committed or pushed.

Agent conversations under their swarm (user request, 2026-11-07): every agent conversation is named
`<agent> · #<task> <title>` (the lead's is `Lead · plan`) through `name_chat`, so the name is the
user's and stays. Swarm conversations are no longer hidden; only old ones without a project are.
TaskNav nests them under each swarm row, which expands with a chevron. Running swarms and the one
on screen start open. While a swarm runs, the conversation list refreshes every 6 s.
Checked: a real run (1 dev, 1 review, 1 test). The review asked for changes, the fix was made and
then approved, 5/5 tasks were done in 1:28, and the sidebar showed the titled conversations under
the swarm. svelte-check is clean.

## Merge of the Scattz1 and ADM2005 forks, swarm on issues (2026-11-07)

- **Patches:** applied as patches (`git diff origin/main <fork>`; both forks base on 0bb5e40) with
  `patch --merge`. Conflicts were only MEMORY.md and one App.svelte line.
- **David (Scattz1):** Agents page, timeline, running box, and the cancelled status for stopped
  delegated runs. tests/delegation.test.mjs was updated for that requirement change (`cancelled`,
  "stopped by the user").
- **Adam (ADM2005, issues-board):** issues, the Board and Issues panels, the issue tool and the
  review level.
- **Swarm rewritten on Adam's issues** (api/swarm.rs):
  - The lead files issues, and agents claim them by stage (dev Backlog/In Dev, test QA, review
    Agent review). They move them with set_issue_stage and release, and a review finishes the issue.
  - FAIL or CHANGES sends an issue back to dev; after 2 rounds or 3 failed attempts it goes to the
    user (raised to high severity and finished).
  - Leases, heartbeats, link cuts and replay stay in memory; an expired lease releases the issue in
    the owner's name.
  - Swarm agents act as themselves in the issue tool (`api::swarm_actor`) and in David's analytics
    and running box (`summary.agentName`).
  - The review level is chosen at launch (default medium) and beside the tabs.
- **UI:** the chat-style progress is gone. The swarm page is launch, then Board/Issues, with
  David's running box docked beside them, which holds the agents and the Cut/Restore controls.
  `/api/active-turns` now also returns `swarms`.
- **Auto delegation:** the sidebar switch and `/api/delegation/auto` (`<data>/auto-delegation.json`).
  On means every agent may delegate to every active agent, and hand-offs start as "wait" without
  a prompt.
- **Checked on Linux:**
  - Rust lib 251 tests; web 191; issues, delegation and project-memory node suites pass.
  - A real run through the page: 2 dev, 1 review, 1 test. 2 issues were filed and went
    Backlog → QA → Agent review → closed, and the goal was met in 1:37.
- **Not yet:** a lease expiry exercised through issues in a real run (the link was cut after the
  agent had finished), and macOS/Windows. Nothing is committed.

Lead chat, Backlog rule, delete (2026-11-07):
- The Lead's conversation lasts. It files stories itself through the miniclaw_project issue_create
  action, and the user talks to it any time. Lead-only actions: issue_assign (gives a story to an
  agent, which stays in the Backlog until that agent takes it) and issue_review_level.
- Developers take only Backlog stories (assigned ones first); taking one moves it to In Dev.
- Escalation uses `await_review` (Human Review whatever the level).
- The swarm goes idle instead of ending, and resumes when new stories arrive.
- Swarm conversations are ordinary project chats (`delegated: false`, questions off), because a
  delegated run gets no project tool. Role rules live in the project instructions, and stories go
  out as short messages.
- Sidebar: Lead row, then a folded Agents group.
- Delete: `POST /api/swarms/{id}/delete` stops the swarm and deletes the project, its conversations,
  its issues and the saved record. The swarm folder's files are kept.
- Checked with real models: the Lead filed 2 stories, and by chat added #3, assigned it to Dev-1 and
  set the level to high. Delete checked through the API. Tests were cut short at the user's request.
