# MiniClaw 0.1.8 (2026-10-11)

One fix, for both platforms. The macOS package is listed below; the Windows packages of 0.1.8 are not built yet,
and the fix is in the source.

## A large conversation no longer takes Codex down

A conversation whose saved tool results had grown large (tens of megabytes: screenshots from Roblox Studio or
Blender runs, generated images) could not be opened. Codex answered the history request in one reply bigger than
the largest frame the backend reads (32 MiB), the backend treated the unreadable reply as a lost connection and
stopped Codex, and because the conversation was reopened as soon as Codex was back, this repeated every few
seconds: every Codex chat said "Codex is unavailable. Restart the MiniClaw backend.", and the notifications filled
with "The Codex connection was lost". The conversation itself looked lost.

Now the backend lists a conversation's turns without their items and reads each turn's items in pages of six, four
turns at a time, so no reply grows with the conversation. A reply larger than one frame (now 64 MiB) fails only the
request that waited for it; the connection and Codex carry on. Verified on the Mac against the conversations that
failed: 90 MB, 124 MB and 380 MB of saved history open in 0.5 s, 2 s and 18 s (the last one saves 54 generated
images while loading); Codex stayed connected throughout, and the ordinary conversations open as before.

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.8-macos-arm64.tar.gz` | 63750941 | `6a8698fa72da78995826dbb97b2ec503b1bfaf7a342bfae0e1132aba25a69548` |

To update an existing installation: stop MiniClaw (Ctrl-C in its Terminal window), unpack this release, run its
`./miniclaw update`, then start MiniClaw with this release's `./miniclaw`. Your conversations and settings are kept.

# MiniClaw 0.1.7 for macOS (2026-10-11)

Apple silicon package built on a Mac (macOS 27, Swift 6.4, Rust 1.98.1) from `git archive 373c448` (the source of the
Windows 0.1.7 packages) plus one fix to the VM helper sources that the newer Swift needed (below). Unsigned, like the
September macOS release: the binaries carry only ad-hoc signatures, so the first start may need "Open Anyway" or
`xattr -dr com.apple.quarantine <folder>` (see `READ ME FIRST.txt`).

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.7-macos-arm64.tar.gz` | 63,694,205 | `2f933c6295795a3f7a821339efcc5f903208df027dd847a34e7feeb202d149ae` |

To update an existing installation: stop MiniClaw (Ctrl-C in its Terminal window), unpack this release, run its
`./miniclaw update`, then start MiniClaw with this release's `./miniclaw`. Your conversations and settings are kept.

## What a Mac user notices since the September release (0.1.0)

- **Plain conversations can run commands.** A conversation without a project gets its own folder,
  `~/Documents/MiniClaw/<title> <short id>`, the first time it runs a command or writes a file; its commands run
  there, in the same sandbox. Before, such a command was refused. The folder shows at the top of the conversation.
  macOS may ask once whether the Terminal app that started MiniClaw may use your Documents folder.
- **Deleting a conversation with a folder** asks with a box, unticked, "Also move this conversation's folder to the
  Trash". Without the tick the folder stays.
- **Project commands** that name no folder run in the project folder.
- **A refused step says why** in the conversation's Activity list.
- **Conversation order** (0.1.7): a turn's Activity and delegation cards stay with that turn after Stop, Send now
  and in long conversations.
- **Claude sign-in from MiniClaw** shows "Open the Claude sign-in page", a "Code from Claude" box and Cancel while
  it waits. On the Mac, Claude Code still opens the browser itself; MiniClaw opens nothing.
- **Setup**: says to press Space to choose and Enter to continue before multi-choice questions; lists the Computer
  Use app and Blender it found; installs Claude Code from Anthropic if it is missing; `miniclaw guide [TOPIC]` and
  "Show me how to find this" give step-by-step help; `miniclaw bundle add sep-roblox` / `bundle status` add the
  Roblox bundle to an existing installation. `miniclaw --help` also lists the Windows-only `repair` and
  `uninstall`, marked as such.
- **MCP servers**: "How do I find NAME?" guides, values in a plainly wrong format are refused, and a
  "Read-only runtime folders" field lets a sandboxed server read a Python or Node runtime.
- **Changes view**: discarding a new file says where it was saved.
- Unchanged on purpose: the Mac wording ("Match macOS", "the macOS Keychain", "this Mac", Seatbelt, the AppleScript
  rule), the sandbox, and quiet output: nothing is printed by default; `MINICLAW_LOG=warn` (or error, info, debug,
  trace) prints MiniClaw's own diagnostics, and any other value stops startup with an error.

## Fix in this package

`scripts/vm/host-relay.swift` and `scripts/vm/guest-bridge.swift`: three `[weak self]` captures become
`[weak self = self]`. Swift 6.4 warns about the old form (ImplicitStrongCapture) and the VM helpers are built with
warnings as errors, so the package could not be built; the lifetimes are unchanged. See TESTING.md.

