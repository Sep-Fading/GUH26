# Windows process review

Reviewed the Windows process launcher, MCP launch path, provider launch paths and
executor dispatch on 2026-09-30. This is source review and cross-compilation evidence;
no Windows process was executed on this Mac.

## Finding corrected during review

The native launcher creates inheritable stdin/stdout/stderr handles and supplies an
explicit `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`. That restricts its own child, but another
thread using ordinary Rust `Command::spawn` can inherit those handles during creation.
The result can expose a tool pipe to another child or keep a pipe open after its intended
owner exits. Rust's internal spawn lock does not cover MiniClaw's direct `CreateProcessW`.
Microsoft describes this mixed-launcher race in
[Why do people take a lock around CreateProcess calls?](https://devblogs.microsoft.com/oldnewthing/20200306-00/?p=103538),
and the [Rust Windows process implementation](https://raw.githubusercontent.com/rust-lang/rust/stable/library/std/src/sys/process/windows.rs)
uses its private `CREATE_PROCESS_LOCK` with optional, not automatic, attribute lists.

All reachable Windows provider launches now use
[`provider_process`](../../server/src/provider_process.rs), which calls the explicit
native launcher in [`windows_process`](../../server/src/windows_process.rs).
The final source scan found ordinary process creation only in Unix-gated implementations.
Windows briefing checks also use the native launcher; image resizing and VM file panels
are gated off. `worker::serve_with_control` and capture helpers now refuse Windows calls.
That worker route belongs to the optional Mac VM-control integration; ordinary Windows
Codex launch uses RPC directly. These source changes close the identified mixed-launcher
path, but concurrent native execution remains a required test.

## Current process and tool behavior

- Both stdio and ConPTY creation supply `PROC_THREAD_ATTRIBUTE_JOB_LIST` to
  `CreateProcessW`. Assignment is atomic with creation; the primary thread stays
  suspended until sandbox records and any network lease are installed. This removes
  the crash interval between a suspended launch and `AssignProcessToJobObject` described
  by Microsoft's [atomic job-assignment guidance](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812).
- Jobs use an unpredictable name, an explicit actual-user owner and a protected DACL.
  This avoids default Administrators ownership under elevated tokens. Name collisions
  fail without modifying the existing object. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
  and explicit cancellation terminate descendants; breakaway is never enabled.
- Stdio retains an explicit inheritable-handle list. ConPTY uses noninheritable transport
  pipes and disables general handle inheritance. Attribute values remain allocated
  through `CreateProcessW`; owned guards release pipes, processes and native allocations.
- Capture drains both streams concurrently with bounded retained output. Root-process
  exit terminates contained descendants. Cancellation retains a queryable job guard so
  sandbox cleanup can wait for `ActiveProcesses == 0` before revoking temporary ACLs.
- Confined Windows MCP now uses AppContainer resources, suspended stdio, and the same
  authenticated network broker as PowerShell. Explicit `runtimePaths` are read-only
  installation grants saved during setup. Execution never derives grants from mutable
  `pyvenv.cfg` files. These grants also apply to Mac MCP through Seatbelt.
- The Windows terminal uses ConPTY with persistent PowerShell state and short OSC command
  markers. Windows PowerShell 5 explicitly receives `DefaultWebProxy`, with local bypass
  and default credentials disabled; environment variables alone only cover newer clients.
  See Microsoft's [Invoke-WebRequest proxy support](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/invoke-webrequest).
- Permission-checked Windows file operations and restricted shell execution are now
  implemented separately. The production sandbox availability gate remains closed
  pending native acceptance; compilation is not permission to enable these features.
  Timed unrestricted shell remains a separate explicit user grant.
- VM transfer and worker-control paths remain excluded on Windows.

## Network service and recovery

The narrow service validates actual named-pipe peer identity, an installation HMAC,
caller-owned job identity and every live process's AppContainer SID before installing
network policy. The client checks SCM service identity and a shared-key service proof.
The read-only readiness probe creates no job or lease and caches its result briefly;
execution always authenticates a fresh connection.

### Client authentication of the service (standard users)

The client never opens the SYSTEM broker process. A non-administrator has no rights on
that process (`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` returns error 5), and the
service does not widen its own process DACL. Identity is established from facts a standard
user can read but only an administrator can set:

1. `GetNamedPipeServerProcessId` on the connected handle gives the kernel-recorded PID of
   the process that created that pipe instance.
2. `QueryServiceStatusEx(SC_STATUS_PROCESS_INFO)` must report `SERVICE_RUNNING` with the
   same nonzero `dwProcessId`. Live PIDs are unique, so a squatting process cannot match.
3. `QueryServiceConfigW` must report `SERVICE_WIN32_OWN_PROCESS`, the `LocalSystem`
   account and exactly `"<service_exe>" --network-service "<config>"`. The SCM started
   that PID from this command line, which replaces the former image-path query. The
   default service security descriptor gives local authenticated users only query rights;
   `SERVICE_CHANGE_CONFIG` is administrator-only.
4. The pipe owner read through the client handle must be SYSTEM or Administrators. A
   standard user cannot assign either owner, and therefore cannot own (or re-ACL) the pipe.
5. The existing HMAC `Hello` proof over a fresh challenge and the boot epoch.

PID reuse: an instance created by a dead process whose PID the service later reuses
would need to exist before the service started. The service creates its pipe with
`FILE_FLAG_FIRST_PIPE_INSTANCE` and reports `SERVICE_RUNNING` only afterwards, so a
running service never coexists with an older instance; later instances need
`FILE_CREATE_PIPE_INSTANCE`, granted to SYSTEM only. Administrators remain trusted.

Sources: Microsoft Learn `GetNamedPipeServerProcessId`, `SERVICE_STATUS_PROCESS`,
`QUERY_SERVICE_CONFIGW`, "Service Security and Access Rights", `CreateNamedPipe`
(`FILE_FLAG_FIRST_PIPE_INSTANCE`, `FILE_CREATE_PIPE_INSTANCE`) and "Process Security and
Access Rights".

### Job-death attestation

The same connection accepts a second, separately signed question: is a named run dead?
The service checks that the job name is gone and that no running process carries the
run's AppContainer SID, and answers one boolean. It installs nothing and keeps no state.
Design, review and evidence are in
[sandbox-review.md](sandbox-review.md#service-attestation-and-hard-links-2026-10-04).

### Declared listeners are default-deny

Microsoft's "ALE Layers" page states that a bind without a local address is classified with
an `FWP_EMPTY` local address. The former "block when the local address is not 127.0.0.1"
filter therefore never matched a wildcard listen, and a sandboxed child could listen on
`0.0.0.0:<declared port>`. `FWPM_LAYER_ALE_AUTH_LISTEN_V4` and
`FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4` now each carry one package-only catch-all block and,
per declared port, one higher-weight exact permit: local address 127.0.0.1 and the port
(plus remote address 127.0.0.1 and TCP at the accept layer). A permit only exempts its exact
match from MiniClaw's own catch-all; it cannot override a block in another sublayer. IPv6
stays fully blocked. `ALE_RESOURCE_ASSIGNMENT` is deliberately not filtered: implicit binds
made by `connect()` are classified there too, and exposure is decided at listen/accept.

Native status (VM2): wildcard and port-0 listens are dropped by the catch-all and the
declared 127.0.0.1 listen is admitted, but the inbound loopback connection is still dropped
at the accept layer by a filter outside MiniClaw's lease. The declared-listener positive
path has never passed natively and remains open. Since 2026-10-05 a lease that declares listening ports
is refused at once as not supported on Windows yet; see
[sandbox-review.md](sandbox-review.md#declared-listeners).

Persistent filters remain in place on uncertain cleanup. Normal service shutdown first
terminates registered jobs, proves their active count is zero, removes exemptions and
filters, and only then reports a successful stop. Panic, failed installation or uncertain
cleanup produces nonzero service status. The service never launches arbitrary privileged
commands or modifies caller-selected filesystem ACLs.

Startup recovers inactive sandbox journals before opening the HTTP listener. A surviving
named job can be reopened only after owner, package SID and containment flags match,
then terminated and observed empty. In the service's own lease recovery an absent name does **not** count as proof
of process death: kernel namespace removal must not be confused with destruction of the
last object reference. The [job-object lifecycle documentation](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
describes termination before object destruction but does not establish that equivalence
for a failed name lookup. Uncertain simultaneous backend/service crash records therefore
remain intact and startup reports recovery is required. Automatic recovery of that rare
case still requires a definitive death/restart proof; no silent ACL rollback occurs.

## Backend fixes after the independent review (2026-10-04)

**Shutdown.** Every owner of a sandbox releases it before the cleanup queue is drained:
`executor::end_runs` stops one-shot shell and Git runs, `Terminals::close_all` closes
terminals, and `mcp_gateway::Hub::close_all` drops each MCP connection. The hub sets its
closing flag first, so a call that still holds a slot cannot reconnect, and a call in
flight is dropped instead of awaited. A terminal opened after `close_all` is not covered.

**Availability gate.** `executor::sandbox_available()` is the one Windows check. The
Changes panel and the briefing survey call `executor::workspace_git`, which returns
nothing while the gate is closed, so Changes shows the conversation view and no ACE is
written. `Link::open` refuses a confined stdio MCP server in the same state. Discard
answers `discard_unavailable`. `execute_git`, `execute_host` and `Link::connect` stay
ungated for native acceptance tests.

**Batch files.** `provider_process::native_image` accepts only `.exe` and `.com`, matched
on the exact leaf name. `windows_process::spawn` and `conpty_pending` call it before
`CreateProcessW`. A `CODEX_BIN` or `CLAUDE_BIN` that is not an absolute native executable
stops startup with an error that names the variable. Microsoft documents that a batch file
needs an explicit `cmd.exe /c`; the Rust advisory for CVE-2024-24576 describes the
argument re-parsing this avoids. Setup already selects `.exe`/`.com` only
(`scripts/setup/system.mjs`).

**New file names.** A name that does not exist yet may be any Unicode name. It is refused
when it has the shape of a generated 8.3 alias: a base of at most eight characters ending
in `~` and digits, at most one period, an extension of at most three characters, no space
([MS-FSCC] 2.1.5.2.1). An existing alias is opened and judged by object identity.
`guarded_create` also requires the created object's final name to equal the request.
Blocked names that do not exist yet are compared by `windows_names_may_match`, which
treats any two different non-ASCII UTF-16 units as a possible case pair. A short name set
by hand with `SetFileShortNameW` has no recognisable shape; only the identity check covers it.

**Computer bridge.** During a helper call or an approval the bridge answers `ping`, queues
other requests (32 at most), and honours `notifications/cancelled` for the running request
by sending no response. The native call is allowed to finish first, so the helper never
runs two actions. Only end of input interrupts a call.

**Review prompt.** A new file's folder and name are split at the last `\` or `/` on
Windows. A listing that fails there refuses the write; names compare without case.

**SQLite.** On Unix the database check closes its descriptor before SQLite opens the file
(SQLite, "How To Corrupt An SQLite Database File", section 2.2). Windows keeps the pin.

## Windows paths and the service journal (2026-10-04)

**Project folders.** `std::fs::canonicalize` answers `\\?\C:\...` on Windows, so the exact
match in `save_project` refused every folder. `permissions::exact_folder` now compares the
folder with `platform_fs::permission_path`, the normalizer the Windows sandbox applies to its
grants: the plain drive form of the final path, links refused. The match stays exact, so a
verbatim, lower-case, forward-slash, 8.3 or junction spelling is still refused. The Mac keeps
its `canonicalize` comparison.

**Protected paths.** `seatbelt::windows_is_protected` treats a `\\?\X:\` prefix as the plain
drive path on both sides, so `rule_files` (which resolves files with `canonicalize`) no longer
reads `AGENTS.md` or `CLAUDE.md` from inside a protected root.

**Mac file names.** On the Mac and in a VM a workspace path again refuses only NUL and newline,
as in baseline `20595a9`; Windows still refuses every control character.

**Service journal.** Once the network service has run, its `leases` folder is owned by SYSTEM
and only SYSTEM may open it. `windows-network-update.ps1` first lists it normally and, when
that is refused, counts it with backup semantics: it enables `SeBackupPrivilege`, opens the
folder with `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`, refuses a reparse
point, lists it through `GetFileInformationByHandleEx(FileFullDirectoryInfo)` and restores
the previous privilege state. The ACL and owner are never changed. Microsoft documents that
SeBackupPrivilege grants all read access regardless of the ACL, SeRestorePrivilege grants
DELETE, and that `FILE_FLAG_BACKUP_SEMANTICS` makes the system override file security checks
for a holder of those privileges (Privilege Constants; CreateFileW). The install script keeps
its plain listing: its own service that never started cannot have written the journal, so
an unreadable folder there is an explicit refusal (the elevated payload also has no room for
the reader under the 30,000-character command limit). `windows-ci-stop-network.ps1` removes
the data folder with backup and restore semantics, only for the exact
`ProgramData\MiniClawNetwork-<32 hex>` folder. It checks the whole tree first, refuses
any reparse point before deleting anything, and deletes each entry through its own handle
(`FileDispositionInfoEx`, ignoring a read-only attribute). `-LeftoverDataOnly` removes the data folder of a service that is
already unregistered and has no program folder.

**Codex turn failures.** A failed turn now names the provider's reason from `turn.error`
on `turn/completed`, or from a final `error` notification (`willRetry: false`); both carry a
`TurnError { message, codexErrorInfo, additionalDetails }` (codex-rs app-server-protocol v2).
`usageLimitExceeded` becomes "Codex usage limit reached. Try again at ..." with the time the
provider gave. Another reason is appended to the old notice only when it is one short plain
sentence without an address, query string or identifier-like run. Otherwise, and when no
reason is given, the notice stays "Codex could not complete this response."

## Working folder, Trash and new names (native sweep, 2026-10-04)

**Working folder.** The backend's chat folder (`backend.cwd`) is now the plain drive form of
its canonical path (`platform_fs::plain_disk_form` removes only a `\\?\X:\` prefix; `\\?\UNC\`
and device forms stay as they are). Codex, Claude and every shell run there, and the folder is
the thread `cwd` and the history filter. Rust documents that `canonicalize`'s verbatim form "may
be incompatible with other applications". Native probe on VM2 (`native-sweep-vm2-focus-red.log`):
started in `\\?\C:\...`, `cmd.exe` printed "UNC paths are not supported. Defaulting to Windows
directory" and ran in `C:\Windows`; Node's `process.cwd()` and PowerShell report the verbatim
form; BusyBox reports `//?/C:/...`. Started in the plain form all four report `C:\...`.
Saved tasks keep working: `history::workspace_spellings` gives both spellings, a summary
matches either, and `thread/list` receives both, since the App Server's `cwd` filter takes one
path or an array of exact paths (OpenAI, Codex App Server, `thread/list`). A started Claude chat
whose CLI session was filed under the verbatim folder name resumes in that folder
(`claude::resume_cwd`); deleting a chat removes the session under both names. The macOS folder
has one spelling and is sent exactly as before.

**Trash.** A deleted conversation's uploads move to the recovery folder. On Windows a missing
recovery folder is still created private; an existing one must be a real folder, not a link,
but need not be private itself: the moved uploads folder keeps its own protected owner-only
access list. Native evidence: the old check refused an existing inherited folder (owner
Administrators under the elevated test user), so the upload stayed served (200); after the move
the folder's list was still `S:(OI)(CI)(F)` only.

**Diagnostic output.** No tracing subscriber was installed, so `tracing::warn!` and `error!`
messages such as "Recovery storage could not be secured." were never printed, on Windows or on the
Mac. `logging::install` now writes MiniClaw's own events as plain text to stderr, which the
launcher (`start-host.mjs`) passes to the terminal on both platforms. `MINICLAW_LOG` takes one level
word (`off`, `error`, `warn`, `info`, `debug`, `trace`); any other value stops startup with an error
naming the variable. Unset, Windows prints warnings and errors and macOS prints nothing, as before.
The filter is a `Targets` filter for `miniclaw_server` only, so HTTP and TLS crates, which can log
headers, never reach the output, whatever the level (tracing-subscriber, `filter::Targets`). The
events carry error values, profile names and SIDs, never secrets or chat text. The network service's
own events still go nowhere: a service has no console.

**New names.** `tests/windows_permissions.rs` now states the rule above ("New file names"):
valid Unicode names are accepted and keep their own spelling under the normalized parent;
generated 8.3 alias shapes, device names, streams and trailing dots or spaces are refused;
blocked new names are related by `windows_names_may_match`.

## Evidence and native acceptance

Host protocol, PowerShell encoding/proxy construction and explicit runtime-grant tests
pass. Windows GNU `cargo check --lib --tests` compiles the current native implementation
and tests. None of that executes Windows APIs.

Native tests cover Unicode/output/status, plain stderr, environment filtering, interactive
stdio, descendant cancellation, unrelated pipe EOF, ConPTY markers/state, restricted MCP
file/network denial, job ownership/recovery, and real PowerShell HTTP through a local test
proxy. CI includes AppContainer ACL/journal tests and an additional disposable standard-user
account so elevated-runner behavior cannot substitute for ordinary installations. Test
filters assert a nonempty selection. WFP tests run separately with elevation. A Windows
startup test checks that an unresolved journal stops serving and preserves its evidence.

Native installation and execution, including the rare crash/restart recovery case, remain
acceptance work until the Windows guest or runner produces results. A job contains process
lifetime; it cannot undo persistent effects of explicitly unrestricted commands.
