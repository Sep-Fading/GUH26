# Independent security review — release 0.1.3 (Windows port)

Scope: `git diff 68fd5ea HEAD -- server/ scripts/ web/src openssl-seed/` (68fd5ea = gate opened and
verified; HEAD = da666cf, release 0.1.3). Read-only review by code reading and official docs; no repo
edits, no guest runs. Earlier resolved reviews (`review-*.md`, the pre-2026-10-05 parts of
`sandbox-review.md`) are not repeated. Verdicts cite `file:line` at HEAD.

**Overall verdict:** Acceptable for the user's own use. I found no confirmed way for a sandboxed
process to reach a file, registry key, pipe, port or process it could not reach at 68fd5ea. The new
isolation machinery (grant pre-walk, unmanageable-item refusal, degraded start, "Unreadable"
retirement, private Node/npm/Python, child-pipes preload, OpenSSL seed, asyncio named pipes, Winsock
capability, uninstall/repair) is fail-closed where it matters and the central design risks are
reasoned through in `sandbox-review.md`. The findings below are all low / informational; none blocks
the user's own use. **Before anyone else uses it**, the two LOW items worth closing are F-1
(`MINICLAW_DOCUMENTS_DIR` is a production env override, not test-only) and F-2 (cross-volume folder
"move to Trash" silently keeps the folder) — both are robustness/clarity, not isolation holes. The
Python P1 / `sh.exe` priming and the libuv child-pipes limitation remain correctness caveats carried
over from the design docs, not regressions.

Most severe first.

---

## F-1 (LOW, confirmed) — `MINICLAW_DOCUMENTS_DIR` is honoured in production, documented as test-only

`server/src/chat_folder.rs:21-23` `root()` reads `MINICLAW_DOCUMENTS_DIR` unconditionally (not under
`#[cfg(test)]`), yet its own doc comment and the struct comment say "tests use it so they never touch
the user's own." The backend will therefore redirect every per-conversation Documents folder to a
caller-chosen location whenever that variable is present in the backend's environment.

Failure scenario: anything that can set the backend process's environment (a launcher script, a
machine-wide `setx`, a compromised shortcut) relocates where conversation folders — which become
writable sandbox grants — are created, outside `Documents\MiniClaw`. A sandboxed process cannot set
the backend's environment, so this is not a sandbox escape; the blast radius is "whoever controls the
backend's env," who already controls the backend. Still, a production code path that moves a security-
relevant root should not be a silently-honoured, undocumented env var.

Severity: LOW. Suggested fix: gate the override behind `#[cfg(test)]` (or a dedicated test-only build
cfg), or rename it and document it as a supported, deliberate relocation knob. The same applies to
`MINICLAW_TRASH_DIR` (`server/src/api.rs:3826`), read first in production.

Confidence: confirmed by reading.

Status (2026-10-06, review-fixes): fixed. `chat_folder::root` reads `MINICLAW_DOCUMENTS_DIR` only
when built with `cfg(any(test, debug_assertions))`; the harness runs the debug backend, and the
installed-build e2e already asks for the real Documents folder. `MINICLAW_TRASH_DIR` predates the
port on the Mac, is used by Mac acceptance tests and by the installed-build e2e (a release build),
so it stays in every build and is now listed in README.

---

## F-2 (LOW, plausible) — deleting a conversation "to Trash" across volumes keeps the folder after the conversation is gone

`server/src/api.rs:2663-2675` deletes the conversation first (`remove_conversation`) and only then
moves its folder to the trash via `chat_folders.remove(&id, trash)`. On Windows the trash is
`<data>\Recovered files` (`api.rs:3828`); the conversation folder is under `Documents\MiniClaw`
(`chat_folder.rs`). `Store::remove` (`chat_folder.rs:829-851`) moves with a single `std::fs::rename`,
which fails across volumes (ERROR_NOT_SAME_DEVICE). When Documents is redirected to OneDrive on a
different volume than LOCALAPPDATA, the rename errors, `delete_chat` returns `{folderKept:true}`
(`api.rs:2674-2675`), and the user who ticked "also move the folder to the Trash" is left with the
folder still on disk — while the conversation and its record are already deleted, so the UI no longer
offers to remove it.