## Verified for this package

- Host checks on the package source: `cargo test --locked` 416 passed, 0 failed, 1 ignored; `npm run check` 0 errors;
  `npm run test:web` 141 passed; `npm run test:setup` 341 passed (14 skipped); the Swift helper tests 20 passed;
  Playwright desktop and mobile 346 passed, 2 skipped. The packaged page is byte-identical to the tested build.
- Package: `package.json` and both manifests say 0.1.7, the backend contains "MiniClaw/0.1.7", the prebuilt file
  list equals the September release's, signatures are ad-hoc only, as before.
- The shipped wizard, run from the unpacked archive into a scratch folder, found the existing Codex and Claude Code
  sign-ins and installed nothing else. On that installation, with the real providers: a plain Claude turn; a
  plain-chat command that made its folder under `~/Documents/MiniClaw`; a project command in the project folder;
  Stop mid-command then a new message, with the stopped Activity above the new messages live and after reload in a
  real browser; Claude sign-in started on a second instance with a separate Claude config (sign-in page in the
  status, opened once by Claude Code, nothing by MiniClaw, cancelled); deleting a conversation with its folder (the
  folder left Documents for the Trash); the sandboxed terminal, Git changes and discard, a sandboxed MCP server; and
  a live Codex turn (about 3 s).
- An existing 0.1.0 installation on that Mac, in daily use, was updated with `./miniclaw update`: it keeps all its
  conversations (Claude and Codex), agents, projects, skills and MCP servers, both sign-ins, and the sandbox.
- A copy of a real 0.1.0 installation's data (made with SQLite's backup command) opened in the 0.1.7 backend with the
  same agents, projects, skills, MCP servers and Claude conversations (same message counts). Codex conversations
  are listed by the installation's chat folder, so the copy, in another folder, did not list them; an in-place
  update keeps that folder.

## Known issues

- The VM was not exercised (none is installed on the test Mac); its helpers were built and their offline tests pass.
- A second-volume Trash and `miniclaw bundle add` were not run on a Mac.
- `miniclaw bundle status` and `bundle add` look for Sep's Roblox bundle in `<installation>-roblox-tools`. An
  installation made by the September release keeps it in `<installation>/bundles/sep-roblox`, so `bundle status`
  reports the folder and its Python as missing although the bundle's MCP servers work. Do not run `bundle add` on
  such an installation until this is fixed; it would make a second copy.
- Right after the update, the first check of one sandboxed MCP server (Settings > MCP servers) did not answer
  within 90 seconds; checking it again worked (17 s). The other five answered at once.

---

# MiniClaw 0.1.7 for Windows (2026-10-11)

Built from branch `windows-port` with `git archive` of the release commit; the GitHub release v0.1.7 names the commit,
lists the packages with their sizes and SHA-256 values, and records what was verified on Windows 11 ARM64.
Bundled Git `miniclaw-git-2.56.0-lpac5` (same bytes as 0.1.5 and 0.1.6). **The x64 package has never been run.**

To update an existing installation: stop MiniClaw (close its window), extract this release, run its
`.\miniclaw.cmd update` (one administrator approval), then start MiniClaw with this release's `.\miniclaw.cmd`.

## Unreleased

- **Delegation limits for the main agent, and up to a day per run.** Settings > Agents > Main agent > Delegation sets how many delegations one message may lead to and each run's time limit, as named agents have; every agent's limit now goes from 1 to 1440 minutes (was 60). A background run gets the full limit; a run you wait for that is still going after 60 minutes continues in the background up to its limit, and its card says so. The agent is told its limits, so it can plan for them.

## Changes since 0.1.6

- **A conversation reads in the order of its turns.** A turn's Activity list (the commands and tools it ran) and
  its delegation cards now always sit with that turn: above its first reply, or under your message when the turn
  has no reply. Before, they could end up at the bottom of the conversation, below newer messages, in three cases:
  - you pressed **Stop** before the agent replied, and a step or a delegated agent finished after your next message;
  - you used **Send now** on a queued message, which stops the running turn, and that turn's step or delegated
    agent reported back after the new message started;
  - you reopened a **long conversation** (more than one page, 50 turns), where every older turn's Activity and run
    cards showed at the bottom until you loaded older messages.
  A late update (a step finishing, a run ending) now changes its card in place and never moves it. A background
  run that is still working stays in view at the bottom even when its turn is on an older page, and moves to its
  turn once that page is loaded. To make this work, each of your messages records which turn it started.
- **Review follow-ups.** Packaging tests no longer pick up `MINICLAW_GIT_ARCHIVE` / `MINICLAW_GIT_SOURCE` from the
  machine they run on, so they pass on a configured release machine too. New tests cover how turns are named, that
  the turn memory is cleared when you switch conversations, and that `uninstall --delete-data` keeps a bundle folder
  that is a link or junction. See `docs/windows/review-release-0.1.7.md`.
