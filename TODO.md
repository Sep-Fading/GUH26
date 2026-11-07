# Remaining work

Reviewed 2026-09-20 against the code on `main`, after the Linux controller was removed and
the backend moved to the Mac with opt-in VMs. Detailed decisions are in [PLAN.md](PLAN.md);
verification evidence is in [TESTING.md](TESTING.md); the latest checkpoint is at the top
of [MEMORY.md](MEMORY.md).

Check an item off only after implementation and relevant tests. "Fixture-tested" means the
tests use stand-ins and the behaviour has not been seen with a real VM in this layout.
Items that described removed code are listed once at the end, with the reason, and are not
open work.

Working today: chat with Codex and Claude as independent providers, projects, agents,
skills and groups, a Seatbelt-confined file and shell tool on the Mac and in VMs, one
on-demand Master VM with a live desktop panel, Computer Use on the Mac and in the VM, and a
terminal installer with short commands.

## Next up

- [x] Deliver the current worker and capture helper to the installed VM. The user ran
  `miniclaw update-vm` and the guest installer on 2026-09-21; `miniclaw verify --chat` then
  passed every check (reported by the user).
- [x] Move the settings screens to shadcn-svelte: one Settings sheet, and `Button`, `Input`,
  `Textarea`, `Switch` and `Dialog` in its panels. Native checkboxes, radios and selects stay
  native by choice. Still Bits UI directly: tooltips, the project and confirmation dialogs,
  the composer's menus and the conversation rail.
- [x] PLAN.md rewritten as the current design on 2026-09-21; the earlier text is in
  [docs/history/plan-2026-09.md](docs/history/plan-2026-09.md). README passages brought in line.
- [x] Desktop stream measured with the real VM: 26.6 fps while the screen moves, median frame
  gap 34 ms; nothing is sent while it is still. See TESTING.md. Open: whether a VM started in
  the background gets its network interface.
