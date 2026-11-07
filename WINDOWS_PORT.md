# Windows port

Branch: `windows-port`. Baseline: `20595a9`, copied from the supplied source archive.
The archive had no Git repository or remote. This repository is isolated from the installed
Mac runtime and retains the current Mac source as its baseline.

## Required outcome

Native Windows MiniClaw with normal chat, providers, agent delegation and approvals,
MCP tools, optional Blender/Roblox/ElevenLabs setup, protected secret storage, file tools,
terminal, timed unsandboxed shell and computer use. Windows VM support is excluded by
the user's instruction. Existing Mac behavior must remain intact.

## Execution and review

Host-runnable changes use red-to-green tests. Windows-only tests are running in the native ARM64 guest; passing suites and
remaining failures are recorded below. Platform-specific behavior uses Microsoft, Rust,
provider and tool-maintainer documentation. Independent subagents own setup, filesystem/secrets,
and provider transport. The lead owns integration, executor/sandbox/terminal/computer
work and acceptance. Completed areas receive a review from an agent other than their
author before being considered ready. Keep commits local until publishing is authorized.

## Acceptance gates

- [x] Windows GNU target passes `cargo check --all-targets`, including native test code.
- [x] Windows ARM64 MSVC executable builds and runs on Windows.
- [ ] Host chat and both provider transports work on Windows.
- [x] Installer handles spaces, Unicode, drive letters and executable wrappers. (ARM64; x64 not run)
- [x] Native secret storage and private state ACLs pass isolation tests. (ARM64; x64 not run)
- [x] File approvals resist reparse points, alternate streams and path aliases. (ARM64; x64 not run)
- [x] Sandboxed execution enforces file and network restrictions. Never fall back to unrestricted. (ARM64; x64 not run)
- [x] Timed unrestricted commands stop their child process tree on cancellation or expiry.
- [x] Interactive terminal and native computer use work with approval controls. (terminal yes, ARM64; native computer use not exercised with real actions)
- [x] Optional tool bundle connects to actual Blender and Roblox Studio. (ARM64, 2026-10-06: Blender GLB export through a MiniClaw prompt; Roblox Part created through a MiniClaw prompt with the Roblox server in unsandboxed mode, confirmed by the user)
- [ ] Windows installer, restart, recovery and update pass on a fresh Windows account.
- [x] Existing Mac Rust suite and affected frontend/backend contracts pass.
- [x] Independent source-review findings resolved and regression evidence recorded. (docs/windows/review-*.md; 0.1.3 review findings fixed in 0.1.4)
- [x] Native security and lifecycle review validated on Windows. (review-release-0.1.3.md plus native acceptance 354/0 and standard user 105/0 on 0.1.4, ARM64)

## Status update (2026-10-05)

The Windows sandbox gate is open on ARM64 behind real preconditions (`server/src/windows_gate.rs`).
Evidence on the second test guest (Windows 11 ARM64, UAC on): native acceptance exits 0 with 321
tests (227 as owner, 94 as a standard user), all 51 Node suites pass, and the release product
check through the HTTP API passes 15 of 15 including hard-kill recovery without a Windows restart
(`../work/windows-vm/gate2-acceptance-vm2-7.log`, `gate2-js-vm2-full-2.log`, `gate2-js-vm2-fix-1.log`,
`gate2-release-e2e-vm2-1.log`, `gate2-e2e-stopped-vm2-1.log`). With the network service stopped,
every sandboxed entry point refuses with a reason and nothing runs unrestricted.

Works inside the sandbox: shell, terminal, file tools, bundled patched Git including HTTPS,
MiniClaw's private Node (official build plus an OpenSSL seed provider), Python without asyncio.
Does not work yet: Python asyncio servers, `npx`/`npm` (both in progress), declared listeners,
x64 programs under ARM64 emulation. Windows x64 builds and passes host checks but has not run
natively in the sandbox. Live provider chat, real computer-use actions, installer acceptance on a
fresh account, Roblox Studio connection and an independent review of the newest sandbox changes
remain open. The sections below are older and partly superseded; see `docs/windows/README.md`.