- The backend, `package.json` and the release are versioned 0.1.7 in the source; the bundled Git is still
  `miniclaw-git-2.56.0-lpac5`.

## Known issues

- The x64 package is built and host-checked but has never been run; all native sandbox verification ran on
  Windows 11 ARM64. Roblox Studio's MCP server is x64 and runs outside the sandbox on ARM64 only with your consent.
- A turn whose message was never linked to it (very rare: a send whose result MiniClaw never saw) is hidden once it
  ends if the conversation has older history; load older messages to see it.
- Unchanged from 0.1.6: the first confined MCP server check right after an update or install can answer "The server
  did not answer in time." once; the retry works. The Windows sign-in opener brings the browser up without focus
  when another window is in front; the link in MiniClaw opens it in front.

---

# MiniClaw 0.1.6 for Windows (2026-10-07)

Built from branch `windows-port` with `git archive` of the release commit; the GitHub release v0.1.6 names the commit,
lists the packages with their sizes and SHA-256 values, and records what was verified on Windows 11 ARM64.
Bundled Git `miniclaw-git-2.56.0-lpac5` (same bytes as 0.1.5). **The x64 package has never been run.**

To update an existing installation: stop MiniClaw (close its window), extract this release, run its
`.\miniclaw.cmd update` (one administrator approval), then start MiniClaw with this release's `.\miniclaw.cmd`.

## Changes since 0.1.5

- **Claude sign-in finishes by itself on Windows.** Signing in to Claude Code from MiniClaw (Settings >
  Providers, or the first screen) opens Claude's sign-in page in your browser, and MiniClaw shows you as signed
  in once you approve there. MiniClaw now opens the page itself, outside the sign-in process's job, so the
  browser is no longer closed when the sign-in command ends. If the browser does not open, or opens behind
  other windows, the page shows "Open the Claude sign-in page" as a link, a "Code from Claude" box for the
  code Claude's page ends with, and Cancel. While a sign-in waits, MiniClaw checks the sign-in state at most
  once every three seconds. The sign-in address and code are never written to logs. macOS is unchanged.
- **Setup explains multi-choice questions.** Before a question where several rows can be chosen, the wizard
  says to press Space to choose or clear the highlighted row and Enter to continue. In a plain Windows console,
  where the prompt draws ASCII marks, it also says that `[+]` means chosen, `[ ]` not chosen, and `[•]` only
  marks the highlighted row.
- **Add Sep's Roblox Development Bundle to an existing installation.** Stop MiniClaw, then run
  `.\miniclaw.cmd bundle add sep-roblox` in the release folder (or run `.\miniclaw.cmd setup` again and choose
  the bundle). Your conversations, projects, approvals, settings, saved keys and existing agents, skills and MCP
  servers with the same names are kept. Running it again adds only what is missing.
  `.\miniclaw.cmd bundle status` shows which parts of the bundle an installation has. If the bundle fails during a
  reinstall over kept data, MiniClaw stays installed and setup names `miniclaw bundle add sep-roblox` as the retry.
- **Uninstall removes the bundle's tools folder with `--delete-data`.** `.\miniclaw.cmd uninstall --delete-data` now
  also deletes `<installation>-roblox-tools` when MiniClaw made that folder for this installation (recorded in its
  `installed.json` from 0.1.6 on). A folder of that name you made yourself, or one made by 0.1.5 or earlier, is kept
  and named. A plain uninstall keeps the folder and says where it is.
- **Building from a clean clone.** The packager no longer needs local copies of MiniClaw's Git build: when
  `MINICLAW_GIT_ARCHIVE` and `MINICLAW_GIT_SOURCE` are unset it downloads the pinned Git ZIP and its
  corresponding source from the v0.1.5 release and checks their size and SHA-256. While the repository is
  private, set `GH_TOKEN` or `GITHUB_TOKEN` (for example `$env:GH_TOKEN = (gh auth token)`). The local
  variables still work for offline builds. See `docs/windows/git-runtime.md`.
- The backend, `package.json` and the release are versioned 0.1.6 in the source; the bundled Git is still
  `miniclaw-git-2.56.0-lpac5` (same bytes as 0.1.5).

## Known issues

- The x64 package is built and host-checked but has never been run; all native sandbox verification ran on
  Windows 11 ARM64. Roblox Studio's MCP server is x64 and runs outside the sandbox on ARM64 only with your consent.
- Unchanged from 0.1.5: the first confined MCP server check right after an update or install can answer "The server
  did not answer in time." once; the retry works. The setup preview's detection panel names what it found but not
  the paths.
- The Windows sign-in opener brings the browser up without focus when another window is in front; the link in
  MiniClaw opens it in front.

