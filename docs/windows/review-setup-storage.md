# Storage review

Independent review by the setup agent, 2026-09-30. Reviewed `platform_fs.rs`,
`platform_fs_windows.rs`, `secrets_windows.rs`, `permissions.rs`, and the changed
identity, attachment, transcript, settings and transfer call sites. No Windows
machine was available, so this is source review, not native acceptance.

## Findings

### Resolved P1: attachment upload used the Unix-only mode API

At review time, [`upload`](../../server/src/api/attachments.rs) creates incoming
files through `tokio::fs::OpenOptions::mode(0o600)`. That method is Unix-only.
This blocks a Windows build and must be replaced with the platform private-file
primitive, then converted to a Tokio file. Simply removing the mode call on
Windows would skip the new explicit private DACL contract. Reported to the root agent; the filesystem agent subsequently replaced it with
`platform_fs::private_file` and `tokio::fs::File::from_std`. Verified the source
correction after the review. Native execution remains pending.

### Native acceptance remains required

The new integration tests in [`platform_fs`](../../server/tests/platform_fs.rs)
and [`secrets`](../../server/tests/secrets.rs) must run on Windows. The workflow
now names both tests explicitly, because `cargo test --bin miniclaw-server` does
not run integration tests. In particular, successful cross-compilation does not
verify inherited DACL handling, sharing violations, junction refusal, file-case
flags, normalized long names or Credential Manager round trips.

No further definite defect was established in the reviewed storage changes.

## Checks and scope

The native file opener checks every ancestor while holding directory handles,
rejects reparse points and multi-link regular files, and checks metadata on the
opened handle. The parent guards last through the leaf open. Callers performing
later path-based operations do not retain those guards, so this is not a claim
that arbitrary multi-operation transactions resist every concurrent path change.
Before enabling a Windows sandbox, exercise directory-replacement races through
the actual tool routes, including final rename operations.

Private creation supplies an owner-only protected DACL at creation time. Existing
private files are checked before truncation. Existing broad ACLs are refused,
not silently rewritten. The macOS path preserves Unix ownership, link-count,
mode and no-follow checks. Several atomic-save callers explicitly drop writable
handles before rename, matching Windows sharing requirements.

Permission keys now normalize existing paths from open handles, reject reparse
points and case-sensitive directories, and allow only ASCII suffixes for missing
paths. The owner added this while review was running; its native alias/case tests
are still required before acceptance. Revocation retains a lexical-key fallback
when a target has disappeared, while grants require the resolved key.

Credential Manager values use the existing service namespace, UTF-8 blobs,
explicit blob-size limits and current-user persistence. Reads release the native
allocation on every post-read result. No provider credential or live stored
secret was read for this review.

## API evidence