This is a data-hygiene / UX defect, not an isolation issue (nothing is over-exposed; the leftover
folder keeps the user's own ACL). The pre-check in `delete_chat` (`api.rs:2665-2668`) only guards the
"no trash at all" case, not a cross-volume rename failure.

Severity: LOW. Suggested fix: fall back to a copy-then-remove (or `MoveFileEx`-style) when `rename`
fails with a cross-device error, or surface `folderKept` to the user with the path so they can delete
it themselves.

Confidence: plausible (rename semantics confirmed; cross-volume Documents is the common OneDrive case,
not exercised natively in the evidence, which stayed under `%TEMP%`/home — see `sandbox-review.md`
"Not covered").

Status (2026-10-06, review-fixes): fixed on the host. A rename that fails with EXDEV or
ERROR_NOT_SAME_DEVICE falls back to a bounded copy (20,000 entries, 1 GiB, 64 levels) that opens
every file without following links and refuses any link, then removes the folder; any failure
removes the half copy and keeps the folder. A kept folder is answered as
`{ folderKept: true, folder }` and the page says where it was kept. Not yet run on a real second
volume.

---

## F-3 (INFORMATIONAL, confirmed) — "Unreadable" retirement trusts the dead-package-SID argument for an un-revoked subtree

`windows_sandbox_journal.rs:648-760`, `windows_sandbox_revoke.rs:473-603`,
`windows_sandbox_retirement.rs:54-130`: when a writable root's descendant cannot be opened for ACL
access (error 5) and MiniClaw never granted it (`discovered`), recovery stops trying to revoke it,
records it as `{"kind":"Unreadable","error":5}` in a v2 certificate, and retires the record after the
job is proven stopped and the profile deleted. A directory recorded this way stands for its whole
subtree, which the walk could not enumerate (`windows_sandbox.rs`/revoke comment), so any inherited
run ACE on children below it is never removed.

The soundness argument (sandbox-review.md "Recovery: what an object nobody granted can carry") is: the
only way such an object carries the run's entry is inheritance of the root's inheritable ACE (case 2),
the entry names the run's AppContainer package SID, the profile is deleted and the name reserved in
`retired/` so no MiniClaw run reuses it, and an AppContainer SID never grants more than the token's own
user already has. I attempted the counterexamples the brief names and could not break it:

- Ownership change mid-run / elevated creator: covered by case 2 — the object was made by another
  account or an elevated program, i.e. the already-accepted trusted-host boundary.
- A sandboxed program creating an Administrators-only object whose ACE is inherited while the later
  walk cannot open it: this is exactly the retired case. The leftover ACE is a deny-nothing allow on a
  SID no principal can usefully assume beyond its own user rights; it is inert, as the "Retained
  unknown identities" argument already established.
- Junction/hardlink tricks: the walk refuses links (`is_link`), `identify_child`
  (`windows_sandbox_revoke.rs:437-475`) opens with `FILE_OPEN_REPARSE_POINT` and `FILE_READ_ATTRIBUTES`
  only (no `SYNCHRONIZE`, no traversal of a reparse target), and the validator accepts `Unreadable`
  only for a `discovered`, non-root entry the record already holds (`windows_sandbox_retirement.rs:
  101-120`), re-validated every start. Tampering (naming a granted object, or version 1) is rejected
  (`unreadable_descendants_retire_and_unreadable_grants_stay`).
- A *granted* object that becomes unopenable keeps the record forever (fail-closed): `recover_object`
  returns an error for a non-`discovered` identity (`windows_sandbox_journal.rs:555-557,772-781`).

Residual risk: the inertness rests on "an AppContainer package SID never exceeds its user's rights"
and on the name reservation holding across the installation's lifetime. Both are Windows-documented and
MiniClaw-enforced, but the un-revoked subtree is a genuine, accepted trade against a denial-of-service
(a sandboxed program could otherwise wedge startup forever). I judge the argument sound; recording it
here because it is the single most load-bearing new isolation claim.

Severity: informational (accepted design risk). Suggested follow-up (not required): when retiring an
`Unreadable` directory, also record its depth/attributes so a future Windows build can re-attempt the
subtree if the object becomes openable again.

Confidence: confirmed by reading; inertness argument plausible (rests on documented SID semantics).

---

## F-4 (INFORMATIONAL, confirmed) — runtime read grants are now exempt from the protected roots

`windows_sandbox.rs:305-317` (`runtime_refused`) and `windows_sandbox_native.rs:161-167,201-207,
242-248`: granting MiniClaw's own `bin\node`, `node_modules\npm` and `bin\python` read-only is now
refused only by the *run's own* blocked rules, no longer by the protected roots. Previously the
installation's protected root (`<install>`, which contains `runtime\bin`) matched and refused every
installed-build launch (the VM2 "every sandbox launch failed with PermissionDenied" bug).

This is safe as written: only the specific pinned runtime subfolders are granted, via
`guarded_acl_path(node.folder/…)`, read-only, hash/byte-verified before use
(`windows_runtimes.rs:load_private_node` etc.); the sibling `data` folder (credentials, state) under
the same protected root is not granted. A run rule that blocks `<install>` or a path inside the runtime
still refuses (`runtime_refused` checks containment both directions;
`installed_runtimes_are_refused_by_the_runs_rules_not_by_the_protected_roots`). No credential or data
path is reachable through this exemption.

Severity: informational. No fix needed. Confidence: confirmed by reading.

---

## Areas reviewed and found sound

- **Private Node folder and npm-tree grants** (`windows_sandbox_native.rs:150-258`): read-only,
  non-inheritable, link-refusing; `bin\node` grants the folder + its own files only, npm's tree is
  granted only when a run uses npm. `program_for` swaps in the private `node.exe`; the user's Node is
  never started in the sandbox. Sound.
- **npm per-server private folders** `<data>\mcp-npm\<id>` (`mcp_npm.rs:51`, `npm_launch.rs:268-271`,
  `windows_sandbox_native.rs:260-270` `grant_owned`): `server_folder` validates `id` as
  non-empty, ≤64, `[A-Za-z0-9-]` only — path-safe, cannot escape `mcp-npm`. The folder is under
  MiniClaw's protected data root, which no request-supplied grant can name; the rest of `data` stays
  denied. Read-write only for the install, read-only for the run. Sound.
- **NODE_OPTIONS `--require` preload and PYTHONPATH/sitecustomize injection**
  (`windows_process.rs:160-178,731-765`, `windows_runtimes.rs:117-135` `node_environment`): the naming
  variables (`MINICLAW_NODE_PRELOAD`, `PYTHON_HOOK_FOLDER`) are stripped from every emitted block and
  the flags are added only for `sandboxed==true`. They never reach an unrestricted process. Both hook
  files are pinned byte-for-byte as part of the private Node / Python check and are read-only to the
  sandbox, so a sandboxed program cannot alter them; influencing them for a later, differently-granted
  run would require writing them, which the ACL forbids. Inheritance by a sandboxed child stays inside
  the same AppContainer and the preload is inert on a fixed libuv or off-Windows. The `--require`
  path is escaped for `NODE_OPTIONS`' backslash-as-escape rule. Sound.
- **OpenSSL seed provider** (`openssl-seed/src/lib.rs`, `openssl.cnf`): `nodejs_conf`-only config, so
  Git/Python (which read `openssl_conf`) ignore it; `OPENSSL_CONF`/`OPENSSL_MODULES` are set only in
  sandboxed launch blocks and never exported to the user's own Node. The provider only answers seed
  requests with `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)` — the same system CSPRNG OpenSSL
  itself would use — so even a hypothetical leak to a non-sandboxed Node weakens nothing. `fill`
  checks the status, `seed_get_seed`/`seed_clear_seed` wipe with `write_volatile`, provider refuses a
  parent, reports strength 256. Sound.
- **asyncio named pipes in `\\.\pipe\LOCAL\`** (`scripts/setup/python-sandbox/sitecustomize.py:
  24-38`): per-pipe name = pid + 128 bits `os.urandom`, `FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance;
  the reading end is created first so a squatter fails creation rather than intercepting. `LOCAL\`
  places the pipe under the run's package SID namespace, unreachable by other runs (measured in
  `sandbox-review.md`). The hook runs only with `MINICLAW_PYTHON_SANDBOX=1` on Windows and chains the
  hidden `sitecustomize`. Sound.
- **Python servers always networked with an empty allowlist + lease ordering**
  (`mcp_client.rs:328-360,395-452`): `enable_network` (capabilities) is applied before process
  creation and `connect_ports` (WFP lease) is awaited before `pending.resume()`; a lease failure
  returns before the suspended process is ever resumed, and the suspended process is terminated by its
  job on drop (`windows_process.rs` `Job`/`PendingStdio`). So a race or failure of the lease install
  cannot leave a resumed `internetClient` process without its WFP confinement — fail-closed. Sound
  (confirmed by call-site ordering).
- **"Offline" runs carrying Winsock-read** (`windows_runtimes.rs` `token_capabilities`,
  `sandbox-review.md` N1): the offline token gains read to the 29 Winsock catalog keys only; socket
  creation still fails 10047, name lookup 11003, measured. Sound.
- **Service Repair request + `winsock-acl-remove` start parameter**
  (`windows_network_broker_native.rs:145-192,571-581,745-751`): the service pipe DACL grants only
  SYSTEM and `owner_sid`; `peer()` rejects any token whose user ≠ owner or that is an AppContainer;
  the Repair frame is signed with the shared secret over the challenge. `service_main` accepts
  `winsock-acl-remove` only as an SCM start parameter, and starting a service requires admin/SYSTEM. A
  sandboxed or non-owner process can trigger neither. Sound.
- **Degraded start / recovery latch** (`main.rs:130-146`, `windows_gate.rs:46-99,241`,
  `windows_sandbox.rs:337-345`, `windows_sandbox_journal.rs:50-77`): a kept recovery record no longer
  aborts the backend; the gate returns `Missing::Recovery` first (after only `SwitchedOff`), so
  `seatbelt::available()` is false and every sandboxed entry point (shell, terminal, Git, MCP,
  `/api/sandbox`) is refused, while chat keeps serving. Independently, each launch re-runs admission in
  `Journal::create` (`recover_active`) which fails closed regardless of the 2-second gate cache. The
  latch self-heals (set to `None` on a later successful admission and after `/api/sandbox/repair` →
  `recover()` + `forget()`). No sandboxed path runs while recovery is pending. Sound.
- **Grant pre-walk TOCTOU** (`windows_sandbox_native.rs:319-408`): every requested grant is opened and
  queued before the profile, record or any ACE exists; the root's inheritable ACE is written with the
  first flushed batch. A file created inside a granted folder during the walk is neither stamped nor
  inherited, so the program *loses* access to it — fail-safe, nothing widened. Refusal while walking
  leaves no profile/record/entry (`Drop` early-returns when `journal`/`sid` are unset,
  `windows_sandbox_native.rs:685-690`). Sound.
- **Unmanageable-item refusal** (`windows_sandbox.rs:349-361` `unmanageable`,
  `windows_sandbox_native.rs:768-772` `managed`): an access-denied open of any granted object aborts
  the launch before anything is recorded, with the path named; a read-only grant is refused the same
  way (no entryless read mode for an LPAC token). Callers surface the words (terminal 409, MCP
  `unprepared`, shell/Git). Sound.
- **Git lpac5 console-handle patch** (`docs/windows/git-patch-review.md` third patch): one
  `DuplicateHandle(..., bInheritHandle=FALSE, DUPLICATE_SAME_ACCESS)` per process, closed by the one
  `atexit` handler, never placed in a `STARTUPINFO`, never inherited; children keep the standard
  handles unchanged. The patch reproduces the archive byte-for-byte. No handle-inheritance change
  reaches the sandbox. Sound.
- **GIT_PAGER / GIT_TERMINAL_PROMPT / GIT_CEILING_DIRECTORIES** (`windows_git.rs:13-23,55-56`): the
  policy pair is appended after the fixed config env; `GIT_PAGER=cat` and `GIT_TERMINAL_PROMPT=0` are
  overridable by the run's own command but no allowlisted inherited variable carries a pager/askpass
  in. Sound.
- **`DIE_ON_UNHANDLED_EXCEPTION` job flag** (`windows_process.rs:39-45`): added alongside
  `KILL_ON_JOB_CLOSE`; makes a crashed/strict-handle-faulting sandboxed process die promptly instead
  of hanging in WER. Narrows nothing. Sound.
- **Roblox bundle `cmd.exe /d /c <path>\mcp.bat`** (`scripts/setup/roblox-bundle.mjs:638-663`): the
  whole command line is fixed in the saved server definition; MCP tool calls travel over stdio and
  never reach a command line, so `cmd.exe`'s re-parse has no model- or user-text input. A
  `<LOCALAPPDATA>` path containing any `cmd.exe` metacharacter (`cmdSpecial` incl. `&`, `%`, `^`, `|`)
  falls back to the working-folder `.\mcp.bat` form, so a user whose profile path holds `&` does not
  get it re-parsed by `cmd.exe`. The server also declares a listening port, so it is refused before any
  sandbox is prepared (`mcp_client.rs:264-281` `declared_listener`) — it never actually runs in the
  sandbox. The `.exe`-only rule is not bypassed because the native image is `cmd.exe` and the batch
  file's `%*` receives nothing. Sound.
- **Uninstall / repair / update elevated scripts** (`scripts/setup/windows-network-remove.ps1`,
  `windows-network.mjs:47-59,181-191,1273-1289`, `uninstall-windows.mjs`, `update-runtime.mjs`): the
  removal command passes only the validated service id (`^[a-f0-9]{32}$`) and canonical owner SID as
  literal args; `removeWindowsNetwork` first checks the caller is the owner SID; the PS script
  re-validates admin role, that the service's `PathName`/`StartName`/config match this installation,
  refuses reparse points on both folders, and the backup-semantics deletion opens every entry with
  `FILE_FLAG_OPEN_REPARSE_POINT` and aborts on any reparse point or entry-type change during removal,
  scoped to `%ProgramFiles%\MiniClawNetwork-<id>` and `%ProgramData%\MiniClawNetwork-<id>` (names
  re-checked against the fixed pattern and base). The 30 000-char cap is enforced on the elevated
  outer command (`networkRemovalCommand`), and the encoded payload is a fixed script plus the two
  validated literals. Sound.
- **Kept-data reuse / install-destination logic** (`scripts/setup/destination.mjs:253-299`,
  `install.mjs:130-230`, `windows-install-directory.ps1:115-131`): a pre-existing folder is adopted
  only when it is this user's protected, private, link-free folder holding *only* `data`, whose
  `kept-after-uninstall.json` marker (v1, ≤4 KiB) names this very resolved path and an owner SID equal
  to the current user's (verified by the elevated `-Kept` script printing the live SID). A failed
  reinstall restores exactly what uninstall left (runtime/settings removed, providers restored,
  marker kept). Sound.
- **Mac shared-code paths**: `chat.rs` refusal reasons / cwd injection are gated so the folder cwd
  check is `cfg!(windows)` only (`chat.rs:900-911`) and the Mac sandbox keeps its existing behaviour;
  `chat_folder` on macOS uses `$HOME/Documents` (`chat_folder.rs:746-748`); `ConfirmDelete`/`TaskNav`
  default `deleteFolder=false` and reset the checkbox on open; `briefing`/`seatbelt`/`api` additions
  are additive and the briefing tests assert no `macOS`/`Seatbelt` leakage. `access_report`,
  `keychain_message`, history/workspace wording and the `logging`-default (warn + eprintln instead of
  aborting) are additive. No Mac regression found.
- **Correctness of new unsafe Win32 / async**: `Checked<T>` (`windows_runtimes.rs:19-63`) runs a check
  once with a panic-safe `Finish` drop guard, bounds every waiter with a deadline, and `off_worker`
  moves blocking waits off the multi-thread runtime via `block_in_place`; tested for the "stuck check
  does not wedge other work or its waiter" case. `SHGetKnownFolderPath`/`CoTaskMemFree`
  (`chat_folder.rs:727-742`), `identify_child` (name validated, no traversal, read-attributes only),
  the seed provider's pointer handling, and the journal v2 certificate backward compat (v1 bytes
  unchanged, v2 only when `unreadable` non-empty) are all correct on reading. No unbounded wait or
  untrusted-input panic found; the `explain`/standard-error watcher keeps ≤31 bytes and bounds its
  1-second wait.

---

## Not reached / not verified

- No native execution: all runtime behaviour is taken from `sandbox-review.md` evidence (VM1/VM2,
  ARM64) and code reading. x64 hosts, a real cross-volume OneDrive Documents, and a standard-user
  reinstall-into-kept-data were not exercised (consistent with the docs' own "Not run" notes).
- The undocumented csrss activation-context caching that P1 and `sh.exe` priming rely on
  (`windows_runtimes.rs:587-611`, `windows_sandbox_native.rs:242-246`) is a Microsoft-unsupported
  dependency; correctness, not isolation. Carried caveat, not a regression.
- The libuv 1.52.1 piped-child limitation (child-pipes preload refuses such children) means some
  Node/Python MCP servers do not work sandboxed until a newer Node is pinned. Functionality caveat,
  documented; not an isolation concern.
- I did not re-audit the WFP broker internals, the journal certificate crypto, or the egress proxy
  beyond the 0.1.3 diff; those were covered by earlier resolved reviews and are unchanged here except
  the additive `access_report`/`Repair`/`recovery` plumbing reviewed above.
- Tests: the new Rust/JS tests assert the right invariants at a good altitude (e.g.
  `unreadable_descendants_retire_and_unreadable_grants_stay`, `a_stuck_check_holds_up_neither...`,
  `sandboxed_bad_handle_and_crash_end_promptly`, `path_within` whole-component, `folder_name`
  sanitisation incl. reserved names/traversal/trailing dots). I did not find a test that asserts too
  little for the areas in scope; the main *untested* path is the cross-volume Trash move (F-2).