---

# MiniClaw 0.1.5 for Windows (commit 6457c45, 2026-10-06)

Source: commit `6457c45be33b7f61d94aa3903d5ec1f9d814a739` (branch `windows-port`) exported with `git archive 6457c45`,
no local changes (source zip sha256 `a33702f018c6396a7c687fdfea6c50b8f8139127270da561f540ad2467737279`, 817 files).
Bundled Git `miniclaw-git-2.56.0-lpac5`. Rust 1.99.0 / MSVC 14.51.36231. Both packages were built on VM1 (Windows 11
ARM64 26100.9457) with the release build script: ARM64 natively, x64 with the MSVC x64 cross tools
(`work/windows-vm/handoff/build-arm64-0.1.5-vm1-1.log`, `build-x64-0.1.5-vm1-2.log`). **The x64 package has never been run.**

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.5-6457c45-windows-arm64.tar.gz` | 114,140,773 | `25d52501cd1dc6d43531847bebe4023edecb39b9679cbca0346e18cad3ae46ab` |
| `miniclaw-0.1.5-6457c45-windows-arm64-git-corresponding-source.tar.gz` | 154,469,901 | `b3e5fb5bafbcbb459f77d4a487cb0711cd260319bcb2946c294354e04cfdfea0` |
| `miniclaw-0.1.5-6457c45-windows-x64.tar.gz` | 123,054,063 | `b69a4d05a50bbbc54be03ac2c6cfdf20292499ed4cc764d006465d395b366dcc` |
| `miniclaw-0.1.5-6457c45-windows-x64-git-corresponding-source.tar.gz` | 154,469,901 | `8d68b8e1e5917ae4a28f01c592d26c6d0156ffd2d8498cf976ff8f19bb2ec81a` |

Publish each Git corresponding-source archive beside its release archive (GPLv2; `GIT-SOURCE-OFFER.txt` inside the
release names it, distributor contact "Sep"). The `.sha256` files were written on Windows and end lines with CRLF;
on a Mac check them with `tr -d '\r' < FILE.sha256 | shasum -a 256 -c`.

To update an existing installation: stop MiniClaw (close its window), extract this release, run its
`.\miniclaw.cmd update` (one administrator approval), then start MiniClaw with this release's `.\miniclaw.cmd`.

## Changes since 0.1.4

- **Setup guides** (commit f05c454). `miniclaw guide [TOPIC]` prints numbered step-by-step guides for what setup or a
  bundled tool needs: the Computer Use helper, Python for the Roblox bundle, Blender, Roblox Studio's MCP server and
  the bridge plugin, the Roblox asset-upload and developer-products keys, the creator and universe IDs, ElevenLabs,
  and provider sign-in. Each guide exists for macOS and Windows and fills in this computer's real paths. Setup finds
  the Codex Computer Use helper, Blender and Studio's plugins folder by itself and offers them; the wizard shows a
  guide before a question it cannot answer; MCP server settings link to the guide for each missing value.
  `docs/setup-guides.md` is generated from the same guides.
- **The Computer Use helper keeps working after Codex app updates.** The helper's path contains the Codex app's
  version (`WindowsApps\OpenAI.Codex_<version>_...`), so the saved path broke after every Codex update. Setup now
  saves your consent with the Codex app's package family and the helper's file name. MiniClaw looks for the helper in
  the newest installed Codex app when it starts and again before each use, and checks its OpenAI signature again
  (the same Authenticode check as setup) before using it. A path saved by 0.1.4 or earlier is migrated the first time
  MiniClaw starts. When no helper can be used, the Tools menu says why (Codex app not installed, helper missing, or
  no valid OpenAI signature). While the helper is configured, `C:\Program Files\WindowsApps` cannot be granted to tools.
- **Roblox key advice in the bundled tools' READMEs** now matches the setup guides: no `0.0.0.0/0` range (restrict to
  this computer's public IP only if it never changes), no "no expiration date" advice (Roblox also expires keys unused
  for 60 days), and the exact scopes (`asset:read`, `asset:write`; `developer-product:read`, `developer-product:write`,
  restricted to one experience). The bundle manifest hashes are updated.

## Verified for this package

- Host checks on the release commit: Mac `cargo test --locked` 405 passed, 0 failed, 1 ignored; Windows GNU
  `cargo check --all-targets` clean; `npm run test:setup` 319 passed, 0 failed (14 skipped); `npm run check`
  0 errors; `npm run test:web` 130 passed; `test:cli` 13, `test:service` 16, `test:computer` 29, all passed.
- Fresh builds: each source folder was unpacked from the commit's archive with every file hash-checked; the built
  backend of both packages contains strings first added in 6457c45; the x64 target folder was new.
- Package checks: every PE image (8 in the release, 28 in the bundled Git ZIP) has the package's machine (ARM64 0xAA64 /
  x64 0x8664); no Microsoft runtime DLL is shipped; the bundled Git ZIPs equal the lpac5 pins; `package.json` and both
  manifests say 0.1.5; the source offer names "Sep"; the x64 READ ME says x64 is not verified natively.
- VM2: `miniclaw setup --preview` from the unpacked ARM64 release (read-only, run from a test folder) shows
  "Computer Use helper: found in Codex app 26.930.3930.0" and "Blender: 5.1 found".

## Not verified

- On VM2 the user's installation was NOT updated to 0.1.5, and the release start check, the installed-build e2e
  (with the native helper re-resolution checks) and the live Claude chat were NOT run on this package: the step that
  stops the user's running MiniClaw was not permitted in this session. The helper re-resolution therefore has host
  tests and the read-only setup preview only, no run on Windows.
- x64 never executed (built only). Codex chat. Computer use with real actions. Roblox Studio through MiniClaw.

## Known issues

- The setup preview's detection panel names what it found (Codex app version, Blender version) but not the paths, and
  has no row for Studio's plugins folder; the interactive wizard and `miniclaw guide` show the paths.
- Unchanged from 0.1.4: the first confined MCP server check right after an update or install can answer "The server
  did not answer in time." once; the retry works. See the earlier notes below for the remaining known issues.

---

Earlier notes (0.1.4, feeeb84) follow unchanged.

# MiniClaw 0.1.4 for Windows (commit feeeb84, 2026-10-06)

Source: commit `feeeb84f1b4c50139fbf5f8651f389b3680634b4` (branch `windows-port`) exported with `git archive HEAD`, no
local changes (source zip sha256 `fab634105ca8143456c63d067bf2d4e9d766112b5dc32692ed9752de9f3773ec`, 806 files).
This is the first package that contains everything: 0.1.3 plus the fixes from the independent review of 0.1.3
(`docs/windows/review-release-0.1.3.md`). Bundled Git `miniclaw-git-2.56.0-lpac5`. Rust 1.99.0 / MSVC 14.51.36231.

- ARM64: built natively on VM1 (Windows 11 ARM64 26100.9457) with the release build script
  (`work/windows-vm/final-verify/build-arm64-0.1.4-vm1-1.log`).
- x64: built on VM2 (same Windows build, ARM64 host with the MSVC x64 cross tools) with the same script
  (`work/windows-vm/final-verify/build-x64-0.1.4-vm2-1.log`). It was not built on VM1 because VM1 had only 2.65 GB
  free after the native acceptance run (1.62 GB after the ARM64 build). **The x64 package has never been run.**

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.4-feeeb84-windows-arm64.tar.gz` | 114,065,172 | `36ae10aaab60457b7bb2bec4ae07a91125ef96641adf5779a858fc2fe678ca21` |
| `miniclaw-0.1.4-feeeb84-windows-arm64-git-corresponding-source.tar.gz` | 154,469,901 | `66e956f82ebc980ce5f7ad2de49b40136625ff07caa196aa32e9b2b9e4ba83ba` |
| `miniclaw-0.1.4-feeeb84-windows-x64.tar.gz` | 122,969,079 | `9011c992763835993c3a632591f613fbdb3bac97c4b1f5f93ffaf93ee400c240` |
| `miniclaw-0.1.4-feeeb84-windows-x64-git-corresponding-source.tar.gz` | 154,469,901 | `342f763d86cb7801cea4705b5e31ca8392cb41faee2d9dbb3b84e249e1b8f8fe` |