- [x] Decide on provider-native tools. Decided 2026-09-20; see
  [the earlier plan](docs/history/plan-2026-09.md#provider-native-tools-decision-2026-09-20).
- [x] Widen `miniclaw_scoped_native` with `list_folder`, `search` (literal text) and
  `replace_in_file` (one exact passage). Rust: `server/tests/executor.rs`, `scoped_tool.rs`;
  end to end: "a granted folder can be listed, searched and edited in place" in
  `tests/host-tools.test.mjs`. In a VM these need the new worker (`miniclaw update-vm`).
- [x] Give shell commands HTTPS access to approved hosts through a per-command proxy, off by
  default, with a project or agent default and a conversation override. See
  [docs/command-network](docs/command-network/README.md). A real download through the proxy
  was observed. Not yet observed inside a real VM.
- [x] Probe which fixed-function native tools Claude Code lets MiniClaw enable. Done
  2026-09-20 ([docs/native-tools-probe](docs/native-tools-probe/README.md)): web search and
  restricted subagents are safe as they are; web fetch reaches local addresses by IP and is
  only acceptable with per-domain allow rules; there is no planning tool by that name.
- [x] Build the `web_tools` switch. Claude: search, restricted subagents, the to-do tools
  and fetch limited to the conversation's hosts and domains (exact and `*.` rules); a
  separate `plan_mode` switch. Codex: `web_search` set to `live` when a thread starts or is
  loaded. Real checks: the real Claude CLI answered from a web search inside a real backend,
  and Codex started the MiniClaw way searched the web and had no file or shell tool. Known
  limit: Claude Code also lets fetch reach its own built-in list of documentation sites.
- [ ] Codex `multi_agent` is not probed and stays off. For Codex a changed switch reaches an
  open conversation only when it is loaded again.
- [x] `tests/browser/providers.spec.ts` "the Providers panel disables Claude" was a race with the
  closing Settings sheet; fixed and repeated 36 times under load.

## Briefing

Built on 2026-09-21: [briefing](docs/briefing/README.md). Observed once with a real Codex model.

- [ ] Web search in VM conversations: the VM worker must accept `web_search: live` (worker
  protocol change and `miniclaw update-vm`). Until then it is "not allowed" there.
- [ ] A VM conversation gets no folder listing, git state or toolchain in its briefing.
- [ ] Observe Claude with the briefing, and whether models ask less across ordinary work.
- [ ] The folder picture is as old as the conversation's last load.

## Questions from the model

Built and fixture-tested on 2026-09-21: [questions](docs/questions/README.md).

- [ ] Observe with real models: that they ask at sensible moments and not too often, and that a
  tool call left open for many minutes survives in the real Codex and the real Claude CLI.
- [ ] VM conversations are not offered the tool: the VM worker accepts exactly one tool. It needs
  a worker protocol change and `miniclaw update-vm`.
- [ ] No notification is raised while a question waits and the page is closed.
- [ ] A question is not shown in the transcript itself, only as the popup and in Activity.

## MCP servers

- [x] Servers that listen (`listen` ports, Roblox Studio). Verified under Seatbelt with the fake
  server; the user has to try it with Studio open.

## Terminal

- [x] Slice 1: the user's shell on the Mac, in the sidebar, sandboxed like a command.
- [x] Slice 2: the agent's approved commands run in the shared shell, output captured between markers.
- [x] Slice 3: a terminal inside a VM through the worker (protocol 6). Tested through the stand-in
  worker; the user has to run `miniclaw update-vm`, restart the VM, and try it for real.
- [ ] A command that outlives the limit inside a VM finishes its Activity entry with no exit code;
  the terminal shows the rest but the entry is not finished later, as it is on the Mac.
- [ ] `(allow pseudo-tty)` is not granted, so `tmux`, `script` and `expect` fail in the terminal.
- [ ] The agent cannot read the shared screen; it sees only its own commands' output.

## Projects that remember

- [x] Standing instructions (`AGENTS.md`, `CLAUDE.md`, project instructions), shared memory, the
  digest and the look-up, for both providers. Observed with a real Codex and a real Claude model.
- [x] VM conversations carry `miniclaw_project`, `miniclaw_ask` and `miniclaw_mcp` when their worker
  names them (worker protocol 5). Tested through the stand-in worker, which runs the real worker
  code and policy.
- [ ] Not seen in a real VM yet: the user has to run `miniclaw update-vm`, restart the VM, and try
  memory, a question and an MCP server from a VM conversation.
- [ ] Rule files (`AGENTS.md`, `CLAUDE.md`) inside a VM's folder are not read.
- [ ] `AGENTS.md` in subfolders or above the project folder, and `@imports`, are not read.
- [ ] Not observed: whether real models save notes unprompted at sensible moments, and how much
  the index and digest cost in real prompt tokens over a long conversation.
- [ ] Copies of project conversations exist only from 2026-09-22 on; older ones appear in the
  digest once they are opened or get a new response.

## Analytics

- [x] A Settings tab with the figures, by agent, model, skill, MCP server, project and tool kind,
  from each provider's own token counts. Observed with a real Codex model and a real Claude model.
- [ ] Attribution is by participation, so the per-agent and per-skill token columns overlap and do
  not sum to the total. Measuring what a skill alone costs would need a turn's tokens split by
  what was in its prompt, which no provider reports.
- [ ] A VM conversation's turns are recorded, but its own worker reports no token counts, so those
  rows carry zero tokens.
- [ ] Nothing exports the figures; there is no CSV or copy button.
- [ ] Rows are kept for a year and never pruned by size, only by age.

## Small things found along the way (2026-09-22)

- [ ] Escape does not close the Tools dropdown; its own button and an outside click do.
- [ ] `tests/browser/providers.spec.ts:45` failed once on desktop in a full run and passed 3 of 3 alone.
- [ ] `tests/browser/environments.spec.ts:74` did the same on mobile in the next full run. Two different specs,
  each once, each fine alone: the full browser run has a timing weakness worth finding.

## Files and pictures in messages

- [x] Uploads in chats for both providers: pictures, inlined text files, saved files, queue,
  Trash on delete. Observed with a real Codex model and a real Claude model.
- [ ] Pictures in VM conversations: the VM worker accepts text input only. It needs a worker
  protocol change and `miniclaw update-vm`. No copy of an upload is placed in a VM either.
- [ ] PDFs are plain files. Claude could read them as documents; Codex cannot.
- [ ] Unsent uploads of an abandoned draft stay on disk until the conversation is deleted.
- [ ] HEIC pictures are plain files; `sips` could convert them.

## Computer Use on the Mac (reported 2026-09-21)

- [x] A failed step shows the helper's error and a plain hint; questions read "Asked" with their
  real duration; Activity entries reach an open page at once.
- [ ] The cause itself: the installed helper (26.913) starts `codex` from the removed
  `/Applications/Codex.app` (error -10005). The user has to open the ChatGPT desktop app so it
  updates the helper, then retry. Untested.
- [ ] No check warns before a turn that the helper is older than the desktop app's plugin.

## Message queue, capabilities and the main agent

Built and fixture-tested on 2026-09-21: [message queue](docs/message-queue/README.md),
[capabilities](docs/capabilities/README.md).

- [ ] Observe with real models: a queue across several real turns, real Codex live search and
  real Claude web search on by default, and a capability greyed out for a real agent.
- [ ] Look into `/api/account` answering 503 `codex_unavailable` when a permission changes at the
  same instant the page refreshes its account. Seen once in a browser test, not reproduced.
- [ ] A queue in a Claude conversation and in a VM conversation is untested.
- [ ] Queued messages cannot be reordered (not asked for).
- [ ] The main agent has no instructions and no file or host rules (not asked for).
- [ ] Computer Use is still unavailable to named agents; the dropdown says so.

## MCP servers and slash commands

Built and fixture-tested on 2026-09-21: [MCP servers](docs/mcp-servers/README.md),
[slash commands and /mcp-create](docs/slash-commands/README.md).

- [ ] Observe it for real: one published stdio server (sandboxed, then with the hosts it
  needs), one remote server with a token, the real Keychain prompts, and a real model using
  `miniclaw_mcp` and following `/mcp-create` to a working server.
- [ ] OAuth for remote servers. Only a bearer token exists.
- [ ] A project's `.mcp.json` is not read by the import.
- [ ] VM conversations cannot reach MCP servers; the servers run on the Mac.
- [ ] Images and audio in a tool result are named, not passed to the model.
- [ ] Resources and prompts of a server are not offered, only tools.
- [ ] A server's tool list is read once per connection; `notifications/tools/list_changed` is ignored.
- [ ] The release package and `miniclaw update` were not rerun since `reqwest`, `toml` and
  `security-framework` were added.

## Tools and permissions

One MiniClaw-owned tool, `miniclaw_scoped_native`, reads files, writes files and runs shell
commands under an Apple Seatbelt profile built from the conversation's grants. It runs on
the Mac for chats bound to an agent or project, and in any VM through that VM's worker.
AppleScript and TextEdit operations are refused on every machine.

- [x] Put exact "Approve Once", "Approve for this session" (eight hours) and "Reject"
  choices in the approval popup, bound to one pending operation. Rust:
  `scoped_approvals_dispatch_and_store_only_sessions`, `scoped_reject_never_executes`.
- [x] Enforce file read and write rules in a sandboxed executor with symlink, traversal and
  path-race tests (`server/tests/executor.rs`, `seatbelt.rs`, `folder_permissions.rs`).
  The Seatbelt profile was also observed in the real guest: the granted workspace was
  writable; `~/.codex/auth.json`, `~/Desktop` and the network were denied.
- [x] Route shell commands through one reviewed tool that shows the exact text and working
  directory, with no execution on reject or timeout.
- [x] Give every VM chat the tool and tell the model it runs inside the VM; the VM's
  workspace folder needs no approval. A chat carries either this tool or Computer Use.
- [x] A permission revoked during a pending or queued operation stops it, and grants and
  revocations hold across a restart. Rust: `a_block_saved_while_the_approval_is_open_stops_the_operation`,
  the queued-call revocation test, `session_grant_is_exact_and_expires_after_eight_hours`
  (store reopened) and `a_revocation_survives_a_restart`.
- [x] Software installation and administrator-level changes cannot happen through the
  tool. Rust: `software_installs_and_administrator_changes_are_refused` tries `/Applications`,
  `/usr/local`, launch daemons and agents, preferences, shell profiles, `sudo`, `defaults`
  and `launchctl` inside the sandbox; all fail and nothing is created.
- [x] Provider sign-in files and MiniClaw's own state are closed to every grant, including
  a granted home folder (`seatbelt::protected_roots`). Rust:
  `miniclaw_state_and_provider_sign_ins_stay_closed_inside_a_granted_folder`.
- [x] A worker cannot alter backend state through files or events. Files: the protected
  roots above cover the data folder, the installation (settings, pairing key), the VM link
  folder and the ledger. Events: `a_worker_cannot_finish_another_machines_turn` (it fails
  when the ownership check is disabled). Requests a worker may make are bounded by
  `server/tests/guest_computer.rs` (`rejects_management_rpc`, `rejects_policy_overrides`).
- [x] Text produced by a worker cannot act in the page. Assistant messages render Markdown
  with raw HTML escaped, https-only links and images only on a click; rail previews render Markdown with HTML, links and images removed. Browser:
  "long rails keep the current mark visible and previews stay inert" feeds an
  `<img onerror>` payload and finds no `img`, `a`, `script` or `[onerror]` in the preview.
  The page's Content-Security-Policy also forbids inline script.
- [x] Computer Use on the Mac is refused for the provider apps, System Settings, passwords,
  terminals, script runners and MiniClaw; a browser approval carries a warning. Known limit:
  a browser tab with MiniClaw open cannot be told from any other tab.
- [ ] Implement other declared capabilities, including Deep Research, only with their own
  tested executors and grants. Unsupported definitions stay inactive.
- [x] An interrupted chat creation recovers its original binding without another
  `thread/start` and without sending anything. Node: "failed binding move recovers the same
  chat after restart" in [storage-errors.test.mjs](tests/storage-errors.test.mjs).

## Project VM controls and routing

- [x] Ask Mac or VM when creating a project and keep the choice fixed. Browser: "project
  machine choice is visible and locked after saving".
- [x] Default VM projects to Master, with the option of a separate project VM.
- [x] Ask Mac or VM for standalone conversations; allow a Mac conversation inside a VM
  project. Browser: "conversation may use Mac under a VM project and cannot switch later".
- [x] Persist project and conversation bindings; refuse an unknown VM and an unavailable
  worker without falling back to the Mac (`server/tests/environment_routing.rs`).
- [x] Require an explicit machine choice for older conversations. Browser: "older
  conversation asks once for its machine".
- [x] Ask before starting a stopped VM for a message, and handle declined, failed and stale
  requests visibly. Observed with the real Master VM.
- [ ] Provision project VMs through the interface. Fixture-tested ("dedicated project shows
  install progress and saves only after its VM is ready", retry and cancel); not yet run
  against a real project VM in the Mac-backend layout.
- [ ] At macOS Setup Assistant, tell the user what to finish, wait for confirmation, verify
  guest readiness independently, then continue dependency setup and sign-in. Fixture-tested
  only.
- [ ] Use a configurable standard setup, initially macOS-supplied apps only. Preserve guest
  files, apps and settings; do not clone private Master data or credentials.
- [x] Put dedicated VM settings and open, start and stop controls in the project
  (`VmControls.svelte` in Project settings). Browser: "a project controls its own VM". Not yet
  observed with a real project VM.
- [x] Add configurable idle shutdown, off by default, per VM: off, 15, 30, 60 or 120 minutes,
  a 60 s warning with "Keep running", then a clean stop and a notice. See
  [docs/vm-lifecycle](docs/vm-lifecycle/README.md). Transfers, backups and background jobs do
  not exist yet; each must call `idle.touch` when it is built. Use of the VM's native window
  is not seen by the backend.
- [x] Add project deletion with an unchecked checkbox labelled "Also delete this project's
  VM". The VM is deleted permanently, only when stopped and after the project's name is typed.
  Master can never be named. The project's conversations are deleted with it (user decision,
  2026-09-21).
- [x] Retain an undeleted dedicated VM as an orphan, listed in Settings, Virtualisation with
  open, start, stop and confirmed delete; no project reassignment flow.

Contracts: [execution environments](docs/history/plan-2026-09.md#execution-environments-2026-09-13).

## VM display and manual control

- [x] Embed a small live guest desktop in MiniClaw. The page reads the stream and paints a
  canvas; see [docs/desktop-stream](docs/desktop-stream/README.md). Observed with the real VM.
- [x] Keep manual input available without pausing agents: the VM's own window has a
  "Control this VM" switch. Used in the real guest to grant permissions.
- [ ] On hover or keyboard focus of the panel, offer Windowed and Fullscreen. Today the
  panel has one "Open desktop window" button, and the native window has its own fullscreen
  button.
- [ ] Keep a persistent fullscreen toolbar with VM identity, Pause and Exit.
- [ ] Make Pause target the desktop-controlling agent, not unrelated agents. Define what
  happens to input already in flight and how work resumes. Nothing exists yet.
- [ ] Test two agents controlling one guest repeatedly before enabling concurrent desktop
  control. Cover separate apps and contention within one app; serialize if needed.

## Global settings and resource limits

- [x] Persist global limits: 50% of host CPU count, 50% of physical RAM and 25% of disk
  capacity by default.
- [x] Enforce durable, serialized reservations across every installation on this Mac
  ([resource-store.mjs](scripts/vm/resource-store.mjs)).
- [x] Count every state except `stopped` for CPU and RAM, and release only after a
  confirmed stop. Observed: a real shutdown left the row `stopped`.
- [x] Reconcile after a crash: `miniclaw ledger --reconcile` releases an idle guest whose
  supervisor died, after a backup.
- [x] Free the budget of a deleted VM: `miniclaw ledger --forget`. Run for real on
  2026-09-20; disk reservation went from 201.1 to 64.5 GiB.
- [x] Show reserved capacity and the resource that blocks an operation: `miniclaw ledger`,
  and the setup wizard names an exceeded budget before it builds anything.
- [x] Add the Settings sheet with a category sidebar: Appearance, Providers, Agents, Skills
  and Virtualisation (Master VM status, editable limits, recorded VMs, reconcile, forget,
  update hint). Browser: `tests/browser/settings.spec.ts`. Dedicated VM settings stay in
  projects. Not yet observed against the real VM session.
- [ ] Apply admission to create, resume, resize and import, not only to start.
- [ ] Include backups in the disk accounting once backups exist.
- [ ] Support external storage only after capacity identity and shared APFS-container
  accounting are verified.

## File import and export

- [x] Add explicit Mac-to-VM and VM-to-Mac transfer through the interface, for multiple files
  and folders. See [docs/file-transfer](docs/file-transfer/README.md).
- [x] Require trusted user selection: the Mac side is chosen in the macOS panel the backend
  opens, and the page never names a Mac path. No shared folder, no general file API.
- [x] Name conflicts ask each time (Replace, Keep both, Skip, apply to all); cancellation,
  progress and failure reporting never call a partial copy finished.
- [x] Tested: traversal, links, a refused path, piece order, free space, cancellation.
- [x] Seen by the user with the real VM on 2026-09-21: the macOS panel opens in front of the
  browser, files move both ways, a repeated file asks about the conflict, and the VM shuts down
  in about 10 s. Open: a worker disconnect in the middle of a real copy.

## Backups and restore (experimental, parked)

The user does not want to invest in backups for now (2026-09-21). A small folder-backup
version exists and is labelled experimental; see [docs/backups](docs/backups/README.md).

- [x] Off by default for every VM; nothing is written until the user acts.
- [x] Workspace folder backups with a manifest of checksums, Daily or Weekly or manual,
  retention 1 to 10 with oldest-first removal to the Trash, three retries then a notification,
  and a checked restore into the VM's Transfers folder.
- [ ] Parked: whole-VM backups, destination picker and external drives, folder choice in the
  page, backups in the global disk budget, schedule conflicts on a shared Master, boot
  verification after restore, identity data, and the iCloud warning.
- [ ] Not observed with a real VM.

## Agent delegation and automatic runs

Built on 2026-09-21 against the fake Codex; see [docs/delegation](docs/delegation/README.md),
[docs/schedules](docs/schedules/README.md) and [docs/activity](docs/activity/README.md).

- [x] Bounded, durable delegated runs using only saved, active agents the delegating agent
  was given. Models cannot invent or activate agents, and a delegated agent cannot delegate.
- [x] Each run is the recipient's own conversation: its pinned revision, instructions, skills
  and permissions, on the parent's machine and project. Nothing of the parent's is lent.
- [x] The user chooses wait or background at every request. Results reach the parent as the
  tool's answer (wait) or once with the next message (background), under the agent's name.
- [x] Cancellation with the parent, time limit, restart closure without re-running, duplicate
  tool calls, deletion with the parent.
- [x] Schedules, off when saved, with visible run state and "Run now". An unattended approval
  waits a configurable time, then the run is terminated.
- [x] Activity tracker for MiniClaw's tool in every conversation, with coloured diffs.
- [ ] Observe all three with real model calls. Unverified: whether Codex keeps a tool call
  open for a long wait-mode run or a long approval wait.
- [x] Let a Claude conversation delegate, through the MCP bridge. Observed with the real CLI
  in both modes on 2026-09-21 (opt-in experiment `tests/experiments/claude-delegation.test.mjs`).
- [x] An account change ends a delegated or scheduled run with that reason.

## Notifications

- [x] Add global Notifications for failed responses and unexpected runtime disconnects.
- [x] Persist safe failure summaries and read state, link to the affected conversation, and
  avoid duplicate notices for one failure.
- [x] Keep credentials, screenshots, private contents and unnecessary paths out of
  notification records and their storage diagnostics.
- [x] VM failures reach Notifications: idle shutdown, failed backup, a VM that could not start,
  a VM that stopped by itself, a terminated scheduled run, an expired review.

## Installation, startup and updates

- [x] Run a fresh Apple silicon installation through the wizard. Done 2026-09-20 into
  `~/MiniClaw-VM`, reusing the cached macOS image.
- [x] Restart a provider that exits by itself, with backoff, without resending anything
  ([providers.rs](server/src/providers.rs) `supervise`).
- [ ] Complete a supported update procedure. `miniclaw update-vm` rebuilds the VM's setup
  disk, and the guest installer replaces the worker and helper; updating the installed
  backend and page is still a manual copy. Verify partial updates and retained data.
- [x] Standalone distribution: `miniclaw package` builds a self-contained, unsigned release with
  prebuilt files and the official Node. Built and its Mac part installed from a bare PATH on
  2026-09-21. Open: a VM installation from a release, and a Mac with no developer tools at all.
- [ ] Measure on-demand VM start time, resource use, disk growth and concurrent workloads
  for supported host sizes.

## Remaining hardening and acceptance

- [x] Deploy idle-history eviction.
- [x] Bound retained replay to 2 MiB and 512 events per chat, and SSE admission to 64
  streams.
- [x] Bound retained RPC notification payload to 32 MiB, preserve uncertainty on overload
  and keep reply handling fair.
- [x] Remove obsolete disposable VM artifacts. Done 2026-09-20: 122 GB of old lab disks and
  images moved to the Trash; reports and logs kept.
- [ ] Test Stop during an actual tool action, early approvals, and Computer Use reliability
  (stale coordinates, first click) in the VM.
- [ ] Test sustained guest networking, DNS and host-address changes and hostile network
  destinations. Preserve host and LAN denial and no public listener; outbound uploads and
  tunnels remain a risk of permitted internet access.
- [ ] Verify ordinary-user workflows over extended sessions, worker failures, disk pressure
  and restart, with no silent failures or automatic resends.
- [x] A conversation that is still responding opens; its partial text is shown once.
- [x] Cache eviction: a finished send receipt no longer pins a conversation; kept receipts
  still answer a retried send after eviction.
- [x] At most 10,000 messages per conversation in memory; request bodies are capped at 256 KiB.
  Open: behaviour when all 32 slots hold protected conversations is only the `task_limit` error.
- [ ] Expand expired-login, inherited-config, provider, MCP, old-account race and
  unsupported-approval tests without signing the user out to create fixtures.
- [x] Axe in both themes and on the white accent, 200% zoom, keyboard-only checks, reduced
  motion. Open: no screen-reader session has been done by a person.
- [x] Audit of 2026-09-21: [report](docs/security/dependency-audit-2026-09-21.md). Open for the
  user: whether to raise `insomniacslk/dhcp` in the network helper.

## Deferred improvements

- [x] Render assistant-message Markdown safely: formatting, tables, highlighted code with
  Copy, https links, images on a click. See [docs/message-markdown](docs/message-markdown/README.md).
- [ ] Evaluate generated shared API types and account-wide event delivery if needed.
- [ ] Revisit the embedded agent runtime only when orchestration requires it.

## Decided against by the user (2026-09-21)

- No automatic startup: MiniClaw is started with `miniclaw` in a terminal.
- The guest password is typed after every VM start: no guest auto-login, and MiniClaw never
  handles that password.
- No Apple Account or iCloud inside the VM.
- The two Go advisories in the network helper stay as they are until gvisor-tap-vsock is next
  updated; no reachable vulnerability was found.

## Removed or superseded, not open work

These were open items in earlier versions of this list. They describe code or plans that no
longer exist.

- **Recovery deployment to the Linux controller (v12 to v17 builds, seed ISOs).** The
  controller was removed; the backend is one process on the Mac. The creation-recovery
  logic itself remains and has one open verification under Tools and permissions.
- **Scoped execution through a shared Standard macOS account, TextEdit as the vetted app,
  local app review in settings, machine-bound app catalog.** Replaced by the Seatbelt
  executor. GUI apps are reached only through Computer Use; AppleScript and TextEdit
  operations are refused by the tool.
- **"Host executor through the VM-backed controller".** The Mac is no longer reached
  through a VM; Mac chats run the tool directly under Seatbelt.
- **"Finish supported VM combinations beyond the Linux-plus-Master pair" and "Expose
  host-side status even when the Linux API cannot run".** There is no Linux guest.
- **"Verify normal prepared startup remains below 126 seconds".** That measured the paired
  boot. MiniClaw now starts without a VM; VM start time is a separate open measurement.
- **"Preserve `codex-docs/fresh-install-lab` as the primary installation".** The primary
  installation is `~/MiniClaw-VM`. The user moved the lab disks to the Trash on 2026-09-20,
  so TESTING.md results that name those guests are historical.
- **"Add automatic failed-Codex-child recovery in Host mode".** Done by the provider
  supervisor, listed under Installation.
- **Guest shutdown hang and the 30-second force-off policy.** Cause found and fixed on
  2026-09-20: macOS answers a stop request with a 60-second confirmation dialog. The
  supervisor now confirms the dialog and allows 90 seconds. The user reports the issue
  fixed; the shutdown time itself was not measured.