## Release 0.1.7 (2026-10-11)

Published at https://github.com/Sep-Fading/miniclaw-windows/releases/tag/v0.1.7 from `373c448`. Fixes
the chat ordering bug (activity and delegation cards stayed at the bottom after Stop, Send now or on long
conversations) with its review follow-ups. VM2 updated from 0.1.6: start check 8/8, installed e2e 28/28,
live Claude 3/3, live Stop-then-send order check in a real browser passed. Summary:
`../work/windows-vm/release-017/SUMMARY.md`.

## Release 0.1.6 (2026-10-07)

Published at https://github.com/Sep-Fading/miniclaw-windows/releases/tag/v0.1.6 from `e7df45a`
(GitHub `main` = `39d193e`). Adds: Claude sign-in that completes automatically on Windows with a link
and paste-code fallback; the selection legend for multi-choice setup questions; `miniclaw bundle add`
and setup reruns that add Sep's bundle to an existing installation; the packager fetching the patched
Git from the release assets; uninstall removing a MiniClaw-created tools folder. VM2 updated from
0.1.5: start check 8/8, installed e2e 28/28, live Claude 3/3, sign-in API 14/14 (no sign-in
completed). Summary: `../work/windows-vm/release-016/SUMMARY.md`.

## Release 0.1.4 (2026-10-06, 04:45)

`../outputs/windows-0.1.4-feeeb84/` holds the ARM64 and x64 packages built from `feeeb84`. Full native
acceptance on VM1: 354 passed, 0 failed (standard-user sequence 105/0 inside it). VM2 updated from
0.1.3 to 0.1.4 with `miniclaw update`; start check 8/8, installed e2e 26/26, live Claude chat 5/5.
Independent review of 0.1.3 (`docs/windows/review-release-0.1.3.md`): acceptable for the owner's
use; its two low findings are fixed in 0.1.4. Still open: x64 never executed; a real UAC consent
click; a fresh Windows account install; Codex chat (usage limit until 2026-10-11); computer use with
real actions; the Roblox half of the final test (Studio sign-in, unsandboxed consent on ARM64).
Summary: `../work/windows-vm/final-verify/SUMMARY.md`.

## Release status (2026-10-06, morning)

Current build: `../outputs/windows-0.1.3-659458d-jf3/` (ARM64). VM2 runs it after `miniclaw update`
from the user's own click-through install; start check 8/8. The user's final acceptance test
(Blender + Roblox through MiniClaw prompts) is half done: the Blender cube export to a GLB on the
Desktop passed with one prompt; the Roblox part is blocked on Studio sign-in and on Studio being
x64 (unsandboxed mode needed on ARM64). See `../work/windows-vm/final-test/REPORT.md` and the
0.1.3 release notes.

## Release status (2026-10-05, evening)

An installable ARM64 release built from `cf7b301` is in `../outputs/windows-0.1.0-cf7b301/` with an
x64 package (never run), checksums, Git corresponding-source artifacts and release notes. On the
second test guest (UAC on) the shipped wizard installs it in under a minute; start check 8/8;
installed sandbox check 20/21 (only paging Git commands in the terminal fail: "cannot spawn less");
live Claude chat including an approved sandboxed command and a private-Node turn; resume, repair,
update 0.1.0 to 0.1.1, forced failed update with full rollback, and uninstall with and without
data all pass. Logs: `../work/windows-vm/release-*-cf7b301-*.log`. Automated installs ran from an
elevated task, so the real UAC consent click is still a manual check. A fresh Windows account,
Codex chat, real computer-use actions and x64 remain unverified.

## Available validation