Publish each Git corresponding-source archive beside its release archive (GPLv2; `GIT-SOURCE-OFFER.txt` inside the
release names it, distributor contact "Sep").

Install, repair, update and uninstall work as described in the 0.1.0 notes below. To update an existing installation:
stop MiniClaw (close its window), extract this release, run its `.\miniclaw.cmd update` (one administrator approval),
then start MiniClaw with this release's `.\miniclaw.cmd`.

## Changes since 0.1.3

- **Documents override is test-only (review F-1).** `MINICLAW_DOCUMENTS_DIR` is honoured only by test and debug
  builds. A release build always puts conversation folders in the user's own `Documents\MiniClaw` (they become
  sandbox grants, so a release no longer lets an environment variable move them). `MINICLAW_TRASH_DIR` stays in every
  build and is now documented in the README.
- **Cross-volume Trash fallback (review F-2).** Deleting a conversation with "also move its folder to the Trash" now
  works when Documents and the data folder are on different volumes (for example Documents on OneDrive on another
  drive): a rename that fails with EXDEV / ERROR_NOT_SAME_DEVICE falls back to a bounded copy (20,000 entries, 1 GiB,
  64 levels) that refuses every link and then removes the folder; any failure removes the half copy and keeps the
  folder whole. A kept folder is answered as `{ folderKept: true, folder }` and the page tells the user where it was
  kept. Not yet run on a real second volume.