- [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
  documents sharing restrictions, delete sharing for rename, creation security
  descriptors and `FILE_FLAG_OPEN_REPARSE_POINT`.
- [GetFinalPathNameByHandleW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew)
  documents normalized paths retrieved from handles.
- [CREDENTIALW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw)
  documents generic credential blobs and persistence semantics.
- [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew)
  documents the current-user credential operation.

## Timed shell gate review

Also reviewed the lead agent's host gate in `chat.rs`, Windows schema in
`scoped.rs`, `shell_access.rs` grant lifetime checks, and
`executor_windows.rs::execute_host_resolving`. No fail-open was found in this
source review. With no sandbox, only `run_shell` carrying a live timed grant
passes the initial gate. The existing worker ownership, active-turn and exact
command permission checks remain after that gate. Before the execution future
is polled, `run_unrestricted` rechecks the original grant identifier, permission
context, configured allowance and active turn; it repeats those checks and wraps
the run in the remaining grant duration. The Windows sandboxed dispatch function
returns failure for every operation. The schema describes that limitation.

This review does not establish native process-tree termination; the Windows
process job tests must supply that evidence on Windows.

## Follow-up broker review, 2026-09-30

The new filesystem broker required a second review beyond the earlier disabled
executor. Two related findings were returned to the owner: Windows sharing modes
do not freeze directory case-sensitivity attributes, and independent absolute-path
opens can collect an ancestor handle from a different tree than the final leaf.
The owner changed the broker's chain construction to handle-relative `NtCreateFile`
opens and changed positive permission coverage to pinned native object identities.
New-file authorization checks the pinned parent identity and conservative blocked
names, then creates the leaf relative to that same parent handle. Source re-review
found those changes address the identified chain and case-folding defects. This
still requires native race tests; it is not a native execution result.

`seatbelt::protected` now builds Windows-specific roots from USERPROFILE, HOME,
APPDATA and LOCALAPPDATA. It includes default and overridden Codex/Claude config
locations, Microsoft Credentials/Vault/Protect directories, and MiniClaw data,
settings and runtime paths. Missing credential directories remain explicit denied
paths, so later sign-ins cannot silently place credentials in a previously granted
folder. The Windows capability gate was not enabled by this change.

## Network service and update follow-up (2026-09-30)

Source review identified and coordinated fixes for three privileged-boundary issues:

- Elevated installation originally reopened a user-writable script after UAC. The launch now embeds the already captured script bytes in its encoded payload and enforces a command-length bound.
- Copied executable ownership must be protected at creation, not repaired after copying: an ordinary owner could retain a write handle across later ACL repair and hash verification. Installer/updater now use an admin-owned protected `FileSecurity` with the security-taking `FileStream` constructor. The updater rejects existing unprotected owners/DACLs before reading config or staging replacement bytes. [Microsoft constructor contract](https://learn.microsoft.com/en-us/dotnet/api/system.io.filestream.-ctor).
- Concurrent ACL and global loopback-exemption read/modify/write operations required native cross-process mutexes; the storage owner added protected named locks. A per-user lock must use the global namespace to include multiple logon sessions.

WFP source review found the split outbound predicates enforce the intended conjunction (IPv4 TCP, exact loopback address, exact proxy port); persistent deny filters commit before exemption is added, exemption is removed before filters, and dropping a lease retains enforcement. Native classification and external firewall interaction remain unverified.

The updater keeps service configuration and persistent filter recovery state, pins staged binary bytes, requires confirmed SCM stop with both exit codes zero before replacement, and attempts rollback only after another confirmed drain. Native service connection-task panics must mark cleanup uncertain; this finding was sent to the runtime owner because ignoring `JoinError` could report a successful stop without proving job death.

Setup regression: 139 tests, 135 passed, four Windows-only tests skipped on macOS (`/private/tmp/windows-setup-service-final.log`). The updater regression was first observed failing before implementation. Native PowerShell parsing, atomic ACL constructor behavior, SCM drain/rollback failure injection, cross-session locking, and actual AppContainer/WFP enforcement still require Windows execution. No native acceptance is implied by the host tests or cross-compilation.

Windows CLI recovery follow-up: native `verify` now checks the host backend, account and SCM service without VM endpoints; `--chat` explicitly opts into one small model call. Interrupted network setup records a private intended service identity and pending application settings, retains files on ambiguous service completion, and `resume` revalidates the data ACL before loading the receipt. Existing services are adopted only through the elevated updater's protected-path/SCM checks; hashes and intended identities must match. A protected exclusive update lock serializes service replacement.

Latest setup/CLI regression after recovery: 153 tests, 149 passed, four native Windows tests skipped (`/private/tmp/windows-setup-recovery-final.log`). Native tests remain pending. Service identity validation now accepts canonical local/domain `S-1-5-21` and Azure AD `S-1-12-1` user SIDs, requires four bounded numeric components, and rejects built-in service principals. The wrapper compares the saved owner with the actual invoking user; the native broker independently compares real peer token ownership. This change was observed failing before implementation, then passed the focused host tests.

Cache replacement follow-up: the downloader now holds native directory handles from the drive root through the cache for the complete download/hash/publication lifecycle, without delete sharing. The helper rejects reparse points, case-sensitive directories, unexpected owners and untrusted ancestor mutation rights (including generic write/all), while tolerating harmless ancestor create-file/create-directory rights. Unexpected helper exit aborts the download. Runtime independent review found no further definite source flaw in the native struct, flag, mask and lifetime contract. Native tests cover broad leaf ACL rejection, junction ancestry and rename rejection while pins are held; they have not executed on Windows.

Latest full setup/CLI regression: 160 tests, 153 passed, seven native-only tests skipped (`/private/tmp/windows-setup-pinned-final.log`). Setup copy now names AppContainer and WFP prerequisites while preserving the native-validation gate. The writable-descendant ACL change was reviewed: object identity is checked before granting, roots remain pinned, and cleanup follows the original ACL handle after rename. Native move/delete behavior still needs execution evidence.

## ACL crash journal review (2026-09-30)

The exact-object ACL update uses the documented user-mode `NtSetSecurityObject` entry with an identity-checked handle and `WRITE_DAC`; this removes reliance on `SetSecurityInfo`, whose documented behavior propagates inheritable ACEs through existing descendants. [Native call contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetsecurityobject), [automatic propagation](https://learn.microsoft.com/en-us/windows/win32/secauthz/automatic-propagation-of-inheritable-aces).

Findings sent to the storage owner:

- A released journal writer lock does not establish that its sandbox job has stopped. In particular, a cleanup timeout releases that lock while descendants remain uncertain. Recovery must require independent job-death evidence or retain bound orphan records without revoking/deleting the profile.
- The parser's `take(5001)` must detect a remaining tail rather than treating truncated malformed input as complete. Normal generated journals are bounded by the global 5,000-object limit; this is malformed-record robustness. Line allocation must also be bounded before, rather than after, reading a full line.
- The new native regression checks child DACL bytes, but protection/inheritance control flags on the directly updated parent also need native preservation assertions. A fresh descriptor currently supplies no original control flags; no behavior claim was made without native execution.

Review-only changes: implementation remains with the storage owner. Native execution is still pending.

## Optional bundle deployment audit

The Windows MCP editor and backend now accept native drive paths and explicit read-only `runtimePaths` (at most eight directories). Previously `Draft::valid` still required a leading Unix slash. Setup obtains the selected Python interpreter's base prefix before review, displays that read grant, rechecks the selection before creating the virtual environment, and records the result in protected MCP configuration. Runtime execution does not infer authority from a mutable `pyvenv.cfg`. Standalone Python MCPs use the same editor field. Windows setup now requires an installed Python interpreter; uv may install its dependencies but cannot silently choose an unreviewed external runtime.

A disposable real installation of the bundled Blender MCP wheel and MCP 1.30.0 installed 39 packages and contained 7,652 filesystem entries before counting the base standard library. Its stdio MCP initialization succeeded without Blender, credentials or external API calls. This demonstrated why the old 5,000-object native grant limit could not admit the bundle; the storage owner raised bounded grant/journal budgets. This measurement used macOS Python 3.14, not a native Windows interpreter.

Both bundled custom Python transports had a reproducible encoding failure under a Windows legacy pipe codec: a Unicode JSON-RPC request ID was corrupted. Explicit UTF-8 stdin/stdout configuration fixes the issue; both subprocess tests passed after failing before the fix, and bundle manifest hashes were updated. Group-owned asset upload already prefers the configured group ID and omits the user creator in that case; developer products use the configured experience universe rather than a personal owner ID. No real API key or paid API was used.

[Roblox's current Windows MCP entry point](https://create.roblox.com/docs/studio/mcp) remains `%LOCALAPPDATA%\\Roblox\\mcp.bat`; the preset supplies this directory as cwd and invokes a static relative batch name. ElevenLabs remains a standalone MCP and was not added to the Roblox bundle. Native Blender socket, Studio plugin loading, group-authorized API behavior and Windows Python dependency startup still need guest verification.

Verification after these changes: 159 setup/CLI tests, 152 passed and seven native-only skipped (`/private/tmp/windows-bundle-final.log`); Svelte check reported zero errors/warnings. Rust runtime-path input test passed after its expected pre-implementation rejection. The precise test selection differs from the previous full suite totals.

## 2026-09-30 follow-up: setup state, import paths, staged prerequisites

The Windows service client configuration previously used Node `writeFile(mode:0600)`, which does not produce a protected Windows DACL. The backend now exposes a bounded `--write-private-file` setup primitive: private parent, create-new private descriptor, or existing private file with identical bytes only. Payload arrives on stdin, never command arguments or output. The installer uses it for provider selections, pending setup and service intent receipts, and the service client configuration. Failure can retain a partial private file; retries refuse to overwrite it. Source independently reviewed by the filesystem agent. Host integration passes; native protected/inherited ACL test awaits execution.

MCP import now uses native absolute path and PATH parsing, USERPROFILE fallback, and Windows native `.exe`/`.com` discovery. Batch shims require an explicit native launcher. Fixture test was red before implementation and passes on the host; native Windows-specific assertions await execution.

Windows protected roots now include configured native helper/provider installation parents (including adjacent dependencies), plus an explicit network client configuration override. Pure synthetic tests pass. No real provider or credential files were read.

Official ARM64 Blender 4.5.9 ZIP, x64 Roblox Studio installer, UTM-linked experimental signed ARM64X Triton driver, and official ARM64 ChatGPT MSIX were staged outside the repository under `../work/windows-vm`. Blender matches its published SHA256; Triton matches the GitHub release digest. Roblox and ChatGPT have local transfer hashes; native Authenticode/package validation remains pending. The actual `@oai/cua` ARM64 helper was extracted for guest smoke testing. Its adjacent package directory has no other files; strings reference built-in Windows DLLs and a Codex app-server authentication exchange. Native launch/authentication remains unverified. The independent `@oai/sky` same-name helper differs and was not selected.

Sources: [Blender release files](https://download.blender.org/release/Blender4.5/), [Roblox Studio setup](https://create.roblox.com/docs/studio/setup), [UTM Triton announcement](https://blog.getutm.app/2026/introducing-triton-directx-11-driver-for-qemu/), [official Windows app deployment](https://learn.chatgpt.com/docs/enterprise/windows-deployment). Triton is experimental and needs compatible host graphics support; it is staged only. Actual Blender headless and Studio rendering tests are needed before any compatibility conclusion.

## Embedded workspace Git (2026-10-04)

Windows setup now includes checksum-pinned MinGit 2.56.0 from the [official Git for Windows release](https://github.com/git-for-windows/git/releases/tag/v2.56.0.windows.1). The planner includes its download and expanded size before approval. The installer reads no existing Git installation, home configuration or credentials. It writes a private runtime under `data/windows-git-runtime/<UUID>` and publishes `data/windows-git.json` through the backend's private-file writer only after extraction completes.

[windows-git.mjs](../../scripts/setup/windows-git.mjs) verifies the archive in memory, rejects traversal, aliases and link entries, and creates each file separately. It retains helper scripts and CA certificates, but omits packaged `etc/gitconfig`, which otherwise includes other machine Git configurations. The runtime receives read-only access to this exact copied tree separately from workspace permissions. Host Git configuration stays disabled. The architecture prefix comes from the inspected packages: `clangarm64` for ARM64 and `ucrt64` for x64.

Update (2026-10-04): setup now installs MiniClaw's own patched Git build carried inside the release (`bin/git`, prefix `git/` for both architectures) instead of downloading MinGit; see [git-runtime.md](git-runtime.md). The MinGit pins above remain in the code but are no longer used by setup.

Both official archive hashes were checked and both passed the production decoder. ARM64 produced 498 ordinary single-link files; x64 produced 372 entries. Forced extraction failure leaves no runtime configuration and removes its partial UUID directory. Red-to-green tests cover package metadata, archive rejection, rollback and installation planning. The setup suite passed 151 tests with seven native-only skips. Independent review closed a Windows superscript device-name alias issue. Native Git execution, helper/DLL resolution, HTTPS trust and workspace acceptance still require the guest run.


## Python architecture after native Blender acceptance

The first Windows ARM64 Blender smoke reached dependency installation and failed while
building cryptography50.0.2 from source because OpenSSL development files were absent.
[Official PyPI metadata](https://pypi.org/pypi/cryptography/50.0.2/json) publishes current
Windows wheels for x64 only; historical ARM64 wheels end at46.0.3. Setup now requires
x64 Python on Windows, including ARM PCs, and permits binary wheels only for both pip
and uv. It does not downgrade cryptography or install a compiler toolchain for users.
Windows can run x64 Python on ARM through [documented emulation](https://docs.astral.sh/uv/concepts/python-versions/#transparent-x86_64-emulation-on-aarch64).
Blender itself remains ARM64 and communicates with Python over loopback.

`checkRobloxBundle` probes `sysconfig.get_platform()` and captures the selected base
prefix before review. It checks default per-user Python installation folders and PATH.
For a custom installation, set `MINICLAW_BUNDLE_PYTHON` to the absolute x64 `python.exe`.
The wizard's prerequisite error links the [official Windows installers](https://www.python.org/downloads/windows/).
The reviewed interpreter and read-only folder must still match at installation time.
Five dependency tests failed before the change and passed afterward. A sixth verifies
that a changed runtime prefix fails before installation. Independent review found no
remaining issue in selection, review enforcement or binary-only commands.

The real Windows11 ARM64 retry passed with Python3.13.16x64 and Blender4.5.9 LTS ARM64.
Bundled MCP created NativeSmokeRobot (eight vertices), exported GLB, and exercised the
persistent bridge plus MCP adapter with matching staged bytes and manifest. Export SHA256:
`7e67ceece6c89c5d633c24fbf64ca9b8aed3bd50b8474b53513ab12301439666`.
Evidence is under `C:\Users\S\AppData\Local\Temp\MiniClaw Blender smoke jwjesru7`.
The smoke used manifest package pins and binary-only installation. Production checkRobloxBundle/prepareDependencies also passed in the guest as user S,
installing binary-only cryptography50.0.2 and importing blmcp successfully (scratch
`MiniClaw native bundle rVkqGH`). This verifies dependency setup, not sandboxed MCP execution. GUI addon installation,
Studio insertion, API upload and rendering were not tested.

## Packaged ConPTY runtime

Windows host packaging fetches Microsoft.Windows.Console.ConPTY 1.24.260710001
from the [official NuGet package endpoint](https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.24.260710001/microsoft.windows.console.conpty.1.24.260710001.nupkg),
or reads `MINICLAW_CONPTY_ARCHIVE` for an offline build. Both paths verify the
same archive size and SHA256 before decoding. `windows-conpty.mjs` selects only
the architecture's two pinned binaries; `backend-package.mjs` records them and
Microsoft's [MIT notice](https://github.com/microsoft/terminal/blob/main/LICENSE)
in the bundle manifest under `bin/conpty`.

`refreshRuntime` requires the pair and notice before changing installed files.
It replaces the backend first, then its terminal files. Failure restores earlier
native replacements from temporary copies; recovery failure retains the copies
and reports their directory. The version manifest is published after the runtime,
network service and page updates. The host regression verified rollback and
both real architecture archives. Native Windows acceptance passed28/28 with zero
skips, including unchanged terminal files when a running backend refuses update.

## Provider settings beside a live database

Directory namespace guards now share writes while denying delete access. This
allows an atomic child-file rename while SQLite keeps its parent guards open.
File guards retain their previous sharing modes. The Store-held settings test
reproduced Windows error32 before the fix and passed afterward; native filesystem
checks passed8/8 and provider controls passed9/9 with unchanged deadlines. Microsoft
documents [write and delete sharing separately](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).

## Packaged sandbox shell

[`windows-shell.mjs`](../../scripts/setup/windows-shell.mjs) selects the official
BusyBox FRP6075 ARM64 or x64 Unicode binary, verifies its pinned bytes, and requires
the matching complete source archive before returning package contents. The
[upstream download page](https://frippery.org/busybox/) identifies the binary and
corresponding source distributions. The full source archive includes architecture
configurations and build scripts. The package carries its unchanged GPL version2
license alongside the executable and source under `bin/shell`.

Both architectures' actual binary hashes and PE identities were checked on the
host; the source archive and rejection of changed source bytes were also checked.
The staged ARM64 shell has passed native integrity, workspace boundary and
persistent-terminal state/grant checks. Final portable-package installation has
not yet passed. `refreshRuntime` includes all shell files in its recoverable native replacement group. The Windows model
briefing describes POSIX sandbox commands and reserves PowerShell for the explicit
timed unsandboxed grant.

## Installation folder protection (2026-10-04)

The network service is a copy of `runtime\bin\miniclaw-server.exe` that runs as
LocalSystem after UAC approval. An installation folder another account can write
would let that account plant the executable the administrator then approves.
A new folder at a drive root is such a place: non-administrators may create
subfolders there, and the folder inherits the root's access rules
([Margosis, unquoted service paths](https://learn.microsoft.com/en-us/archive/blogs/aaron_margosis/it-rather-involved-being-on-the-other-side-of-this-airtight-hatchway-unquoted-service-paths)).
`mkdir` mode bits have no effect on Windows.

[`destination.mjs`](../../scripts/setup/destination.mjs) now claims the folder through
[`windows-install-directory.ps1`](../../scripts/setup/windows-install-directory.ps1):

```powershell
$acl.SetAccessRuleProtection($true, $false)
$acl.SetOwner([Security.Principal.SecurityIdentifier]::new($user))
foreach ($sid in @($user, 'S-1-5-18', 'S-1-5-32-544')) {
```

The script pins every ancestor without delete sharing, refuses an ancestor whose
owner is untrusted or that lets another account replace, rename or re-permission
the path (the same `0x500D0150` mask as the download cache check), then creates the
folder with `CreateDirectoryW` and its final protected ACL. It never changes an
existing object's ACL. SYSTEM and Administrators keep access so an administrator
account other than the installing user can read the backend during elevation.
The wizard rejects share, device, stream and relative destinations first.

`pinnedBackend` in [`windows-network.mjs`](../../scripts/setup/windows-network.mjs)
runs the same script in verify mode before every installation, resume and update,
then requires the backend to match the hash recorded in `runtime\manifest.json`
and passes that manifest hash to the elevated script:

```js
export async function pinnedBackend(runtime, { verify = verifyInstallation } = {}) {
  await verify(dirname(runtime));
```

Verify mode requires that no other account holds more than read and execute on
the installation folder, `runtime`, `runtime\bin` and the backend. An installation
made before this change in a shared location fails the check and must be reinstalled.
`start-host.mjs` does not repeat the check at each start.

The elevated scripts treat a stopped service as drained when it reports `NO_ERROR`,
or when it reports `ERROR_SERVICE_NEVER_STARTED` (1077) and the `leases` journal holds
no record. Microsoft documents that a service sets
[`dwWin32ExitCode` to `NO_ERROR` on normal termination](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/ns-winsvc-service_status)
and that 1077 means
[no start attempt was made since boot](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--1000-1299-).
The update script restarts the original binary when the swap fails after the drain.

Native execution on VM2 (2026-10-04) found that an update pinned the swapped backend
against the installed manifest, which `refreshRuntime` publishes last, so every changed
backend was refused. The update now pins against its bundle's manifest, which
`pinnedBackend` accepts only inside the verified installation folder. The 1077 rule works
for a new service, whose configuration folder has no `leases` yet. Once the service has
run, `leases` is private to SYSTEM (`platform_fs::private_dir` in the service), so the
elevated update cannot list it and fails closed, and `windows-ci-stop-network.ps1` cannot
remove the configuration folder.