The host is macOS. Windows 11 Pro ARM64 is installed in the UTM test guest, with
Guest Tools and Visual Studio 2026 Build Tools installed. The bootstrap ran under the
actual guest account S. Node 24.21.0 ARM64, Git 2.56.0 and Python 3.13.15 were validated.
Rust 1.99.0 built the backend, provider fixture and all test executables with native
ARM64 MSVC. Native agents (8), filesystem (8), private setup file (1), and secret
storage (3) tests passed. Broker authentication and restart passed after the pipe
access fix. The sandbox now uses a pinned BusyBox shell after native PowerShell
startup required access outside the intended policy. Native shell file boundaries,
persistent terminal state and all three inherited-ACL checks now pass.
Git and network acceptance remain pending. Interrupted-reboot recovery and all five
timed unrestricted shell tests pass. Timed unrestricted shell uses PowerShell.
Cross-compilation and host-run tests can provide partial evidence only. A Windows CI workflow
is prepared, but there is no configured Git remote/runner here and no workflow execution
is implied. Native sandbox, PowerShell, desktop integration and actual app tests remain
required. Do not describe an unexecuted Windows build as a working Windows release.

## Current status

Native integration is in progress with three subagents and independent reviews.
See [implementation and blockers](docs/windows/README.md), [storage review](docs/windows/review-setup-storage.md),
[process review](docs/windows/review-processes.md), and [computer-use integration](docs/windows/computer-use.md).
No Mac runtime changes, real credential reads or real provider/tool calls were made.

Windows runtime support remains unverified. The source now includes a handle-relative
file broker, LPAC execution, an authenticated WFP service, sandboxed stdio MCP, ConPTY
and the native computer-helper protocol. Native acceptance still gates availability.
The installer preserves restrictive defaults. Native process lifecycle tests passed
with the pinned ConPTY runtime. The latest shell integration passes its native
persistent-terminal check. Git, network, standard-user and installed-release checks
still gate release readiness. The native provider/chat suite passed57 cases
with no skips, including actual Codex against a local test provider. These results
do not establish authenticated live provider use or native desktop control.

Local GNU cross-linking did not produce a release executable: Zig 0.15.2 could compile
C dependencies and check all Rust targets, but its runtime libraries did not satisfy
Rust GNU's MSVCRT link; an older Zig attempt lacked a header required by AWS-LC.
No dependency security checks or cryptographic sources were weakened to force a build.
The Windows ARM64 guest completed its user-context MSVC build and produced a portable
release with bundled Node. That package predates the latest shell and permission
changes; the final package must be rebuilt and pass installation/update acceptance.
CI has not run.

Latest host Rust regression passed 264 tests (one opt-in real-provider test ignored),
including the current portable shell and literal Git argument contracts. Doctests
also passed after selecting the matching Rust toolchain.
The frontend passed 102 tests with zero Svelte errors or warnings. Portable release
tests passed 10 tests with one Windows-only launcher test skipped. Native Windows testing is in progress. The Blender 4.5.9 ARM64 export and MCP bridge
staging roundtrip passed with x64 Python 3.13.16. Studio, GUI, provider authentication
and full installation acceptance remain open. Checkpoints are recorded in MEMORY.md.


## Installation foundation checkpoint (2026-09-30)

This checkpoint predates the LPAC/WFP implementation, protected service updater,
interrupted setup recovery, ARM64 portable package and explicit MCP runtime grants.
It records the initial checks rather than the current feature availability.

The installer and package builder now select Windows x64 paths and `.exe` names,
resolve native provider executables, use LocalAppData, omit the VM prompts, and
reject a Mac prebuilt bundle even on the same CPU. Windows state initialization
calls `--secure-data-dir` before writing provider settings. The review explicitly
states that sandboxed shell and MCP execution are unavailable; setup does not
convert them to unrestricted execution.

The optional bundle uses Windows virtual-environment entry points. Its guide
prints PowerShell literal arguments and asks for the actual Studio Plugins Folder.
Roblox's batch entry point is a static relative command with its installation
folder supplied as cwd. The bridge uses Python's OS temporary directory. An update
tries replacing the backend before changing page assets, so a locked Windows
executable produces a stop-and-retry error.

Final setup verification on macOS: 127 tests passed. Two native
Windows tests (batch launch with difficult paths and locked executable replacement)
are present but unexecuted here. The Windows workflow also includes native storage,
permission, credential, skill-import and excluded-worker tests. No Git remote is
configured and the workflow has not run. The installer acceptance gate remains
unchecked until installation, restart and recovery run on a Windows account.