- **Listening-port refusal is not retryable.** "Check connection" on a sandboxed MCP server that declares listening
  ports (for example `roblox_studio`) answers `retryable: false`, so the page does not offer a retry that cannot work.
  Other connection faults stay retryable.

## Verified for this package

- Native acceptance on VM1 (UAC off, elevated S) from the HEAD tree (hash-verified, changed files stamped, a fresh
  compile of `miniclaw-server`): `scripts/windows-native-acceptance.ps1` unchanged, every runtime input provided,
  exit 0, **354 tests passed, 0 failed** (249 as the elevated owner, 105 as a disposable standard user through
  `scripts/windows-native-standard-user.ps1` with the ACL, network and runtimes tests).
- Package checks: every PE image (8 in the release, 28 in the bundled Git ZIP) has the package's machine (ARM64 0xAA64 /
  x64 0x8664); no Microsoft runtime DLL is shipped; the bundled Git ZIPs equal the lpac5 pins; manifests say 0.1.4;
  the source offer names "Sep".
- On VM2 (UAC on), the user's installation updated from 0.1.3 with `miniclaw update` (data kept, network service
  updated to the new backend), then: release start check 8 of 8, installed-build e2e 26 of 26 (shell, terminal,
  private Node, Git incl. HTTPS through an approved host, Changes and Discard, confined MCP server, restart and hard
  kill recovery, plain-chat Documents folder, Desktop project), live Claude chat (3 prompts: plain turn; project turn
  whose sandboxed command ran in the project folder without naming it; plain chat whose command created its own
  `Documents\MiniClaw\<title id>` folder), and the Settings "Try again" path (`POST /api/sandbox/repair` answered
  ok, sandbox, Git and network stayed available).

## Not verified

