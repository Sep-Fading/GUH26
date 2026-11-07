# Memory

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
