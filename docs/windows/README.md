# Windows development branch

This is an experimental port, not a supported Windows release. The user requested
all normal MiniClaw features with Windows VM support excluded. The full request
is not yet complete; [the acceptance checklist](../../WINDOWS_PORT.md) tracks it.
The branch is `windows-port`, based on the supplied source archive (`20595a9`).
Native verification runs on two Windows 11 ARM64 guests (build 26100) in UTM on the Mac.
No changes were installed into the existing Mac runtime.

## Architecture status

- Windows ARM64: all native verification of the port ran on Windows 11 ARM64 guests.
- Windows x64: builds and passes host checks; not verified natively in the sandbox.
  The x64 pins match their bytes and every shipped x64 executable is PE machine 0x8664, the
  Windows GNU x64 cross-check builds all targets, and the release READ ME for x64 says the same.
  The only sandboxed x64 run (x64 Git under ARM64 emulation) failed with STATUS_NO_MEMORY; it is
  judged an emulation artifact, but only a run on real x64 hardware can settle it. Details and the
  single deciding test: [x64 readiness](x64-readiness.md).

## Source implementation

- Windows x64 and ARM64 installer paths, native executable discovery, portable
  Node packaging and update checks. Setup retains sandboxed defaults.
- Windows drive paths in MCP and agent-permission proposal cards; spaces and Unicode.
- Windows Credential Manager secrets and protected owner-only DACLs for private
  state, with checked handles, link refusal and a SQLite lifetime guard.
- Existing Windows permission targets normalize handle-resolved names, including
  case and 8.3 aliases. Case-sensitive trees and ambiguous new names are refused.
- Authenticated loopback Claude bridge. Mac continues using Unix sockets.
- Native process creation supplies an explicit inherited-handle list and assigns
  a kill-on-close Job Object atomically at creation, retaining suspension until
  policy setup completes. Cancellation terminates
  the tree. MCP and provider subprocesses use the same Windows launcher so a
  concurrent broad-inheritance launch cannot accidentally inherit tool pipes.
- PowerShell commands use UTF-16LE `EncodedCommand`, explicit UTF-8 output and
  text error output. Timed shell grants retain backend ownership, expiry and
  revocation checks. This is unrestricted execution only after the user grants it.

## Isolation and desktop integration

The Windows file executor now uses parent-relative native handles and checks
every operation against positive grants and protected roots. LPAC execution grants
access to specific file identities, retains recovery records for ACL cleanup,
and keeps each run's scratch directory under protected MiniClaw state. See the
[sandbox implementation review](sandbox-review.md).

Sandboxed shell, stdio MCP and the ConPTY terminal use the same process sandbox.
An authenticated Windows service installs WFP restrictions and verifies job
ownership. It does not launch model commands. Networked children remain suspended
until the service acknowledges their policy. Service setup, update and interrupted
setup recovery are implemented in [the setup scripts](../../scripts/setup/windows-network.mjs).

## Sandbox availability

Since 2026-10-05 the sandbox is available on a Windows installation that can enforce it, and
only there. [`seatbelt::available`](../../server/src/seatbelt.rs) asks
[`windows_gate`](../../server/src/windows_gate.rs), which checks, in this order:

| Precondition | How it is checked | What the user is told when it is missing |
| --- | --- | --- |
| `MINICLAW_DISABLE_SANDBOX` is unset | environment; any non-empty value closes the gate, nothing opens it | "The sandbox was switched off for this run with MINICLAW_DISABLE_SANDBOX." |
| Windows build 26100 or later (Windows 11, version 24H2) | `RtlGetVersion`, once | "MiniClaw's sandbox needs Windows 11, version 24H2 (build 26100) or later. This computer runs build N." |
| Process creation knows the LPAC attribute | `UpdateProcThreadAttribute` with `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` on an empty list, once; nothing is created | "This Windows version cannot start the restricted process type MiniClaw's sandbox uses." |
| The pinned shell beside the backend has the released bytes | `windows_shell::runtime`, pinned until the backend exits | "The sandbox's shell is missing or was changed. Run `miniclaw update`, then start MiniClaw again." |
| The pinned ConPTY pair has the released bytes and loads | `windows_conpty::runtime`, pinned until the backend exits | "The sandbox's terminal runtime is missing or was changed. ..." |
| The network service is configured, registered, running as LocalSystem and proves the installation key | `windows_network_broker::readiness`: private client configuration, SCM registration and state, then the handle-free pipe authentication with a fresh HMAC challenge | one of four sentences: not installed (run `miniclaw resume` or `miniclaw setup`), removed from Windows, not running (start "MiniClaw network isolation" in Windows Services, or restart Windows), could not be verified (run `miniclaw update`) |