- x64 never executed (built only). A real second volume for the Trash fallback. Codex chat. Computer use with real
  actions. Roblox Studio through MiniClaw (x64 Studio cannot run in the ARM64 sandbox; needs the user's sign-in).

## Known issues (unchanged from 0.1.3)

- The first confined MCP server check right after an update or install can answer "The server did not answer in
  time." once; the retry works (seen again in the 0.1.4 start check).
- See the 0.1.3 and 0.1.0 notes below for the remaining known issues.

---

Earlier notes (0.1.3, 659458d + journal-fix) follow unchanged.

# MiniClaw 0.1.3 for Windows ARM64 (659458d + journal-fix, 2026-10-06)

Source: commit `659458d` (branch `windows-port`) exported with `git archive`, plus the 22 files of the journal-fix
working tree listed below (not committed; `work/windows-vm/journal-fix/pkg-changed.txt`, source zip `src-jf3.zip`,
sha256 `6c7adfede07893fcebd2203109a38e6b09041b8a12cb609a67984d2a77a089d8`). Bundled Git `miniclaw-git-2.56.0-lpac5`.
Built natively on VM2 (Windows 11 ARM64 26100.9457, UAC on) with the release agent's build script
(`work/windows-vm/journal-fix/vm2-build-0.1.3-jf3-1.log`). ARM64 only; no x64 package was built.

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.3-659458d-jf3-windows-arm64.tar.gz` | 114,083,646 | `8a2ef0d8ab25352f10cca451f710206e4158a2dd1f21fe169e9f11d24357aac9` |
| `miniclaw-0.1.3-659458d-jf3-windows-arm64-git-corresponding-source.tar.gz` | 154,469,901 | `aab1e3e754952929ad9d4473b7327c4d4dd823ab4e93f73bfeb1c3e09a7b8358` |

Publish the Git corresponding-source archive beside every copy of the release archive (GPLv2).

## Changes since 0.1.2-fa1

- **A stuck sandbox record no longer stops MiniClaw.** If records of an earlier sandboxed run cannot be resolved at
  start, the backend starts anyway: chat and settings work, and only sandboxed work (commands, terminal, Git views,
  sandboxed MCP servers) is paused. Settings > Permissions shows the reason, naming the record and the item, with a
  Try again button. Before, MiniClaw refused to start ("Windows sandbox recovery needs attention").
- **Folders with items you cannot manage are refused before anything changes.** A granted folder that contains an
  item your Windows account cannot manage (for example Administrators-only folders an elevated program left behind)
  is refused with "The folder contains items the current Windows user cannot manage, starting with <path>". No sandbox
  record, profile or permission entry is created. The message reaches MCP Check connection, the terminal, the shell
  and Git views. Read-only grants are refused the same way.
- **Recovery resolves records whose only uncertainty is an item MiniClaw never granted** and this account cannot open:
  once the run is proven stopped and its profile deleted, the record is retired and names the item as unreadable.
  An item MiniClaw did grant and can no longer open still keeps its record (sandboxed work stays paused, with the path).
- **Sandboxed MCP servers with listening ports** are refused before any sandbox is prepared, with the way to run them
  ("Run outside the sandbox" in the server's settings, which only you can turn on). On ARM64 the message adds that
  x64 programs such as Roblox Studio's StudioMCP.exe cannot run inside the sandbox.
- **Roblox bundle (Windows):** `roblox_studio` is registered as `cmd.exe /d /c <LOCALAPPDATA>\Roblox\mcp.bat` with no
  folder, so Studio's folder (its sign-in profile, `mcp.bat`, per-user Studio builds) is never given to a sandbox.
  Setup and SETUP.md say it runs only outside the sandbox on Windows. Existing installations keep the entry they have.
- Also in this build (commits since cf7b301, see their notes): Roblox bundle requires native-architecture Python on
  Windows and a short bundle hint; project folder by default, per-conversation Documents folder, refusal reasons, MCP
  warm-up wait; Git pager policy, runtime warm-up, kept-data reinstall, update messages.

## Verified (VM2, the user's installation, updated from 0.1.2 with `miniclaw update`, data kept)

- The two records left by the 2026-10-06 incident were resolved by MiniClaw's own recovery at the first start
  (retired, 13 unreadable WebView2 folders named in each); no record was deleted by hand and no permission changed.
- Release start check 8 of 8 (start from the release folder, page and assets byte for byte, sandbox/Git/network
  available, providers, confined MCP server on the private Node, headless browser, restart, hard kill and recovery).
- `roblox_studio` Check connection answers the listening-port message and writes no record; a sandboxed server with
  folder `%LOCALAPPDATA%\Roblox` is refused with the unmanageable-item message and writes no record.

## Changed files (over 659458d)

- `docs/mcp-servers/README.md`
- `docs/windows/sandbox-review.md`
- `scripts/setup/bundles/sep-roblox/SETUP.md`
- `scripts/setup/bundles/sep-roblox/manifest.json`
- `scripts/setup/roblox-bundle.mjs`
- `scripts/setup/roblox-guide.mjs`
- `server/src/api.rs`
- `server/src/api/terminal.rs`
- `server/src/main.rs`
- `server/src/mcp_client.rs`
- `server/src/mcp_npm.rs`
- `server/src/windows_gate.rs`
- `server/src/windows_sandbox.rs`
- `server/src/windows_sandbox_journal.rs`
- `server/src/windows_sandbox_native.rs`
- `server/src/windows_sandbox_retirement.rs`
- `server/src/windows_sandbox_revoke.rs`
- `server/tests/windows_acl_inheritance.rs`
- `server/tests/windows_startup.rs`
- `tests/windows-setup.test.mjs`
- `web/src/SandboxAccess.svelte`
- `web/tests/sandbox-access.test.ts`

---

Earlier notes (0.1.0, commit cf7b301) follow unchanged.

# MiniClaw 0.1.0 for Windows (commit cf7b301, 2026-10-05)

Source: commit `cf7b3017f407c7335c3e7b91aa0de71341f06e37` (branch `windows-port`, exported with `git archive`, no
local changes), bundled Git `miniclaw-git-2.56.0-lpac5`. Built natively on the Windows 11 ARM64 test guest VM2 (build
26100.9457, UAC on) with Rust 1.99.0 / MSVC 14.51.36231. Build logs: `work/windows-vm/release-build-arm64-cf7b301-vm2-1.log`,
`release-build-x64-cf7b301-vm2-1.log`.

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `miniclaw-0.1.0-cf7b301-windows-arm64.tar.gz` | 113,977,058 | `eda02e8fea311c754da03d5047ce78eb227a92d7cc596f807c3f4fdc48e0e7b2` |
| `miniclaw-0.1.0-cf7b301-windows-arm64-git-corresponding-source.tar.gz` | 154,469,901 | `5bcd2a5d2ca9c23813efe3cd7808f88831801e79c06ac647483d972cefb92955` |
| `miniclaw-0.1.0-cf7b301-windows-x64.tar.gz` | 122,883,199 | `98bfdc9e1c525a1c43cf78cf3488fe3a5494f16dd10ffb04beef71f7ea43d532` |
| `miniclaw-0.1.0-cf7b301-windows-x64-git-corresponding-source.tar.gz` | 154,469,901 | `c91c92069659e0fc7c41ab91e61ccb87bc927144a7aef79f9263dc88e8a0c196` |

The Git corresponding-source archive must be published beside every copy of its release archive (GPLv2; see
`GIT-SOURCE-OFFER.txt` inside the release, distributor contact "Sep").

## Install

1. Extract the whole `.tar.gz` (Explorer "Extract all", or `tar -xzf`). Keep the extracted folder: its `miniclaw.cmd`
   starts, updates and uninstalls MiniClaw.
2. Open a normal terminal (not "Run as administrator") in that folder and run `.\miniclaw.cmd setup`.
   Press Enter at each question for the defaults (folder `%LOCALAPPDATA%\MiniClaw Install`, port 8787, no bundle,
   no Computer Use helper, Install). A custom folder: `.\miniclaw.cmd setup "D:\Some Folder"`.
3. Windows asks once for administrator approval (consent for "Windows PowerShell") to install the network
   isolation service. A second approval comes only if the Microsoft Visual C++ runtime is missing or too old.
   Answer within 3 minutes (setup's wait for the elevated step times out after 180 s; `miniclaw resume` then finishes it).
4. Start: `.\miniclaw.cmd`, then open http://127.0.0.1:8787. Stop: Ctrl+C or close the window.

Needs Windows 11 24H2 (build 26100) or later and Claude Code or Codex. If Claude Code is missing, setup installs it
from Anthropic by itself; sign in afterwards from MiniClaw. Setup on VM2 took 35-44 s (no download: VC runtime and
Claude Code were already present).

## Repair, update, uninstall

- `.\miniclaw.cmd repair` - restarts the network service, which checks and repairs its Windows access entries (1 approval).
- Update: stop MiniClaw, extract the newer release, run its `.\miniclaw.cmd update` (1 approval). Backend, page and every
  native runtime file (ConPTY, BusyBox, Git, private Node with npm and its seed library, Python hook) are swapped as one
  recovery group; a failure restores every earlier file and the earlier Git runtime.
- `.\miniclaw.cmd uninstall [PATH]` - asks first, refuses while MiniClaw is running, then with 1 approval removes the
  service after it deleted its own Winsock entries, its program and data folders, then `runtime\` and `settings.json`.
  Your `data\` folder stays; `--delete-data` (asked a second time) removes it too.
- `.\miniclaw.cmd resume` - finishes a setup interrupted after the service step; on a finished install it says there is
  nothing to resume.

## What works (verified on the installed ARM64 build, VM2)

- Shipped wizard, default and custom (space + non-ASCII) destinations, release folder with space + non-ASCII path.
- Start with `.\miniclaw.cmd`; page and assets served from the installed `runtime\web`, byte for byte; Microsoft Edge
  (headless) loads the page, Settings opens, no repair banner.
- Sandbox available with Git, network and healthy Winsock access; service runs as LocalSystem with the installed
  backend's exact bytes.
- Live chat with Claude Code (signed in), verified on the installed build of commit 1e66d73 (same sandbox and chat code;
  this package adds the patched Git and the update/resume fixes): plain turn; in a project a sandboxed shell command
  approved through MiniClaw's approval, file written inside the workspace, write outside refused, the private Node runs
  in the same command; history kept across quit and restart. Not repeated on this exact package to stay within the
  agreed number of live prompts.
- Terminal (state across commands, outside refused), `node -e` with crypto/fs on MiniClaw's private Node 24.21.0,
  Git init/add/commit/status, HTTPS `git ls-remote` to an approved host (other hosts refused), Changes + Discard,
  a confined stdio MCP server on the private Node, quit/restart, hard kill and recovery without a Windows restart.
- Repair, update 0.1.0 -> 0.1.1, a forced failed update that rolled back completely, uninstall (keep data and
  `--delete-data`) leaving no service, no Winsock entries for that installation and no program folder.

## What does not work or is not verified

- **x64**: builds and passes host checks; not verified natively in the sandbox (no x64 run here at all).
- Codex: not verified (its account is out of usage until 2026-10-11). Live chat was verified with Claude Code only.
- Python MCP servers built on asyncio, `npx`/`npm` inside the sandbox: work in progress by other agents; not checked
  in this release run.
- Declared listening ports for MCP servers: refused as unsupported.
- Computer use: not verified with real actions.
- x64 programs inside the sandbox on ARM64 Windows do not start.

## Known issues

- The first confined MCP server check right after installing answered "The server did not answer in time." once; the
  retry and every later start worked (first-launch latency).
- `git log`, `git diff` and other paging Git commands typed in the terminal fail with "cannot spawn less" (exit 128):
  the bundled Git has no pager. Workaround: `git --no-pager log` or `git log | cat`. Other Git commands on the terminal
  exit 0 (Git lpac5).
- The page asks for `/api/vms/master/status` on Windows and gets 503 (console noise only).
- A failed update says "The installed backend does not match its release manifest; reinstall MiniClaw" although
  everything was rolled back and nothing needs reinstalling.
- After `uninstall` without `--delete-data`, the kept `data\` folder blocks a new setup into the same folder (setup
  refuses an existing destination); move or delete it first.
- Pressing Ctrl+C in `miniclaw.cmd` may show cmd's "Terminate batch job (Y/N)?" after MiniClaw stopped.
- The release is not code-signed; Windows SmartScreen may warn the first time.