Why the build floor: Microsoft documents no minimum version for the LPAC attribute
([UpdateProcThreadAttribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
lists `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` for Windows 8 and does not list the LPAC
attribute; [Launch an AppContainer](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
describes LPAC without a version). Every native isolation result, and the Winsock catalog key
list, was measured on build 26100. Nothing older is claimed; lowering `MIN_BUILD` needs a
native run on the older build.

Why the service is required even for a command without network: every sandbox launch writes a
journal whose first line is the service's authenticated boot epoch
(`Journal::create` calls `verified_boot_epoch`), and recovery after a crash asks the service to
attest that the run's job is dead; a standard account cannot prove that by itself. An offline
run needs no filter from the service: its token holds only the installation's Winsock-read
capability, so Winsock starts but no socket can be created (2026-10-05, "Language runtimes in the
sandbox: decision and implementation" in the [sandbox review](sandbox-review.md)).

Two capabilities depend on the gate and on something of their own:

- **Git views** (`executor::git_available`): the private Git that setup installs
  (`windows-git.json` in the data folder). Without it Changes shows the conversation view,
  discard answers `discard_unavailable` with the reason, and the briefing says Git could not be
  checked instead of "not a git repository".
- **Network for commands**: the service publishes whether its Winsock catalog entries are
  healthy (`winsock-acl-status.json`); `windows_gate::network` repeats that so the page can say
  it early. The service decides again at every lease and refuses in its own words.

`GET /api/sandbox` returns `{ available, reason?, git: { available, reason? }, network: {
available, reason? } }` on both platforms, and on Windows also `access: { state, time?, reason? }`
when the service has published one: `healthy`, `repaired` (Windows had removed entries and the
service put them back) or `failed`. `POST /api/sandbox/repair` asks the running service to check
and repair its entries now (no elevation); when it cannot, the answer names `miniclaw repair`,
which restarts the service from the installed backend with one administrator approval. The
permission settings show the message and the "Set up again" button (`SandboxAccess.svelte`). A closed gate behaves exactly as the tested
gate-closed path did before: no sandboxed tool is offered, the terminal answers
`terminal_unavailable`, a confined stdio MCP server is refused, Git is not run, nothing is
stamped on a folder and nothing runs unrestricted instead. Each refusal ends with the reason
above, and the model's briefing carries it too. `execute_host` (shell), `execute_git` and the
MCP spawn check the gate themselves as well as their callers.

Cost: the answer is reused for 2 seconds. Measured on the test guests (debug build): a fresh
check 0.8 to 5.2 ms, a reused answer about 50 ns. A stopped or started service is noticed within the
reuse time; a stale "available" opens nothing, because every launch authenticates the service
again before its sandbox exists. There is no test hook that can open the gate.

Known gaps, each failing closed with a clear message: a declared listening port is refused as
unsupported; the page reads `/api/sandbox` only for the access message above. A repository
whose folder belongs to Administrators or another account is refused by the bundled Git: Changes
then shows the conversation view without saying why, and discard fails with that reason.

Cost that grows with the folder: every sandbox launch stamps the whole working folder. With
50,000 files a terminal opens in about 8 s and each Git command of the Changes view takes about
8 s (release build, [TESTING.md](../../TESTING.md), 2026-10-05); with the private Node staged
(`d155e6d`) the same check measured 9.7 to 11.4 s to open the terminal and 10.5 to 12.6 s per Git
command (`gate2-release-e2e-vm2-1.log`). A 5-file folder: terminal open request 64 to 71 ms, first
output after about 3.1 s, one Git command 90 to 121 ms.

Evidence (2026-10-05, `work/windows-vm`, all with verified source hashes and fresh builds):
`gate-rust-vm2-3.log` (elevated owner, UAC on: 71 groups, 444 tests), `gate-rust-vm1-1.log` (the
same list on the first guest, UAC off: 444 tests), `gate-standard-vm2-1.log` (standard user: 88
tests), `gate-js-vm2-gate-5.log` (node suites with the ordinary token: 138 tests),
`gate-e2e-vm2-2.log` and `gate-release-e2e-vm2-2.log` (product check through the HTTP API, with
latency), `gate-release-closed-vm2-1.log` (a release backend without its shell: every entry
point refuses with the reason). With the private Node (checkpoint `d155e6d`, VM2, UAC on):
`gate2-acceptance-vm2-7.log` (`scripts/windows-native-acceptance.ps1` exit 0: 321 tests, 94 of
them as a disposable standard user), `gate2-js-vm2-full-2.log` and `gate2-js-vm2-fix-1.log` (all
51 node suites with the gate open), `gate2-release-e2e-vm2-1.log` (release product check 15 of 15),
`gate2-e2e-stopped-vm2-1.log` (network service stopped: every entry point refuses with "The
MiniClaw network service is not running", nothing stamped). A launch no longer fails when a run
ends during its admission check ([sandbox review](sandbox-review.md), "A run ending while another
starts").

The Windows computer adapter implements the installed helper's request protocol
and relays approvals. It still needs interactive native authentication and app
approval tests. See [computer-use integration](computer-use.md) and its
[request sequence](computer-use-sequence.md).

Windows VM routes and the VM worker are intentionally unsupported. Uploaded
images use the bounded original on Windows; the Mac `sips` resize helper is not
launched there.

## What runs inside the sandbox

| Tool | Status | How |
| --- | --- | --- |
| Shell commands, terminal | work | The pinned BusyBox shell; ConPTY for the terminal. |
| Git, including HTTPS | works | The bundled Git (lpac5), its OpenSSL rebuilt to use `BCryptGenRandom`, its exit status correct on the terminal; HTTPS only through approved hosts. |
| Node | works with MiniClaw's private Node | `bin\node` holds the official Node.js 24.21.0 `node.exe`, MiniClaw's OpenSSL seed library and an `openssl.cnf` that makes Node seed from `BCryptGenRandom`. `node` in the shell and terminal finds it first on `PATH`; an MCP server whose command is any `node.exe` runs with it instead. The user's own Node is never started inside the sandbox. Script files, ES modules and packages work (`NODE_OPTIONS` gains `--preserve-symlinks --preserve-symlinks-main`). Network only through approved hosts (`NODE_USE_ENV_PROXY=1` is set with the proxy). Not working yet: a child process with piped standard handles or a `fork` channel (`exec`, `execFile`, `execSync`, `spawnSync`, `spawn` with the default pipes, `fork`). Node 24.21's libuv 1.52.1 names those pipes outside the sandbox's namespace and would retry forever; MiniClaw's preload (`bin\node\child-pipes.cjs`, added to `NODE_OPTIONS`) refuses such a child at once with `ERR_MINICLAW_SANDBOX_CHILD_PIPES` and one line on standard error, and an MCP server that stops because of it is reported as "This server starts a child process, which the Windows sandbox cannot support with the bundled Node yet." Children with inherited, ignored or file handles work. No official Node bundles the fix (libuv 1.53.0) as of 2026-10-05 ([sandbox review](sandbox-review.md), "Child processes of sandboxed Node"). |
| Python (ARM64 Python on ARM64 measured; x64 hosts not yet) | starts; `asyncio` MCP servers work | The launcher primes Python's side-by-side manifests before starting it: the interpreter, `python3*.dll`, and the extension modules of `DLLs` and `site-packages` (also for a virtual environment's launcher and its base interpreter). `bin\python\sitecustomize.py`, first on `PYTHONPATH` in every sandboxed run, gives `asyncio` a named-pipe wake-up channel and named subprocess pipes in the run's own namespace, since the sandbox refuses the loopback listener and global pipe names asyncio uses. An MCP server whose command is a Python image always runs in the networked token with its proxy and lease, allowing only its approved hosts (possibly none): `asyncio` cannot be imported without a socket. In the shell and terminal without approved hosts `asyncio` still does not import. Not covered: isolated mode (`-I`, `-E`), embedded Python with a `._pth` file, `uv run`/`uvx`, the selector event loop, `multiprocessing` ([sandbox review](sandbox-review.md), "asyncio in the sandbox"). |
| x64 programs on ARM64 | do not start | The emulator needs registry keys the sandbox cannot read ([sandbox review](sandbox-review.md), "x64 programs on ARM64"). |
| `npm`, `npx` in the shell and terminal | work with MiniClaw's npm | `bin\node` also holds npm 11.19.0 from the same pinned Node archive (`node_modules\npm`, its tree pinned by digest) and two `sh` launchers, `npm` and `npx`, which run npm's scripts with the private Node. Package scripts run in BusyBox `sh` (`sh.exe` beside it, primed outside the sandbox like Python), never `cmd.exe`, with npm's own standard handles; a script that captures a child's output (esbuild's) fails at once with `ERR_MINICLAW_SANDBOX_CHILD_PIPES`, see the Node row. The user's `.npmrc` is not read; cache and global folder live in the run's scratch. Network only through approved hosts (`registry.npmjs.org`). |
| MCP servers saved as `npx …` or `npm exec …` | work | Import saves the bare name; nothing named `npx.cmd`/`npm.cmd` is ever started. The first start installs the package with MiniClaw's npm, in the sandbox through the server's approved hosts (the registry host must be one), into the server's own folder `<data>\mcp-npm\<id>`; every start then runs the package's command script with the private Node and the saved arguments as an argument list, the install read-only. Later starts need no network. The server list says what runs (`runs`). |
| Other `.cmd`/`.bat` launchers as MCP commands | refused | Batch files are never started through `cmd.exe`; name `node.exe` and the package's script instead. |

Compatibility of the private Node: projects that need another Node major version, and native
addons built for another Node ABI, do not load; a server that relies on the user's global packages
must name their folder as a runtime path. Node security releases reach the sandbox with a MiniClaw
update (three pins: `scripts/package-release.mjs`, `scripts/setup/windows-node-sandbox.mjs`,
`server/src/windows_runtimes.rs`). The first official Node with libuv 1.53.0 or later makes piped
children work; the preload then does nothing by itself.

## Working folders of conversations (2026-10-05)

- A project conversation works in its project's folder. A `run_shell` without `cwd` runs there.
  A sandboxed command whose `cwd` the sandbox may not enter (not the working folder, not a granted
  folder, or inside a blocked one) is refused before it is asked about or started, and the reason
  names the folder to use. The AppContainer shell cannot start in such a folder: in report
  cf7b301 the model passed the provider's own folder (`<install>\data\chat-workspace`) and BusyBox's
  `cd` failed with exit 2.
- A conversation with no project and no agent gets its own folder at its first command or file
  write: `Documents\MiniClaw\<title> <id prefix>`, Documents being the user's Documents known
  folder (`SHGetKnownFolderPath(FOLDERID_Documents)`, so a folder moved to OneDrive is followed).
  Only that folder is granted to its sandbox (read and write); the rest of Documents, other
  conversations' folders and MiniClaw's data folder stay denied. Path comparisons for the working
  folder resolve both sides (`platform_fs::path_within`: case as on disk, forward slashes, the
  verbatim prefix) and compare whole components.
- Deleting such a conversation keeps its folder unless the box in the dialog is ticked; then the
  folder moves to MiniClaw's Recovered files folder, only when it is a real folder (not a link)
  directly inside `Documents\MiniClaw` and recorded for that conversation alone.

## Claude Code sign-in (2026-10-07)

A user of 0.1.5 pressed Sign in for Claude and saw only waiting. Up to 0.1.5, MiniClaw started
`claude auth login --claudeai`, discarded all of its output and only polled `claude auth status`.
The page never got the sign-in address, a CLI that stopped with an error left the attempt
"pending" for three minutes, and Settings > Providers showed no link or Cancel at all.

Measured on VM1 with Claude Code 2.1.289 (logs in `work/windows-vm/claude-login-*.log`): the CLI
prints `If the browser didn't open, visit: <url>` (the code-page variant of its sign-in address)
with piped, closed or open input, with MiniClaw's environment allowlist and in a pseudoconsole.
It then opens a second address (the same one with a `localhost` callback) by running
`rundll32 url,OpenURL`, or the program named by `BROWSER`. On VM1 that opener worked from the
released backend, so the exact failure on the user's PC is not reproduced. Two defects remain on
every Windows PC: the opener runs inside the provider's kill-on-close job, so a browser that was
not already running would be started inside it and closed when the sign-in ends; and the
`rundll32 url.dll,OpenURL` pattern is a known living-off-the-land command that security tools may
block, which leaves nothing on screen.

Since this fix MiniClaw reads the CLI's output ([`claude_login.rs`](../../server/src/claude_login.rs)),
so the page shows "Open the Claude sign-in page" and a "Code from Claude" box with Cancel, in
Settings > Providers and on the first screen, and an error the CLI stops with. The address's query
and the code are never logged.

The normal path stays automatic on both platforms (lead decision, 2026-10-07): the browser opens
Claude's page whose callback is the CLI's own `localhost` listener, and the CLI finishes by itself.
On Windows the login CLI gets `BROWSER=<miniclaw-server.exe>` and `MINICLAW_SIGN_IN_OPENER=1`, so
it runs `miniclaw-server.exe <url>` as its opener. That step accepts only Claude's HTTPS sign-in
page or `http://localhost|127.0.0.1:<port>/callback`, then starts
`miniclaw-server.exe --open-sign-in-url <url>` with `CREATE_BREAKAWAY_FROM_JOB`, which opens the
page with `ShellExecuteExW` (COM initialized as Microsoft asks for
[ShellExecute](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew)).
Measured with a job like the provider job that reports every process added to it
(`claude-login-auto-job-vm1.log`): with rundll32, and with the opener in a job without breakaway,
the browser processes land inside the CLI's kill-on-close job; ShellExecute starts the browser
from the calling process, not from Explorer. So only the sign-in job sets
`JOB_OBJECT_LIMIT_BREAKAWAY_OK`, and only the opener's second step uses it; the CLI's other
children (reg.exe, where.exe) stay in the job, and Cancel still ends the CLI and them. The
trade-off: a process in that one job may leave it by asking for breakaway explicitly; the CLI is
the user's own provider, not sandboxed code, and every other job keeps breakaway off
([nested jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)).
If breakaway is refused, the first step opens the page from inside the job. The link and the code
box remain the fallback (Claude's code page, then paste). macOS is unchanged: the CLI opens its
page with `open` and MiniClaw opens nothing. While a sign-in waits, `claude auth status` runs at
most once per three seconds (plus once right after the CLI exits), not once per poll.

The Windows opener brings the browser up without focus when another window is in front
(screenshot `claude-login-auto-screen-vm1.png`); the link in the page opens it in front.

Workaround for 0.1.5: in PowerShell run `claude auth login`. It opens the sign-in page, or open
the address it prints. If Claude shows a code after the sign-in, paste it into that window. Then
press Refresh in MiniClaw's Settings > Providers; MiniClaw uses the same sign-in.

## Installing, repairing, updating and removing a release

All commands run from the unpacked release folder, which must be kept: its `miniclaw.cmd` starts,
updates and removes the installation (the installation folder holds `runtime\`, `data\` and
`settings.json`, not the command-line tool).

| Command | What it does | Administrator approvals |
| --- | --- | --- |
| `.\miniclaw.cmd setup` | Wizard: destination, port, optional bundle and Computer Use helper, review, install | 1 (network service); 1 more if the Visual C++ runtime is missing or too old |
| `.\miniclaw.cmd setup` again, on an installed folder | Offers the optional bundle for that installation and adds it (below); nothing of MiniClaw itself is built or installed again | 0 |
| `.\miniclaw.cmd bundle add sep-roblox [PATH]` | Adds Sep's Roblox Development Bundle to an installation, keeping its data (below); refuses while MiniClaw runs | 0 |
| `.\miniclaw.cmd bundle status [PATH]` | Lists which parts of the bundle an installation has (read-only, also while MiniClaw runs) | 0 |
| `.\miniclaw.cmd` | Starts the remembered installation; closing the window or Ctrl+C stops it | 0 |
| `.\miniclaw.cmd repair` | Restarts the network service from the installed backend, which checks and repairs its Winsock entries | 1 |
| `.\miniclaw.cmd update` (from the newer release) | Replaces the backend and every native file the new bundle records (ConPTY, BusyBox, Git, the private Node with its seed library and npm, Python's hook) as one recovery group, then the page; the service is updated inside the group, and any failure restores every earlier file | 1 (+1 for the runtime as above) |
| `.\miniclaw.cmd resume` | Finishes a setup that stopped after the network service step; on a finished installation it says there is nothing to resume and exits 0 | 0, or 1 when it has to finish the service |
| `.\miniclaw.cmd uninstall [PATH] [--delete-data]` | Asks first; refuses while MiniClaw runs from that installation. The service is stopped, started once with `winsock-acl-remove` so it deletes its own Winsock catalog entries, the registry is checked for its capability SID, and only then the service, `%ProgramFiles%\MiniClawNetwork-<id>` and `%ProgramData%\MiniClawNetwork-<id>` are removed (`scripts/setup/windows-network-remove.ps1`, one elevated step). Then `runtime\`, `settings.json` and the program files kept in `data\` (network client configuration, private Git runtime) go. The rest of `data\` stays, with a marker (`data\kept-after-uninstall.json`: this folder and the user's SID), unless `--delete-data` is given and confirmed a second time. Sep's bundle folder `<installation>-roblox-tools` stays with a plain uninstall (the command names it); `--delete-data` removes it too, but only when its `installed.json` says the bundle created that folder for this installation (0.1.6 and later), never a folder the user made or one from before 0.1.6. A declined approval removes nothing. | 1 |

`miniclaw setup` into a folder that uninstall left this way asks whether to install there again and keep
the data (conversations, projects, settings). It recognises the folder only when it holds nothing but `data\`,
uninstall's marker names this very folder and the signed-in user, and `windows-install-directory.ps1 -Kept`
confirms the folder is still protected, owned by that user and private (`destination.mjs` `keptData`); any other
existing folder is refused as before. The provider choice made in the wizard replaces the kept one, and the marker
goes once the installation is complete. A failed install leaves the folder exactly as uninstall left it.

### Adding Sep's Roblox Development Bundle later (2026-10-07)

Up to 0.1.5 the bundle could only be chosen at the first installation. A user who ran setup again into the folder
uninstall left with their data and chose the bundle saw steps 1 and 2 (building, private Git) run and step 3 stop with
"An agents database already exists; bundle setup never overwrites profiles". That message and "Nothing was installed;
your data ... was kept as it was" were true: the reinstall's own rollback removed `runtime\`, the settings, the private
Git runtime and its record (`data\windows-git*`) and put the earlier provider choice back; the data folder's
permissions are only checked, never changed, by `--secure-data-dir`; only a Visual C++ runtime that setup installed
first stays (it is Microsoft's, shared, and never removed). A finished installation could not be chosen at all
("That path already exists").

Now the bundle is added to what an installation has, through the backend's own store: `miniclaw-server --add-bundle
<data>` reads the catalog on standard input and merges it in one SQLite transaction while holding the data folder's
instance lock, so it refuses while MiniClaw runs ("MiniClaw is running from ... Stop it (Ctrl+C in its window), then run
... again. Nothing was changed.", the words uninstall uses). What "additive" means ([`bundles.rs`](../../server/src/bundles.rs)):

- an agent, skill or skill group with the same name (ignoring case and outer spaces; deleted agents do not count) and an
  MCP server with the same name are left exactly as they are and reported "already present"; the bundle's own
  references (its agent's skills, its groups' members, its rules) point to them; anything missing is added;
- permission rules and capability defaults are added only for the bundle agent added in that run, and the main agent is
  allowed only MCP servers added in that run; the main agent's model, effort and skills are set only in a data folder
  MiniClaw never opened (as a new installation with the bundle has them);
- conversations, projects, approvals, settings and saved keys are not touched; a refusal (a list limit, an invalid
  entry) adds nothing;
- bundle files beside the installation (`<installation>-roblox-tools`) are copied only where missing; a file that differs
  from the release's copy is kept and named; an existing Python environment is checked (`import blmcp`, and on Windows the
  interpreter's architecture) and reused; a failed add removes exactly what it created;
- running it again adds nothing (`miniclaw bundle status` shows what is there).

Entry points: `miniclaw setup` on a finished installation (the remembered one, or the folder typed at the destination
question) shows only the "Optional bundles" question and the review "Add Sep's Roblox Development Bundle to the existing
installation (your data is kept)"; `miniclaw bundle add sep-roblox [PATH]` does the same from the terminal. A reinstall
into kept data with the bundle chosen adds it after MiniClaw is installed again (last step); if that step fails, the
reinstall stands, the bundle step removes what it made, and setup says to run `miniclaw bundle add sep-roblox`. The
connection guide (with the Blender and Studio plugin paths setup finds) is shown after every add.

Workaround for 0.1.5 users: there is no supported command in 0.1.5 (its setup refuses an agents database that
exists). With a release that has this change: stop MiniClaw, run `.\miniclaw.cmd update` from the new release folder
(the add runs the installed backend, which must know `--add-bundle`; an older one is named with that advice before
anything is changed), then `.\miniclaw.cmd bundle add sep-roblox`. Until then the only way is by hand, as the final
test did on 2026-10-05 (`../work/windows-vm/final-test/REPORT.md`): copy the bundle folder, make its Python environment
and add each MCP server in Settings > MCP servers.

Commands without a path act on the remembered installation, which the last finished setup records. `update` and
`repair` now say first which installation they act on, `uninstall` names it in its question, and a setup that
replaces another remembered installation says so and names the earlier one, which stays installed
(`view.mjs` `rememberedNotice`).

Setup and update wait up to 5 minutes for the administrator approval (`windows-network.mjs` `approvalWaitMs`). On a
timeout, setup keeps its files and says to approve the prompt if it is still open and then run `miniclaw resume`; an
update is undone and says to run `miniclaw update` again. A failed update that was undone completely says that
nothing changed, why it failed and what to try; it advises reinstalling only when the undo itself failed.

Ctrl+C in a terminal where `.\miniclaw.cmd` runs makes cmd.exe ask "Terminate batch job (Y/N)?" after MiniClaw
stopped. That prompt belongs to cmd.exe's batch processing (Microsoft documents it for batch programs, e.g. in
[pause](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/pause)) and has no switch.
The known workarounds either end the user's own cmd.exe session (`... & exit`), open another window (`start`),
take console input away from the wizard (`< nul`), or patch cmd.exe; a native `.exe` launcher would avoid it but
is a new signed binary. The launcher therefore stays as it is: answering either way is harmless, MiniClaw has
already stopped.

The first sandboxed launch after the backend starts no longer pays for checking MiniClaw's own runtime files: the
private Node (about 80 MB hashed), npm's tree (about 2,000 files) and Python's hook are checked on a background
thread as the backend starts (`windows_runtimes::warm_up`). A launch that arrives earlier waits for that same
check, up to 3 minutes (`CHECK_WAIT`), and nothing is used before it passed. A `node.exe` MCP server runs on the
private Node, so the folder of the Node it named is no longer granted to its sandbox (that grant stamped every file
of a Node installation, npm included, at each launch: 6.8 s on the first launch after an install, 0.8 s later).
With `MINICLAW_LOG=debug` the backend logs each check's duration and, for each confined MCP server, the time to
prepare its sandbox, grant the runtimes, create and resume the process, and the time until it answered `initialize`.

Measured right after a fresh install on the ARM64 test guest (start check, first MCP check): before, the first
check failed with "The server did not answer in time" (15 s) and passed on a retry, 23.2 s in all; with these
changes MiniClaw's own share is 20 ms (sandbox ready) instead of about 7 s, and the first check passed in 12.4 s and
14.7 s without a retry in two installs, but timed out once more in a third. What remains is Windows starting the
first sandboxed process after an install or after the guest sat idle (7-16 s; concurrent sandboxed starts finish
together, and the same `node.exe` started outside the sandbox at that moment took 0.4 s). Logs:
`../work/windows-vm/polish-start-check-arm64-polish*-vm2-1.log`, `polish-first-launch-before-*-vm2-*.log`.

The service's `leases` journal folder is readable only by SYSTEM once the service has run; the removal
deletes it with backup and restore semantics, never by changing its ACL, and refuses any reparse point.

## Native build and verification

Use Windows x64 or ARM64 with Node 24+, stable Rust MSVC and the Visual Studio C++ Build
Tools/Windows SDK. See [Rust installation](https://rust-lang.org/tools/install/)
and [Microsoft setup](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup).
The TLS dependency's [Windows build requirements](https://aws.github.io/aws-lc-rs/requirements/windows)
explain NASM/prebuilt assembly objects for x64 and the clang-cl requirement for
ARM64. Include the Visual Studio LLVM compiler and LLVM toolset on ARM64.

Run the commands in [Windows CI](../../.github/workflows/windows.yml), including
its selected native tests. The workflow is prepared but has not run: this local
repository has no configured remote. Tests use disposable state and fake credentials;
they do not require reading the user's provider sign-in files.

```powershell
npm ci
npm run check
npm run test:web
cargo build --locked --bin miniclaw-server --example provider_fixture
cargo test --locked --test platform_fs --test secrets --test windows_permissions --test windows_startup --test windows_worker
cargo test --locked --lib powershell::tests
cargo test --locked --lib windows_process::tests
```

For a local package after native tests pass:

```powershell
cargo fetch --locked
node scripts/package-backend.mjs windows-package
```

Native acceptance must cover a fresh standard-user account, Unicode/spaced paths,
private-state denial to a second user, Credential Manager round trips, junctions,
hardlinks, path aliases, process descendants on expiry/revocation, provider login
and chat, restart/recovery and locked-executable updates. Blender and Roblox
Studio need actual installed applications; their presence is not inferred from
successful setup-script tests. Pass sandbox/file-policy and ConPTY acceptance before
enabling those capabilities. Desktop controls additionally need an interactive
Windows session and the supported provider helper.

## Platform evidence

- [PowerShell executable arguments](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1): encoded scripts are UTF-16LE; explicit text output avoids CLIXML diagnostics.
- [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) and [nested jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs): process-tree lifetime and assignment constraints. Jobs are not a file/network sandbox.
- [CreateProcess handle inheritance](https://devblogs.microsoft.com/oldnewthing/20200306-00/?p=103538): every concurrent launcher must restrict inheritance; a handle list on only one path is insufficient.
- [AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation): the token isolation boundary used by the Windows sandbox implementation.
- [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew): sharing, explicit security descriptors and reparse-point opening.
- [GetFinalPathNameByHandleW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew): normalized names from opened handles.
- [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew): per-user credential persistence and blob limits.

Independent reviews: [storage and shell gate](review-setup-storage.md),
[process creation](review-processes.md) and [sandbox](sandbox-review.md). Cross-compilation and Mac tests
do not replace any of the native acceptance checks above.
