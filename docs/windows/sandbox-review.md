# Windows isolation implementation review

The native broker opens every component through `NtCreateFile.RootDirectory`. It retains
ancestor handles until the operation ends, rejects reparse points and multi-link files,
and checks existing grants against the full 64-bit volume serial and 128-bit file identity. A containing grant never defeats
a nested block. Newly created files require a pinned authorized parent and `FILE_CREATE`.
Blocked names are additionally compared conservatively across ASCII case variants.

The process sandbox uses a unique AppContainer package SID per run and LPAC startup
attributes. Existing host objects receive ACEs through checked handles. Approved writable
directories pass the run SID to new descendants; their identities are journaled before
inheritance is enabled. The disposable scratch directory also inherits its ACE. No network capability is
added. Host folders overlapping protected or blocked paths are refused before launch.
Write grants include content changes and deletion of individually approved objects.
Explicit roots remain pinned while approved descendants can be renamed. Native BusyBox
shell and persistent terminal file-boundary tests pass. Sandboxed commands use the pinned
BusyBox runtime; timed unrestricted commands use PowerShell.
Preparation refuses more than 100000 objects, 64 levels or 120 seconds with an actionable
narrower-folder error. A 6000-file native acceptance fixture exercises the former size limit.

The elevated service owns only network restrictions. Persistent WFP filters block all IPv6. Outgoing IPv4 TCP is restricted to the exact
loopback proxy and separately approved local-client ports. TCP listening and incoming
connections require separately approved ports and the exact 127.0.0.1 address. The filter transaction commits before the package receives a loopback exemption.
Inbound grants additionally use the fixed System32 CheckNetIsolation utility with a
validated AppContainer profile. Its native readiness behavior remains to be verified.
Exemption removal precedes filter deletion, and deletion requires the service's proof
that the associated job has stopped. Dropping the native lease retains the restrictions;
there is no automatic stale cleanup that could open a live process's network access.

## Review findings addressed

- Replaced uppercase positive-path matching with native object identity matching.
- Replaced absolute ancestor/leaf reopen sequences with parent-handle-relative opens.
- ACL changes use `NtSetSecurityObject` on the checked handle to avoid the high-level
  API's recursive inheritance propagation. Native tests pass for blocked child ACLs,
  new descendants, moved descendants and prelaunch orphan cleanup.
- New-file policy now checks parent identity before creation, including nested blocks.
- Preserved unique-SID restrictions across service crashes using persistent WFP filters.
- Kept caller-supplied executable launches and arbitrary ACL changes out of the service protocol.
  The only elevated helper is the fixed Windows network-configuration utility.

## Verification limits

Host tests cover deny precedence, exact-file scope and drive-root scope. Native ARM64
tests also pass for filesystem mutation, shell boundaries, persistent terminal state
and inherited ACL cleanup. Network initialization and Git path normalization still
fail native acceptance. The production sandbox availability gate remains disabled.

Normal cleanup now waits for the owned job to have no active processes before removing
its ACEs and profile. Private journals flush object identities before ACL changes and job
names before launch. Recovery skips live locks, malformed records and bound jobs without
proof of termination. Unbound setup records revoke only the recorded SID on matching
object identities. `OpenFileById` follows moved objects without trusting their former names;
verified deleted IDs need no ACL removal. Scratch lives under the protected data directory
and cleanup pins its root until only empty-directory removal remains. Existing named jobs
require matching owner/package identities and termination before replay. Missing job names
still retain their records without a termination proof. Named global mutexes serialize ACL updates across
same-user backends and loopback updates across elevated broker services.

Native interrupted-reboot recovery now passes against the preserved original fixture.
Windows returned error87 for some deleted or replaced file identities; that error is
not treated as proof of deletion. Recovery first proves job termination, revokes
reachable matching objects and inherited entries, removes scratch, and successfully
deletes the profile. It then flushes a terminal certificate and moves the unchanged
evidence into the private `retired/` directory with explicit `UnknownIdentity` entries.
The retired profile name cannot be reused. Historical ACL removal is not claimed.

[Journal recovery](../../server/src/windows_sandbox_journal.rs) validates archives at
startup and explicit recovery, outside the active-record limit. Each new launch checks
active records and refuses reuse of any archived profile name, without rescanning the
whole history. Archive readers share read access while denying writes and deletion.
Native regressions cover concurrent readers and this admission boundary, plus malformed
and inconsistent certificates, profile-deletion failure, retry after interrupted
archival, repeat recovery and more than1000 archived records. A real LPAC test proves
that a fresh sandbox cannot read a sentinel accessible through the old SID's retained
ACE; the old-token positive control succeeds.

## Network capability decision (2026-10-04, decided: see "Winsock catalog access for networked runs")

No capability has been added to the production token. Every sandboxed run still starts as an
LPAC with zero capabilities, and `exact_proxy_only` still fails natively: `WSAStartup` returns
10107 because the child cannot open the Winsock catalog keys.

Measured on the second ARM64 guest (VM2, Windows 11 26100) under unchanged WFP restrictions:

| Token | Winsock | Approved proxy | Other endpoint | Registry |
| --- | --- | --- | --- | --- |
| none | 10107 | not attempted | not attempted | every open denied |
| `internetClient` | 10107 | not attempted | not attempted | every open denied |
| `registryRead` | 0 | denied (10013) | denied | see below |
| both | 0 | reachable | denied | see below |

Microsoft documents only that an LPAC "cannot open any keys in the registry unless it has the
*registryRead* capability". The native DACLs show what that capability carries. It is the
same ACE set that ordinary AppContainer apps receive through `ALL APPLICATION PACKAGES`:

- All of `HKLM` that standard users can read, read-only. A synthetic `HKLM\SOFTWARE` key with
  an inherited ACL was readable; creating a subkey was denied.
- The `HKCU` and `HKCU\Software` keys themselves, so subkey names (installed software) can be listed.
- Inheritable read on `HKCU\Environment`, `Control Panel`, `Keyboard Layout`, `EUDC`,
  `Software\Classes`, `Software\Policies`, and under `Software\Microsoft` at least
  `Internet Explorer`, `Windows\CurrentVersion\Explorer`, `Internet Settings`, `Policies`,
  `Shell Extensions`, `Themes` and `WinTrust`.

It does not carry ordinary application settings: a synthetic `HKCU\Software\<name>` key with
the inherited default ACL stayed denied, as did the owner-only `HKCU` and `HKLM` sentinels,
the outside-grant file and protected MiniClaw state.

`HKCU\Environment` is the blocker. It holds the user's persistent environment variables,
where developers commonly keep provider keys (`setx`). The launcher deliberately strips the
child environment to an allowlist; `registryRead` would let a networked command read the same
values from the registry and send them to an approved host. Explorer history and proxy
settings are also exposed. The capability was therefore not adopted. Options for the lead:

1. Accept `registryRead` for networked runs only, and document that such runs see the same
   registry as any Store app, including persistent user environment variables.
2. Install-time custom capability: the elevated service adds a read ACE for a MiniClaw-specific
   capability SID to the `HKLM` root key and the Winsock/Tcpip parameter keys. This exposes no
   `HKCU` data but changes ACLs on global objects, which the current rules forbid, and it is unverified.
3. Run networked commands under a dedicated sandbox account whose own hive is empty. Larger redesign.

Network limits with both capabilities held in the offline and proxy modes of
`capability_network_boundaries`: only the exact IPv4 loopback proxy port was reachable; a
second loopback port, `127.0.0.2`, `::1`, UDP and all listening were denied, also for a
descendant process. The listener mode failed: the child could bind and listen on
`0.0.0.0:<declared port>`. Whether a non-loopback peer can connect is unverified; this must be
resolved in the WFP filters before any networked capability ships.

`offline_run_stays_closed` is the production-path regression for runs without approved
hosts: zero capabilities, no registry key opens, no outside or protected file, no socket.

Evidence: `capability-exposure-vm2.log`, `network-boundaries-vm2-listener-red.log` and
`network-socket-vm2-red.log` in `work/windows-vm`. Sources:
[LPAC and registryRead](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer),
[capability SIDs in registry ACLs](https://learn.microsoft.com/en-us/troubleshoot/windows-server/windows-security/sids-not-resolve-into-friendly-names),
[DeriveCapabilitySidsFromName](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-derivecapabilitysidsfromname).

### Narrow Winsock ACL experiment (2026-10-04, VM2, diagnostic only)

Result: Winsock works in a zero-`registryRead` LPAC with read ACEs for one custom capability
SID on exactly 29 `HKLM` keys, and every negative control passes. Nothing in production
changed; all DACLs were restored and verified.

Measured on VM2 (Windows 11 ARM64 26100.9457, native ARM64 child, unchanged WFP lease) with
the token `internetClient` + `miniclawWinsockRead.diagnostic-5f0c2d7e`
(`DeriveCapabilitySidsFromName`, SID `S-1-15-3-1024-546099102-…-2124216421`):
`WSAStartup` 0, TCP connect to the approved loopback proxy succeeded, `localhost` resolved.

| Key (under `HKLM`) | Count | Right needed |
| --- | --- | --- |
| the `HKLM` root key itself | 1 | `KEY_QUERY_VALUE` (0x1) |
| `SYSTEM\CurrentControlSet\Services\WinSock2\Parameters` | 1 | `KEY_READ` (0x20019) |
| `…\WinSock2\Parameters\Protocol_Catalog9` and its `Catalog_Entries64` | 2 | `KEY_READ` granted; 0x19 sufficed |
| every entry under `Protocol_Catalog9\Catalog_Entries64` | 14 | `KEY_READ` |
| `…\WinSock2\Parameters\NameSpace_Catalog5` and its `Catalog_Entries64` | 2 | `KEY_READ` granted; 0x19 sufficed |
| every entry under `NameSpace_Catalog5\Catalog_Entries64` | 5 | `KEY_READ` |
| `SYSTEM\CurrentControlSet\Services\WinSock\Parameters` | 1 | `KEY_READ` |
| `…\Services\WinSock\Setup Migration\Providers`, `…\Providers\Tcpip`, `…\Providers\Tcpip6` | 3 | `KEY_READ` |

All ACEs were explicit and non-inheritable. The entry counts (14 and 5) are this machine's
catalog; the rule is "every entry". IPv4 alone needs 28 keys (without `Providers\Tcpip6`).

Why the earlier 49-key experiment failed: the missing key was the `HKLM` root, not the
principal. With ACEs only on the catalog keys, `NtOpenKey` on the full path already succeeded
for both the package SID and the capability SID, but every `RegOpenKeyEx(HKEY_LOCAL_MACHINE, …)`
returned 5, because advapi32 must first open `\REGISTRY\MACHINE` for the predefined handle and
the LPAC had no access to that key. Intermediate keys (`SYSTEM`, `CurrentControlSet`,
`Services`, `WinSock2`) need no ACE and stayed denied; the `CurrentControlSet` link was not an
obstacle. `AFD`, `AppId_Catalog`, `Tcpip\Parameters` and `Dnscache\Parameters` need no new ACE
(the Tcpip parameter keys were already readable to the `internetClient` LPAC).

Which principal: both work. The package SID and a held capability SID each satisfied the
AppContainer half of the access check, and the same ACEs for a capability the token did not
hold gave nothing. That matches Microsoft's description that access must be granted "through a
Package SID or Capability SID", and that an LPAC additionally loses the `ALL APPLICATION
PACKAGES` grants. The capability SID is the one to use, because it is fixed at install time;
the package SID is new for every run and would need an elevated ACL write per run.

Ablation (each a separate run of the same child):

- No root ACE: `WSAStartup` 10107. Root with only `KEY_QUERY_VALUE` or only `READ_CONTROL`: works.
- One protocol entry missing: 10107. One namespace entry missing: 11003. Entries with query-only: 10107.
- `WinSock2\Parameters` without `READ_CONTROL` (0x19): 10107.
- No `WinSock\Parameters`, or query-only on the three helper keys: `WSAStartup` 0 but socket creation fails with 5.
- No `Setup Migration\Providers`: socket creation fails with 10041.
- Custom capability without `internetClient`: `WSAStartup` 0, socket creation fails with 10047.

Negative controls with the 29-key token, all denied: `HKCU` root, `HKCU\Environment`,
`HKCU\Software` (open with any mask, and enumeration), `HKCU\Software\Classes`,
`HKU\.DEFAULT`, `HKLM\SOFTWARE`, `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`, an
inherited-ACL `HKLM\SOFTWARE` sentinel, an owner-only sentinel, `HKLM\SYSTEM`, `…\Control`,
`Session Manager\Environment`, `…\Services`, `SAM`, `SECURITY`, `HARDWARE`, enumeration of the
`HKLM` root, creating a subkey in the catalog, the outside-grant file (read and write), a
second loopback port, `127.0.0.2`, `::1` and listening. The same child with `registryRead`
instead showed all of the `HKCU` and `HKLM\SOFTWARE` controls exposed.

Method and restoration: the test changes each DACL with `RegSetKeySecurity` on the exact key
(no propagation), after writing a durable record of every before/after descriptor; restore
uses the original descriptor bytes and asserts SDDL and byte equality per key. A set/restore
self-test on synthetic keys runs first. The runner also compares owner, group and DACL of 142
keys (root, `SYSTEM`, `SOFTWARE`, `Services`, and the `WinSock2`, `WinSock`, `AFD`, `Tcpip`,
`Tcpip6`, `Dnscache`, `NetBT`, `nsi` subtrees) before and after: identical in every run, and no
key still named the diagnostic SID. `HKCU` was never modified. Winsock's own opens were listed
with an ETW registry trace of the child.

Proposed install-time design (not implemented, needs the lead's decision because it changes
ACLs on global objects):

- The elevated network service installer derives one install-specific capability SID, adds the
  ACEs above, records them durably, and removes exactly those ACEs on uninstall. Networked runs
  get `internetClient` plus this capability; offline runs stay at zero capabilities.
- What changes globally: one more read ACE on 29 system registry keys, including the `HKLM`
  root. A capability SID is not a secret, so any local process that can start an AppContainer
  can name it; the effective grant is "AppContainers that ask for it may read the Winsock
  catalog", which ordinary users and regular AppContainers can already read. No `HKCU` data,
  no value on the root key, and no enumeration of `HKLM` is exposed.
- Risks: the catalog entry keys are rewritten by `netsh winsock reset` and by VPN or LSP
  installers, so explicit ACEs go stale and networked runs then fail closed with 10107; the
  service must verify and repair the ACEs at start and before granting network. Windows
  servicing may reset these DACLs. Editing system-key DACLs can also conflict with security
  products.

Not verified: persistence of the root-key ACE across a reboot (no reboot was allowed); an
inheritable ACE on `WinSock2\Parameters` as an alternative to per-entry ACEs; x64 Windows,
x86 children (`Catalog_Entries` instead of `Catalog_Entries64`) and other Windows builds;
UDP, real DNS and non-loopback traffic; real tools (Git, curl, Node) under this token.

Evidence in `work/windows-vm` (all VM2): `winsock-acl-vm2-final.log` (baseline and the 29-key
set with assertions), `winsock-acl-vm2-explore1.log` and `-explore2.log` (principals, root key,
ETW traces), `-explore3.log` and `-explore4.log` (ablation). Test:
`server/tests/windows_winsock_acl_diagnostic.rs` (ignored; `winsock_acl_restore` is the
recovery entry). Sources:
[AppContainer and LPAC access model](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer),
[DeriveCapabilitySidsFromName](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-derivecapabilitysidsfromname),
[capability SIDs on registry ACLs, including third-party ones](https://learn.microsoft.com/en-us/troubleshoot/windows-server/windows-security/sids-not-resolve-into-friendly-names),
[Chromium sandbox design, LPAC](https://chromium.googlesource.com/chromium/src/+/main/docs/design/sandbox.md),
[RegSetKeySecurity](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regsetkeysecurity).

## Winsock catalog access for networked runs (2026-10-05)

The user chose option 2 of the decision above. A networked run now carries `internetClient` and
one install-specific capability; the network service grants that capability read access to the
Winsock catalog keys and nothing else. `registryRead` is not used. Runs without approved hosts
keep a token with no capability. `seatbelt::available()` is unchanged.

### What changes on the machine

One explicit, non-inheritable allow entry for the installation's capability SID on these keys
under `HKLM`, and no other change to any global object:

| Key | Rights |
| --- | --- |
| the `HKLM` root key | `KEY_QUERY_VALUE` (0x1) |
| `SYSTEM\CurrentControlSet\Services\WinSock2\Parameters` | `KEY_READ` (0x20019) |
| `…\WinSock2\Parameters\Protocol_Catalog9`, its `Catalog_Entries64`, every entry below it | `KEY_READ` |
| `…\WinSock2\Parameters\NameSpace_Catalog5`, its `Catalog_Entries64`, every entry below it | `KEY_READ` |
| `SYSTEM\CurrentControlSet\Services\WinSock\Parameters` | `KEY_READ` |
| `…\Services\WinSock\Setup Migration\Providers`, `…\Providers\Tcpip`, `…\Providers\Tcpip6` | `KEY_READ` |

Both test guests have 14 protocol and 5 namespace entries, so 29 keys. Entries are listed at
run time; a name that is not twelve digits, or more than 256 entries in one catalog, refuses the
whole pass. The three `WinSock` helper keys below `Parameters` are skipped when absent; every
other key is required. Owner, group and audit list are never written, and nothing in `HKCU`.

Only `Catalog_Entries64` is granted. Microsoft documents that on a 64-bit computer every Winsock
call that is not a `…32` call uses the native 64-bit catalog, so 64-bit children read this view,
on x64 and ARM64. MiniClaw's bundle holds 64-bit programs only (BusyBox, Git, Node per
architecture; x64 Python). The 32-bit `Catalog_Entries` view is deliberately not managed
(`managed()` refuses it), so a 32-bit child of a user-configured server gets `WSAStartup` 10107
in a networked run. Adding the view is one constant and needs a decision, because it goes beyond
the approved key list.

### Design

- **Capability name.** `miniclawWinsockRead.<32 hex install id>`, taken from the service name in
  the service's protected configuration
  ([`capability_name`](../../server/src/windows_winsock_acl.rs)). It is not stored as a second
  field: the backend reads the same service name from its private client configuration, so the
  two cannot disagree, and no file can name another capability such as `registryRead`. The SID
  comes from `DeriveCapabilitySidsFromName` and must have the custom-capability shape
  `S-1-15-3-1024-` plus eight words before anything is written.
- **Who writes.** Only the SYSTEM service
  ([`Manager`](../../server/src/windows_winsock_acl_native.rs)). It runs
  `ensure` when it starts, before it reports Running, and again before every lease.
- **Before-record.** `ProgramData\MiniClawNetwork-<id>\winsock-acl\record.json`, in a folder only
  SYSTEM can open. Each key's whole descriptor (bytes and SDDL) is written and flushed before
  that key's first change, and again when a recreated key differs. The first snapshot is kept.
- **Adding.** Under the machine-wide MiniClaw lock, each key is opened by name, and the kernel's
  name of the opened handle (`NtQueryKey`) must equal the requested path, with `CurrentControlSet`
  allowed to resolve to `ControlSet<nnn>`. The key's access list is copied with one entry
  inserted after the explicit entries, and set with `RegSetKeySecurity` on that handle with a
  descriptor that carries the access list only. Protection and auto-inherit markers are kept.
  A second pass that finds the exact entry changes nothing.
- **Repair.** A missing or altered entry is put back and logged
  (`winsock-acl\events.log`, and `winsock-acl-status.json` beside the configuration, which
  administrators can read). If any key cannot be verified the lease is refused with
  "Network access for sandboxed commands is unavailable: … The command was not started", the
  job is stopped, and no filter or loopback exemption is installed. The service keeps running
  for job attestation.
- **Removal.** Entries for this SID are deleted from every managed key that exists, then read
  back. Recorded descriptors are never written back, so another installation's entry or a later
  change by Windows stays. The result says which keys now equal their recorded bytes. Removal is
  triggered by starting the stopped service with the parameter `winsock-acl-remove`; the service
  removes, publishes the result and stops. Microsoft's default service security gives
  `SERVICE_START` to administrators and SYSTEM only.
- **Scripts.** The install script fails, and rolls back including removal, unless the status
  file says `ok` after the service reports Running. The update script restarts the service,
  which runs the same verify-and-repair. `windows-ci-stop-network.ps1` runs the removal, then
  checks the root key and every key under `Services\WinSock*` itself for the SID before it
  deletes the service. There is no product uninstaller for the network service yet; it must
  perform the same two steps.
- **Token.** [`Sandbox::enable_network`](../../server/src/windows_sandbox_native.rs) adds the
  two capabilities; `sandboxed_shell`, the terminal and the MCP client call it only when a
  proxy lease will follow. The service refuses a lease for a job whose processes hold any
  capability other than these two.

### Adversarial review

- **Who can use the entries.** A capability SID is not a secret; any local program that may
  create an AppContainer process can name it. What it gains is read access to the Winsock
  catalog, which every ordinary user process and every non-LPAC AppContainer already has. The
  root entry allows `KEY_QUERY_VALUE` on the root key itself: it has no values, and listing
  subkeys stays denied (measured).
- **A link in the catalog.** A registry link in place of an entry key would redirect the open.
  Creating one needs write access to the catalog (administrators). The handle's kernel name is
  checked before any change; the native test plants a link to a key elsewhere and the pass
  refuses it, leaving the target untouched.
- **Tampered record.** The record is never a source of data to write. Paths read from it must be
  in the managed list; the worst a forged record does is make removal look at a managed key.
- **Lost update between two installations.** Read, modify and write happen under the
  `Global\MiniClaw.NetworkIsolation` mutex that only SYSTEM and administrators can open. A writer
  outside MiniClaw can still race; the entry is re-read after the write and checked again before
  the next lease.
- **Partial failure.** Keys corrected before a failing key keep their entry. That grants
  nothing usable (Winsock needs all of them) and the lease is refused.
- **Capabilities without filters.** `internetClient` without the service's filters would reach
  the internet. Every call site creates the process suspended, obtains the lease, then resumes;
  a failed lease drops the pending process, whose job is kill-on-close. This order is enforced
  by convention at three call sites, not by types.
- **Removal trigger.** A start parameter needs `SERVICE_START`. A standard user cannot start the
  service at all. An administrator who runs it only makes networked runs fail until the next
  start.
- **Status file.** It sits in a folder only SYSTEM and administrators can write. The service
  never reads it.

### Evidence

All logs are in `work/windows-vm`; each prints the guest, the hash of every source it verified
against the host tree (279 files), `Compiling miniclaw-server`, and build times after the run
began. Both guests are Windows 11 ARM64 26100; VM1 has UAC off, VM2 has UAC on.

- **Red before.** `exact_proxy_only` failed with `WSAStartup` 10107 in every earlier run, last in
  `native-sweep-vm2-rust-2.log`.
- **Production path, elevated owner, VM1** (`winsock-prod-vm1-main-3.log`): 12 of 12 steps.
  Install added 29 entries. `machine_keys_carry_exactly_the_service_entries`: 29 keys exact,
  root `0x1`, others `KEY_READ`, flags empty, neighbours without the SID.
  `offline_run_stays_closed` (no capability, registry denied, 10107). `exact_proxy_only`.
  `networked_run_stays_closed_elsewhere`: token holds exactly the two SIDs; proxy reached;
  second loopback port, `127.0.0.2`, `::1`, `1.1.1.1:443`, and listening on `127.0.0.1`,
  `0.0.0.0` and `::1` all 10013; no datagram arrived at a loopback or LAN receiver; `HKCU` root,
  `Environment`, `Software`, `Software\Classes`, `Software\Microsoft`, `Volatile Environment`,
  `HKU\.DEFAULT`, `HKLM\SOFTWARE` and three keys below it, `SYSTEM`, `Control`,
  `Session Manager\Environment`, `Services`, `LanmanServer`, `Services\WinSock2`,
  `Services\WinSock`, `SAM`, `SECURITY`, `HARDWARE` denied through Win32 and NT opens with four
  masks; root listing denied; registry writes denied; outside-grant file and protected state
  denied. `dropped_catalog_entry_is_repaired_before_the_lease`: the entry was deleted from a
  real catalog key, the next lease repaired it, the key equals its earlier bytes, and the
  status file names it. Service restart: nothing to repair. Networked MCP server and terminal
  tests pass. Removal: 29 keys stripped, 29 identical to their recorded descriptors; the
  runner's own before/after comparison of 65 keys shows 0 differences.
- **Same on VM2 with UAC on** (`winsock-prod-vm2-main-1.log`): 12 of 12, removal 29 of 29, 0 of
  65 keys differ.
- **Standard user, VM2, UAC on** (`winsock-prod-vm2-standard-1.log`): the whole sequence, 69
  tests passed, none failed, including `offline_run_stays_closed`, `exact_proxy_only` and
  `networked_run_stays_closed_elsewhere` as the non-administrator account; service, profile and
  account removed; 0 of 65 keys differ.
- **Manager on a synthetic key tree, VM1** (`winsock-prod-vm1-synthetic-3.log`): 7 of 7:
  entries land on managed keys only; recreated, rewritten, new and wrong entries are repaired;
  removal restores every descriptor byte for byte; two installations are independent in both
  removal orders; a later change by something else survives removal; an unwritable key, an
  unexpected entry name and a missing catalog fail the pass; a link to another key is refused.
- **Two installations on the real keys** happen in the standard-user sequence only when it runs
  inside `windows-native-acceptance.ps1` (owner service plus standard-user service). That
  script was not run as a whole.
- **Host**: 17 unit tests for the key list, access-list editing, record and messages; Windows
  GNU `cargo check --all-targets`; `npm run test:setup`.

### Real tools (VM1, `winsock-prod-vm1-tools-4.log`)

- The pinned shell's `nc` reaches the proxy and gets its 403 for an unapproved host; another
  port is refused. A sandboxed stdio MCP server with an approved host does the same and
  resolves its host name through Winsock (`networked_mcp_server_reaches_only_its_proxy`).
  The older fixture called `nc -w 2`; this BusyBox build has no `-w`, so its "network denied"
  result was always true. Both fixtures now call `nc` without it, and the parent's listener
  remains the deciding check.
- **Bundled Git over HTTPS fails**: `fatal: unable to access …: Insufficient randomness`. The
  bundled `libcrypto-3-arm64.dll` (OpenSSL 3.6.5) imports `CryptAcquireContextW` and
  `CryptGenRandom` and not `BCryptGenRandom`. In the sandbox token `CryptAcquireContext` fails
  with 0x80090017 (it reads provider registration from the registry) while
  `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)` and every tested CNG algorithm provider
  succeed (measured, `INFO crypto` line). OpenSSL uses CNG only when built with
  `_WIN32_WINNT >= 0x0600`. Rebuilding the bundled OpenSSL with that define should fix Git; this
  was not done here. `git --version` and `git init` work.
  **Update 2026-10-05 (git-https agent): fixed in the lpac4 Git runtime** (OpenSSL rebuilt with
  `_WIN32_WINNT=0x0A00`, [git-runtime.md](git-runtime.md), "HTTPS inside the sandbox"). VM1,
  `work/windows-vm/git-https-native-vm1-green-2.log`: from a folder that is not a repository, bundled Git
  `ls-remote https://github.com/git/git.git HEAD` through the approved proxy returns the HEAD line;
  `expired.badssl.com` and `self-signed.badssl.com`, approved, are refused by certificate verification with
  the bundled CA ("certificate has expired (10)", "self-signed certificate (18)"); `gitlab.com`, not approved,
  gets the proxy's 403 and is named in `refusedHosts`; a run without approved hosts has no network
  (`curl_global_init failed`). Red before the fix on the same build: `git-https-native-vm1-red-1.log`.
- **Node** aborts at startup: offline with `WSAStartup` 10107 (Node initialises Winsock for
  `--version`), networked with `Assertion failed: ncrypto::CSPRNG` (the same random source).
- **x64 programs under ARM64 emulation do not start in the sandbox at all**: x64 Node and x64
  Python exit with 0xC0000017, offline, started directly. ARM64 Python 3.13 exits with
  0xC0150002 (side-by-side configuration). These are independent of the network change and
  mean the bundled x64 Python tools cannot run sandboxed on ARM64 as things stand. x64 hosts
  are untested.
- Git outside a repository stops at the first parent folder it may not read
  (`failed to stat '…'`); the test initialises a repository first.
  **Update 2026-10-05: fixed** by `GIT_CEILING_DIRECTORIES` for every sandboxed run that has the bundled
  Git ([git-runtime.md](git-runtime.md), "Repository discovery inside the sandbox"); native test
  `git_discovery_stops_at_the_grant` (red `git-https-native-vm1-red-1.log`, green
  `git-https-native-vm1-green-2.log`). The real-tools test now runs `ls-remote` without a repository.

### Declared listeners

A lease that declares listening ports is refused at once with "Listening ports for sandboxed
programs are not supported on Windows yet" (`connect_ports`, shown by the MCP client). Earlier
native runs showed the inbound loopback connection dropped at the accept layer by a filter
outside MiniClaw's lease even with `CheckNetIsolation LoopbackExempt -is`; the deep diagnosis is
deferred. The default-deny listen and accept filters are unchanged and still tested.
`declared_listener_is_refused_as_unsupported` passes on both guests.

### Residual risks and open items

- The root-key entry lives on a volatile key and is expected to vanish at every boot. The
  service adds it again at start (automatic start). Not verified across a real reboot: guests
  may not be restarted by agents.
- `netsh winsock reset`, a VPN or LSP installer, and Windows servicing were simulated by
  deleting an entry and by recreating keys in the synthetic tree, not run for real.
- x64 Windows hosts and other Windows builds are untested. The key list is this build's.
- A security product may object to access-list changes on system keys; the service then
  refuses networked leases.
- Node and emulated or ARM64 Python do not work in the sandbox (above). Bundled Git HTTPS works since
  lpac4 (2026-10-05).
- No product uninstaller calls the removal yet.
- A lease refusal while repair is impossible was tested through the manager and the client
  message parser, not through a running service with an unwritable real key.

### Sources

- [AppContainer and LPAC](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
- [DeriveCapabilitySidsFromName](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-derivecapabilitysidsfromname)
- [RegSetKeySecurity](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regsetkeysecurity) and [RegGetKeySecurity](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-reggetkeysecurity)
- [Order of ACEs in a DACL](https://learn.microsoft.com/en-us/windows/win32/secauthz/order-of-aces-in-a-dacl)
- [SECURITY_DESCRIPTOR_CONTROL](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-control)
- [ZwQueryKey, KeyNameInformation](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwquerykey)
- [WSCEnumProtocols32: native 64-bit and 32-bit catalogs](https://learn.microsoft.com/en-us/windows/win32/api/ws2spi/nf-ws2spi-wscenumprotocols32)
- [Service security and access rights: default `SERVICE_START`](https://learn.microsoft.com/en-us/windows/win32/services/service-security-and-access-rights)
- [StartService: start parameters reach ServiceMain](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-startservicew)
- [Loopback for server inbound connections, CheckNetIsolation](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/troubleshooting-uwp-firewall)
- [OpenSSL `rand_win.c`: CNG only with `_WIN32_WINNT >= 0x0600`](https://github.com/openssl/openssl/blob/openssl-3.6/providers/implementations/rands/seeding/rand_win.c)

## Sources

- [NtCreateFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile)
- [CreateFile sharing behavior](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
- [AppContainer and LPAC startup](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
- [SetSecurityInfo inheritance](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setsecurityinfo)
- [WFP filter arbitration](https://learn.microsoft.com/en-us/windows/win32/fwp/filter-arbitration)
- [Loopback exemption update contract](https://learn.microsoft.com/en-us/windows/win32/api/netfw/nf-netfw-networkisolationsetappcontainerconfig)

- [Inbound loopback configuration](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/troubleshooting-uwp-firewall)
- [Kernel object namespaces](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces)

- [Exact-object security update](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetsecurityobject)

- [Full file identity](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openfilebyid)
- [Windows recursive deletion](https://doc.rust-lang.org/std/fs/fn.remove_dir_all.html)

## Retained unknown identities

Native NTFS testing found that `OpenFileById` returns error 87 for a deleted ID,
including with the documented 64-bit NTFS descriptor. That error remains an
uncertain result; it is not treated as proof of deletion. Recovery still refuses
admission without proven job termination, and any reachable object's ACL or
namespace cleanup failure remains fatal.

After checking reachable recorded objects by full identity, bounded descendant
revocation, scratch cleanup and successful profile deletion, recovery can archive
unknown identities under the private `sandbox-journals/retired` directory. The
archive preserves the original entries and an explicit `UnknownIdentity` result,
plus the completed cleanup proof. It does not claim that every historical ACE was
removed. Certificates are flushed before an atomic no-replace rename, validated
on subsequent recovery, and reserve the old profile name against reuse. Historical
archives have a separate validation deadline and do not consume the active journal
count limit.

This has the same boundary as a trusted host process moving a newly created file
outside the approved roots: an unreachable object can retain an inert, unique run
SID ACE. The model process cannot move objects outside its grants, the old process
must be proven stopped, its profile is deleted, and future runs use different
identities. No host-wide permission, filesystem scan or volume access is added.

## Recovery and cleanup after independent review (2026-10-04)

This section covers revocation, journal parsing and job-termination proof. Grant-time
checks are unchanged except for hard links inside the granted folders (see "Service
attestation and hard links" below): a launch still refuses reparse points, more than
100000 objects, 64 levels or 120 seconds.

### Revocation no longer reuses grant-time guards

A run can leave things a launch would refuse. Native LPAC testing showed a sandboxed
process with write access creates hard links (`CreateHardLinkW` succeeded for an existing
and a new file); junction creation was refused inside LPAC with error 5, but a host tool
can create one in the same folder mid-run. Cleanup used to fail on either, and on trees
above the launch limits, and the retained record then blocked every later launch and
startup, including after a restart.

[Revocation](../../server/src/windows_sandbox_revoke.rs) now works only on handles:

- A recorded object is reopened by full file ID with `FILE_FLAG_OPEN_REPARSE_POINT`, so the
  handle names that object even if it has become a link. The run's entry is removed from
  the object itself. Link count is irrelevant because the security descriptor belongs to
  the file, not to a directory entry.
- A retained writable root lists its own entries through its handle and opens each child
  by name relative to that handle with `FILE_OPEN_REPARSE_POINT`. A reparse point has its
  own entry removed and is never listed or followed. Nothing is resolved by path.
- There is no object, depth or time cap. Each directory loses its inheritable entry before
  its children are listed, every step is idempotent, and an interrupted walk restarts from
  the root. If a listed name vanishes (possibly renamed) the walk repeats, up to three passes.
- Sharing and lock violations and delete-pending names are retried for about 0.6 seconds
  per object. An object that still cannot be opened is an error and the record is retained.
- An object without a DACL, or whose DACL has no entry for the run SID, is left unwritten.
- Scratch removal deletes a top-level junction or directory symlink as a link and never
  opens its target.

Residual limits: an object in a granted folder that the user cannot open for `WRITE_DAC`
(for example one created there by another account during the run) still retained the
record; since 2026-10-06 such an object is retired as `Unreadable` when MiniClaw never granted it
(see "Folders with items the user cannot manage"). A host program that moves a new object out of every granted root during the run
leaves an inert entry for the retired run SID, as before.

### Journal growth and limits

Children found during revocation are recorded once per identity, in batches of 512 with
one flush before that batch's ACL changes. Retried recoveries append nothing new. Writers
stop recording discovered children at 250000 lines or 64 MiB and keep revoking; the
retained root finds those children again. The reader accepts 400000 lines and 192 MiB, so
no record this code writes, including a terminal certificate that repeats every record,
can exceed what it can read back.

### Torn and empty records

`Journal::create` flushes the header before the profile, scratch directory or any ACL
change exists; `allow`/`flush` flush identities before their grants; `bind` flushes the
job name before the process resumes. A record is therefore complete before the step it
describes, and recovery reads accordingly:

- No complete line (empty file, or only an unterminated first line): the launch stopped
  before anything existed. The record is removed.
- An unterminated final line after complete lines: that step never ran. The tail is
  truncated under the record lock and recovery continues with the complete lines.
- A complete line that does not parse, a missing header before other lines, or anything
  after a terminal certificate: corruption. The record is retained and startup stops.
- A retired archive with an unterminated tail is treated as altered.

A new record that cannot be locked or have its header written is deleted, since nothing
was granted.

### Job termination without a restart

Sandbox jobs are named, created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and no breakaway.
Recovery accepts one of these proofs for a bound record, in this order:

1. `job_empty` record. When a job is observed with no active process (normal cleanup after
   `terminate_and_wait`, or recovery that reopened the job), `{"job_empty": "<job>"}` is
   flushed before revocation starts. A cleanup that then fails can be retried in the same
   boot. The value must equal the bound job.
2. The job can still be opened and verified: terminate, wait for zero active processes.
3. The job name is gone and the broker's authenticated boot epoch has changed.
4. The job name is gone in the same boot (`JobClosed`). Windows terminates every member when
   the last handle to a kill-on-close job closes, and the name disappears with that handle.
   [The liveness check](../../server/src/windows_sandbox_liveness.rs) then confirms the
   terminations finished: it lists every process, and requires that none still running has
   the run's AppContainer SID. A process it cannot inspect counts as unproven unless Windows
   reports it in another session or under another account. Unproven after 15 seconds is an
   error, and the record is retained with the existing restart message.

The fourth proof runs with the backend's own rights. It passed natively for the elevated
test owner. Under a fresh standard account on the second test guest the check found
processes in its session that it could not inspect, so it reported unproven and the record
was retained. The SYSTEM service now attests the same fact for any account (next section);
this account's own check remains the fallback when the service gives no attestation.

### Control flags

`NtSetSecurityObject` cleared `SE_DACL_AUTO_INHERITED` on objects that had it: a stored
descriptor never carries `SE_DACL_AUTO_INHERIT_REQ`, and in native testing the kernel kept
the inherited flag only when the request bit accompanied it. Microsoft's reference defines
both bits but does not state this rule; the evidence is the native red and green runs. `change_acl` now adds the request bit when
the object had the flag. The kernel does not propagate to children either way. A native
test covers a directory and a file through grant and revoke.

### Launch cost

Grants are queued and applied in batches of 512: one journal flush, then one acquisition
of the cross-process ACL lock, per batch. Cleanup holds the lock per batch as well and
skips the write when the DACL has no entry for the run SID. Cleanup always runs on the
cleanup worker, including for a sandbox that never bound a process.

Measured on the second ARM64 test guest (6 cores, debug build) with a 50051-object
writable folder, two rounds each: launch stamping went from 39.1-45.0 s to 15.9-17.0 s
and cleanup from 40.5-42.4 s to 2.6-3.1 s. Revoking 101101 objects created during a run
took 13.6 s; before, that cleanup failed. The Git runtime grant is not cached: every run
has its own SID, so a cache would need a standing grant to a shared identity.

## Service attestation and hard links (2026-10-04)

### Job-death attestation by the service

After `Hello`, a client may send one of two frames: a lease request, or

```json
{"attest":{"profile":"MiniClaw.Run.<32 hex>","job_name":"Global\\MiniClaw.Job.<32 hex>"},"proof":[...]}
```

The proof is HMAC-SHA256 with the installation key over
`"MiniClaw Windows job attestation v1\0" || challenge || 0 || json(attest)`. The domain
string differs from the lease and `Hello` domains, so no proof serves two purposes. The
[service](../../server/src/windows_network_attest.rs) answers `{"ok":true,"dead":<bool>}`
and closes. It answers `dead: true` only when all of this holds, in this order:

1. The pipe client's token user is the configured owner and is not an AppContainer
   (`peer`, unchanged), and the MAC verifies. Nothing is inspected before that.
2. Both names match the generated patterns exactly.
3. `OpenJobObjectW(job_name)` fails with `ERROR_FILE_NOT_FOUND`. Any other result,
   including access denied or a different object type under that name, is `false`.
4. With `SeDebugPrivilege` enabled, two listings of every process, 100 ms apart, each find
   no running process whose `TokenAppContainerSid` equals the SID derived from the profile
   name. A process ID that no longer exists (`ERROR_INVALID_PARAMETER`) or a signalled
   process handle counts as exited. A running process whose token cannot be read is `false`.
5. The job name is still absent afterwards.

[`attest_job_dead`](../../server/src/windows_network_broker_native.rs) authenticates the
service as for a lease, asks, and repeats every 250 ms for up to 15 seconds while the
answer is `false`, because the terminations Windows starts when the job closes take a
moment. Recovery calls it for proof 4 before its own process check. With an attestation the
record is recovered. Without one (service not configured, stopped, an older service that
drops the unknown frame, or `false` after 15 seconds) the account's own check runs as
before, so nothing that used to recover stops recovering. A live member defeats both.

Adversarial review of the request:

- **Oracle.** The answer is one bit and needs the installation key (a file only the owner
  can read) and the names. It says whether a job with that 128-bit random name exists (job
  names in `Global\` are visible to every local user anyway) and whether some process has
  the SID derived from that 128-bit random profile name. A caller who learned another
  account's profile name could learn whether that run is alive. Profile names are stored in
  the owning account's private journal and registry hive. Requiring that the profile belong
  to the caller was considered and not added: `CreateAppContainerProfile` documents no
  location the service could check without impersonating the user, the client connects at
  identification level, and recovery may ask again after the profile was already deleted.
  The residual one-bit disclosure is accepted.
- **Race between the name check and the listing.** A member alive at the first listing is
  in the snapshot and is seen running. A child it creates after the snapshot is in the same
  job (no breakaway) and carries the same AppContainer token, so it is either terminated
  with the job or seen by the second listing. The name is checked before and after. The
  listings are a confirmation that the documented kill-on-close termination finished, not
  the source of the guarantee.
- **PID reuse.** A running process keeps its ID, so reuse can only replace an exited
  process; whatever runs under the ID now is the process that gets inspected.
- **Suspended member.** Suspension does not signal the process handle and the token stays
  readable, so it counts as running.
- **Member without the AppContainer token.** Every sandboxed process is created with
  `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and `PROC_THREAD_ATTRIBUTE_JOB_LIST` in one
  `CreateProcessW` call, and children inherit the token. The lease path already refuses a
  job containing any process without the package SID (`validate_job_contents`).
- **Denial of service.** Only the owner can open the pipe. A request without a valid MAC
  costs no scan. Scans run one at a time on a blocking thread; the existing limit of 64
  connections applies. An owner process can delay its own leases, nothing more.
- **Unreadable processes.** On the second test guest SYSTEM could open and read the token
  of every process (145 of 145; only the idle pseudo-process, ID 0, fails, with error 87).
  Microsoft's `OpenProcess` page says the System and CSRSS processes cannot be opened by
  user-level code. If that happens on some build the service answers `false` and the
  account's own check decides, as before this change.

### Hard links at grant time

The guard exists because an ACL entry lands on the file, not on a name. A file in a granted
folder that has a second name elsewhere would let a sandboxed command change, through the
approved name, a file the user sees under a name they never approved: a package store
shared by other projects, or a file under a protected root. Reading is the same exposure
when the other name lies under a blocked or protected path.

[`guarded_linked_file`](../../server/src/platform_fs_windows.rs) now lists every name of a
file with more than one link (`GetVolumePathNameW`, `FindFirstFileNameW`,
`FindNextFileNameW`). The list must have exactly the link count read from the handle and
contain the pinned path. [`Sandbox::linked`](../../server/src/windows_sandbox_native.rs)
grants the file only when every name

- lies inside a root of this run that has at least the access being granted (a name in a
  read-only root refuses a write grant),
- lies under no blocked or protected path, and
- opens the same file identity as the pinned handle.

Roots are the run's own non-blocked grants, plus the read-only Git runtime and shell
program for a shell sandbox. Anything else is refused with `PermissionDenied`. The message
gives the reason and the approved path, never the other name. A volume that cannot list
names (`FindFirstFileNameW` fails) is refused. A single-file grant, a private file, and
every host-side file tool keep the strict one-link rule.

Time of check: a link added after the check by the sandboxed process itself needs write
access to the file and to the new name's folder, so it stays inside the writable roots; in
native testing an LPAC process could not link a file it may only read. A link added by an
ordinary host process is inside the already accepted trusted-host boundary: that process
could change the file directly. A link added between the count and the listing makes them
disagree and refuses the launch.

Microsoft's pages do not state that `FindFirstFileNameW` returns names relative to the
volume root. That is observed behaviour; the code does not depend on it beyond the checks
above, which fail closed if it differs.

### Standard-user teardown

`windows-ci-remove-profile.ps1` required the profile folder to be owned by the disposable
account. Windows creates profile folders owned by SYSTEM (seen for every profile on the
second guest, the signed-in owner's included; no Microsoft page states the owner), so the
check refused every profile. It now accepts SYSTEM, Administrators or the account as owner
and additionally requires an explicit Full control entry for the account and no entry for
any other user account. `DeleteProfileW` then failed with error 87 because PowerShell
passed the string-typed `$null` computer name as an empty string; Microsoft documents that
the parameter must be NULL, and the call now passes a null pointer. The account's private
`temp` and `data` folders admit only that account, so its own test script removes them
after the last test passes. The standard-user script takes the `windows_acl_inheritance`
test binary as a new mandatory `-AclTests` argument.

### Evidence (second ARM64 test guest, 2026-10-04)

Logs are in `work/windows-vm`. Each run printed the hash of every refreshed source and
`Compiling miniclaw-server`, and checked that the test binaries were written after the run
began.

- Red, elevated owner, same tests with the service answering no attestation and every
  multiply linked file refused as before: 11 groups passed, 4 failed
  (`live_member_blocks_absent_job_proof`, `internal_hard_links_are_granted`,
  `outside_hard_link_is_refused`, `protected_hard_link_is_refused`).
  `attest-red-vm2.log`. The standard-user red is the earlier
  `recovery-fixes-standard-vm2.log`: that account could not inspect processes in its session.
- Green, elevated owner, against a disposable service built from these sources: 15 of 15
  groups, `attest-green-vm2-final.log`. `large_tree_is_revoked`: 101101 objects revoked in
  13.1 s, `attest-large-vm2.log`.
- Standard user, whole sequence: 64 tests passed, none failed, exit 0, including
  `backend_exit_recovers_without_restart`, `live_member_blocks_absent_job_proof`, the three
  hard-link tests, service removal, profile removal and account removal.
  `attest-standard-vm2-3.log`. Two earlier runs stopped first at a test assumption that
  only an elevated account satisfies and then at `DeleteProfileW` (`-1.log`, `-2.log`).
- SYSTEM token readability probe: `attest-token-diag-vm2.log`.

Not run: `windows-native-acceptance.ps1` as a whole, and the GitHub workflow.

### Sources

- [CreateHardLinkW: the security descriptor belongs to the file](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createhardlinkw)
- [CreateJobObjectW: closing the last handle of a kill-on-close job](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-createjobobjectw)
- [SECURITY_DESCRIPTOR_CONTROL bits](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-control)
- [ProcessIdToSessionId access requirement](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-processidtosessionid)
- [OpenProcess: SeDebugPrivilege, process ID 0, System and CSRSS](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess)
- [LocalSystem account: SE_DEBUG_NAME is enabled](https://learn.microsoft.com/en-us/windows/win32/services/localsystem-account)
- [AdjustTokenPrivileges: ERROR_NOT_ALL_ASSIGNED on success](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-adjusttokenprivileges)
- [TOKEN_APPCONTAINER_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-token_appcontainer_information)
- [DeriveAppContainerSidFromAppContainerName](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-deriveappcontainersidfromappcontainername)
- [OpenJobObjectW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openjobobjectw)
- [UpdateProcThreadAttribute: job list and security capabilities](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
- [FindFirstFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfilenamew) and [FindNextFileNameW: ERROR_HANDLE_EOF](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findnextfilenamew)
- [GetVolumePathNameW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumepathnamew)
- [BY_HANDLE_FILE_INFORMATION: nNumberOfLinks](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information)

## Language runtimes in the sandbox (diagnostic, 2026-10-05)

**Decided and implemented later the same day**: see "Language runtimes in the sandbox: decision and
implementation" below (own Node from the official build with a seed provider, N1, N2, P1; N4 not used).

Diagnostic and design only. No production code changed, the availability gate is unchanged, and
every registry descriptor touched by the experiment was restored and verified. Everything below
was measured on VM2 (Windows 11 ARM64 26100.9457, UAC on, elevated test owner) in the real sandbox
token, with a disposable network service installed from the same build. Standard-user runs, VM1,
x64 hosts and other Windows builds were not measured.

### What fails, and why

| Runtime | Symptom in the sandbox | Cause (measured) |
| --- | --- | --- |
| Node 24.21 ARM64, no approved host | `WSAStartup: (10107)`, exit 0x80000003 | libuv starts Winsock in every process and treats failure as fatal. The token cannot read the Winsock catalog keys. |
| Node, any token without the two keys below | `Assertion failed: ncrypto::CSPRNG(nullptr, 0)`, exit 134. `node --version` is not affected, it returns before this check. | Node's bundled OpenSSL 3.5.8 seeds through `CryptAcquireContextW(PROV_RSA_FULL, CRYPT_VERIFYCONTEXT)`. `node.exe` imports it from advapi32 and does not import bcrypt at all. The call returns 0x80090017 because it opens `HKLM\SOFTWARE\Microsoft\Cryptography\Defaults\Provider Types\Type 001` and then `…\Defaults\Provider\Microsoft Strong Cryptographic Provider`, and both are denied. `BCryptGenRandom` and `RtlGenRandom` work in the same token; `rsaenh.dll` and `cryptsp.dll` load. |
| Node running a script file | `EPERM: operation not permitted, lstat 'C:\'`, exit 1 | Node resolves the real path of the main module and of every required module, and on Windows that starts with `lstat` of the drive root, which the sandbox cannot open. `node -p 1+1` is not affected. |
| Python 3.13.15 ARM64 | exit 0xC0150002, no output | `python313.dll` carries a side-by-side manifest (resource 2), as does `python.exe` (resource 1). It names `Microsoft.Windows.Common-Controls` 6.0 and the `longPathAware` setting. The loader asks csrss for an activation context when the library loads; csrss reads the registry while impersonating the sandbox token and is denied at `HKLM\…\CurrentVersion\SMI\WinSxS Settings` and `…\SideBySide\Winners`. The WinSxS folder and manifests are readable. `CreateActCtxW` from inside the token fails with 14001 for any manifest with a setting or a dependency, and succeeds for an empty one. Node's own manifest has neither, which is why Node gets past this point. |
| Python `socket`, `ssl` | `ImportError: WSAStartup failed: error code 10107` | Same Winsock catalog denial as Node. |
| Python `asyncio` | `OSError [WinError 10047]` or 10107 at import | `_overlapped` creates a socket when it is imported. Socket creation needs `internetClient`. |
| x64 Node and Python under emulation | exit 0xC0000017 | See "x64 programs" below. |

Python's own OpenSSL (3.0.21) is not affected by the random-source problem: `ssl.RAND_bytes`
works in every token in which `ssl` can be imported.

Two things that looked like remedies and are not:

- `lpacCryptoServices` changes nothing. `CryptAcquireContextW` fails the same way, and no key in
  `HKLM` (414975 visited), `HKCU` (7866) or `HKU\.DEFAULT`, and none of 46714 surveyed files and
  folders, names its SID.
- `lpacCom` and `lpacAppExperience` do not help Python. Python started under them only because an
  earlier run in the same Windows session had made csrss build and cache the activation context
  for that exact file. A fresh copy of the same files still failed.

### Options

"Controls" means the same negative controls as the Winsock work, run in the same token: `HKCU`
root, `Environment`, `Software` and five more `HKCU` keys, `HKU\.DEFAULT`, seventeen `HKLM` areas
including `SOFTWARE`, `SOFTWARE\Microsoft`, `SYSTEM`, `Services`, `SAM` and `SECURITY`, two
sentinels, root listing, three registry writes, the outside-grant file, protected state, eight
folder listings, and live TCP and UDP receivers on loopback, `127.0.0.2`, `::1` and the machine's
own address. No control was exposed and no receiver was reached in any run (0 of each in all
five logs).

| # | Remedy | Extra access | Measured result | Evidence | Who decides |
| --- | --- | --- | --- | --- | --- |
| N1 | Runs without approved hosts carry the installation's existing Winsock-read capability, without `internetClient` | Read of the 29 Winsock catalog keys the service already manages. No filter lease exists for such a run. | `WSAStartup` 0. Every socket fails with 10047 (IPv4, IPv6, stream, datagram, raw). Name resolution fails with 11003, also for `localhost`. Node and Python probes: every connect, datagram and lookup failed. `node --version` works; Python `socket` and `ssl` import. Controls pass. | `runtimes-vm2-discovery-2.log` (`offline-winsock`), `-final-1.log` (`both-offline`), `-python-1.log` (`py-primed-winsock-only`) | Lead. It ends "an offline run has no capability", which `offline_run_stays_closed` asserts. |
| N2 | `NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main` in the sandbox environment | None | Script files run. Without it, `EPERM lstat 'C:\'`. | `-narrow-1.log` (`node-crypto3` against `node-crypto3-preserve`) | Lead. It changes how Node resolves symlinked packages. |
| N3 | Bundle a Node whose OpenSSL uses `BCryptGenRandom` (built with `_WIN32_WINNT` 0x0600 or later), or a release containing nodejs/node#65796 | None | Not built. `BCryptGenRandom` works in every token measured. Does nothing for a Node the user installed. | `CRYPTO` lines in every log | Lead |
| N4 | Read entry for the installation's capability SID on the two Cryptography keys above (`KEY_READ`, not inherited). The `HKLM` root entry (`KEY_QUERY_VALUE`) is also needed and already exists for that SID. | The values of two keys that are the same on every Windows installation: the default provider's name, and its image path, type and signature. | Node runs, alone and with N1 and N2, offline and networked. Ablation: without the provider key 0x80090019; without the root 0x80090017; query-only on both keys 0x80090017; `SOFTWARE\Microsoft\Cryptography` and `SOFTWARE\Policies\Microsoft\Cryptography` are not needed. Controls pass, including `HKLM\SOFTWARE\Microsoft\Cryptography` itself. | `-narrow-1.log`, `-final-1.log` | User: a second set of entries on machine-wide keys. |
| P1 | Before starting Python, the unsandboxed launcher calls `CreateActCtxW` on `python.exe` (resource 1) and `python313.dll` (resource 2) | None | Python then starts in the unchanged token with no capability, from a path Windows had never seen (`primed-copy`) and from the installation with the standard library. The same copy without the call exits 0xC0150002. | `-narrow-1.log` (`py-fresh-none`), `-final-1.log` (`primed-production-offline`), `-python-1.log` | Lead. It relies on csrss caching activation contexts, which Microsoft does not document. |
| P2 | Bundle a Python whose two manifests have no dependency and no setting | None | Not built. An empty manifest and Node's manifest both work in the token. | `SXS` lines in every log | Lead |
| P3 | Read entries for the capability SID on 12 keys: the root, `SMI\WinSxS Settings` and the three keys of its `…settings-2016…` subtree, `SideBySide\Winners`, and the three keys each of its `…common-controls…_none_…` and `…controls.resources…_en-gb_…` subtrees | Listing of `SideBySide\Winners`, which shows the installed side-by-side components, and reading those subtrees | Fresh Python starts; all three manifest shapes load. Ablation: without the `SMI` subtree, without the resources subtree, or with `Winners` alone, it fails. Controls pass. | `-python-2.log`, `-python-1.log` | User. Key names contain the display language and a component hash, so the list differs per machine and changes with servicing. |
| A | `registryRead` | Everything in "Network capability decision" above. Its SID is on 397538 of 414975 `HKLM` keys and 4608 of 7866 `HKCU` keys, and on none of the surveyed files. | Node, Python and x64 Node all start. | `-discovery-2.log` (`offline-registryRead`), `-final-1.log` (survey) | User. Already declined once because of `HKCU\Environment`. |

N4 and P3 were also measured together in one token, offline and networked
(`both-offline`, `both-networked`): Node and Python run, the networked run reaches only its proxy,
and the controls pass.

### What still does not work, whatever is chosen

- **Python `asyncio` in a run without approved hosts.** It needs a socket, and only
  `internetClient` allows one. In the production networked token with P1, Python ran completely,
  `asyncio` included, and reached only the proxy. A Python MCP server built on `asyncio`
  therefore needs the networked token even with no approved host, with a lease that allows
  nothing. That is a design change and needs a decision.
- **Name resolution in the networked token.** `localhost` resolves; `example.org` fails with
  11001 in the child while it resolves outside the sandbox. Tools must give host names to the
  proxy. Whether the query leaves the machine before the answer is withheld was not captured.
- **Child processes of the runtime.** P1 was measured for a process the launcher starts. Python
  starting Python from inside the sandbox, and extension modules that carry their own manifest,
  were not measured.

### x64 programs on ARM64

Every x64 program exits with 0xC0000017 because the emulator (`xtajit64se.dll`, which does load)
opens `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0` and is denied; the lookups under
`HKLM\SOFTWARE\Microsoft\Wow64\amd64` find nothing on this machine and are not the problem. With
a read entry on that one key and the root, x64 Node ran as far as the Winsock failure, and x64
Python then stopped with 0xC0000022 at `…\CurrentVersion\SideBySide`. With `registryRead`, x64
Node starts. `%SystemRoot%\XtaCache` is closed to the sandbox (administrators and the cache
service only) and was not needed. Microsoft documents no capability for emulation, and Chromium
turns its network sandbox off when running emulated on ARM64. `HARDWARE` is rebuilt at every
boot, so an entry there would have to be reapplied like the root entry. Recommendation: do not
pursue this; bundle ARM64 Python for ARM64 hosts, since ARM64 Python works with P1.

### Recommendation

1. Fix what MiniClaw bundles rather than the machine: N3 for Node (the bundled Git has the same
   OpenSSL problem), and P2 or P1 for Python. None of these grants access.
2. Adopt N2 for sandboxed Node.
3. Adopt N1 if the lead accepts one read-only capability on offline runs. Without it no Node
   program starts offline and Python has no `socket` module. Its controls passed.
4. Ask the user about N4 only if a Node that MiniClaw does not bundle must work. It is two keys
   with fixed names and public contents. Do not ask for P3: its key list is machine-specific and
   P1 covers the same case without access.
5. Decide separately whether `asyncio` servers get the networked token with a lease that allows
   nothing.

### Method and limits

- Test: [`windows_runtime_diagnostic.rs`](../../server/tests/windows_runtime_diagnostic.rs),
  ignored tests `runtime_experiment`, `capability_acl_survey` and the recovery entry
  `runtime_acl_restore`. A plan file names, per phase, the capabilities, the read entries and
  the runs: a probe child of the test binary, or a real program. Entries are set with
  `RegSetKeySecurity` on the exact key after a durable before-record, and restored from the
  original descriptor bytes; `HKCU` is never written. A self-test on synthetic keys runs first.
- The entries were given to a separate diagnostic capability (`miniclawRuntimeRead.diagnostic-7c41a9e2`)
  held next to the installation's Winsock capability, so that run needed its own root entry.
- Each log names VM2, the hash of the test source, 281 to 283 sources verified against the host
  tree, `Compiling miniclaw-server`, and build times after the run began. The runner's own
  comparison of 93 to 106 keys before and after shows 0 differences in every run, and no key
  still named the diagnostic SID. The service's 29 Winsock entries were removed, 29 as recorded.
- Opens were listed with ETW (registry, file and process providers) for the child and for csrss.
- `runtimes-vm2-discovery-2.log` ends with one runner failure: the csrss trace query was
  malformed. The test itself passed, and the query was fixed for the later runs.
  `runtimes-vm2-discovery-1-snapshot-bound.log` is a runner failure before any test ran.
- Not measured: a real MCP server, npm, ES modules or a virtual environment under these options;
  how long csrss keeps a cached context and how many it keeps; a reboot; a standard user.

### Sources

- [AppContainer and LPAC: `registryRead`, `lpacCom`](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer). Microsoft publishes no meaning for the other `lpac*` names; the [capability reference](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations) does not list them.
- [Chromium sandbox design, LPAC](https://chromium.googlesource.com/chromium/src/+/main/docs/design/sandbox.md), [capabilities per process type](https://chromium.googlesource.com/chromium/src/+/main/sandbox/policy/win/sandbox_win.cc), [installer entries for its own capability SIDs](https://chromium.googlesource.com/chromium/src/+/main/chrome/installer/setup/configure_app_container_sandbox.cc), [network sandbox off when emulated on ARM64](https://chromium.googlesource.com/chromium/src/+/main/sandbox/policy/features.cc)
- [OpenSSL `rand_win.c`](https://github.com/openssl/openssl/blob/openssl-3.5/providers/implementations/rands/seeding/rand_win.c) and [`e_os.h` default `_WIN32_WINNT` 0x0501](https://github.com/openssl/openssl/blob/openssl-3.5/include/internal/e_os.h); [default raised on master](https://github.com/openssl/openssl/pull/31774)
- [Node `InitializeOncePerProcessInternal`](https://github.com/nodejs/node/blob/v24.x/src/node.cc), [`ncrypto::CSPRNG`](https://github.com/nodejs/node/blob/v24.x/deps/ncrypto/ncrypto.cc), [startup check removed on main](https://github.com/nodejs/node/pull/65796)
- [CryptAcquireContextW error codes](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-cryptacquirecontextw)
- [libuv `uv__winsock_init`](https://github.com/libuv/libuv/blob/v1.x/src/win/winsock.c)
- [CPython `python.manifest`](https://github.com/python/cpython/blob/3.13/PC/python.manifest)
- [Activation context creation through csrss](https://learn.microsoft.com/en-us/archive/blogs/junfeng/activation-context-creation-flow); [csrss impersonation and its context cache (Zero Day Initiative)](https://www.thezdi.com/blog/2023/1/23/activation-context-cache-poisoning-exploiting-csrss-for-privilege-escalation)
- [How emulation works on Arm](https://learn.microsoft.com/en-us/windows/arm/apps-on-arm-x86-emulation)

## Access-list writes found by the first runs through the product (2026-10-05)

The first end-to-end run with the gate open left an entry of a finished run on files inside the
working folder, with cleanup reporting success. Two defects in how a run's entry is written and
removed were behind it; both are fixed in
[`write_acl`](../../server/src/windows_sandbox_native.rs) and
[`acl`](../../server/src/windows_sandbox.rs).

**An inherited entry of a live run survived that run's cleanup.** While a terminal is open, the
Changes panel's Git (or any one-shot command) is a second run on the same folder. A file it
creates or replaces inherits both runs' entries. The second run's cleanup removed its own; the
terminal's cleanup later called `SetEntriesInAclW(REVOKE_ACCESS)` for the terminal's entry, got
the list back with the same number of entries and no error, wrote it, and reported success.
Reproduced through the API (discard a tracked file with the terminal open: `README.md` and
`.git\index` kept `S-1-15-2-…:(I)(M)` after the terminal closed and after a restart) and in
`file_made_by_another_run_is_revoked`, where every object the second run made, including a
folder with an inheritable entry, kept the first run's entry. The entry names a package SID
that no later run can receive (the profile is deleted and its name is never reused), so no run
gained access; the user's files were left with a dead entry and nothing said so.

**A grant and revoke turned an ordinary file's inherited entries into explicit ones on a
protected list.** `GetSecurityInfo` does not return what is stored for an object without the
auto-inherited marker, which is every file an ordinary program created: it infers inheritance
from the parent folder's list as it is at that moment. Once the folder carried the run's
inheritable entry, an existing file's entries no longer matched and were reported as explicit
entries on a protected list. The code wrote that view back with its own entry added, which made
it true: `D:(A;ID;FA;;;SY)…` became `D:P(A;;FA;;;SY)…` after one run. A later permission change
on the folder would not have reached such a file.

What changed:

- The list is read with `GetKernelObjectSecurity`, which returns the stored bytes and control
  bits without inference.
- The run's entry is added and removed in a byte-exact copy of that list
  (`acl::with_trustee`, `acl::without_trustee`, unit-tested on every host). Every other entry
  keeps its bytes, flags and place. Removal takes every allow or deny entry of the SID,
  inherited or not. A second grant of the same object with the same inheritance flags adds its
  rights to the existing entry, as before.
- After a removal the stored list is read again. If the entry is still there, it is removed
  with a write that does not carry the auto-inherit request (the object then loses its marker),
  and if it is still there after that the cleanup fails and keeps its record. A removal is
  never reported without having been seen.

Evidence (VM2, elevated owner, `work/windows-vm`): red `gate-focus-vm2-1.log` (entries remain,
cleanup `Ok`), `gate-focus-vm2-2.log` (with the check: `before=4 after=4` for exactly the four
objects, cleanup fails), `gate-focus-vm2-3.log` (entries gone; the untouched file still ends as
`D:P`), green `gate-focus-vm2-5.log`: the new test, the ten other `windows_acl_inheritance`
tests, `windows_sandbox::` (29), the gate tests, terminal, MCP, `windows_broker`, `windows_git`
and the five `windows_git_commands` tests. The API reproduction is `gate-diag-acl-vm2-2.log`.

Not covered: an object that already carries the auto-inherited marker and a stale inherited
entry at the same time (the fallback write) was not provoked natively; objects under
`%TEMP%` and a user's home folder were, a folder on another volume or a network share was not.

## A run ending while another starts: launch admission (2026-10-05)

Every launch first scans the sandbox records folder and refuses to start while a record of a run
that is not live is unresolved ("Windows sandbox recovery retained N unresolved records"). A
record held by a live run is skipped: its file is open without write sharing and locked. Natively
`default_mcp_workspaces_are_private_between_runs` failed once this way in the standard-user
sequence (`gate-acceptance-vm2-2.log`): the previous test's MCP server had just ended, its cleanup
finished its record while the next server's launch scanned the folder, and the scan counted the
finished record as unresolved. A user would see the same when a command ends at the moment the
terminal or Changes starts another run.

Three moments of a finishing record were misread:

- The cleanup closed the record and then deleted it by name. A scan could open the record in
  between, and the deletion then failed (the scan's handle has no delete sharing).
- A name listed by the scan and deleted before the scan opened it was counted as unresolved.
- A deleted name stays listed until its last handle closes, and opening it meanwhile fails with
  access denied (`STATUS_DELETE_PENDING`). The scan counted that as unresolved too. The standard
  library cannot tell the case apart by path metadata: it answers access denied from the folder
  listing, which still shows the name.

What changed ([`windows_sandbox_journal.rs`](../../server/src/windows_sandbox_journal.rs)): the
cleanup deletes the record's name while its handle still holds the record and its lock (the
record is created with delete sharing); the scan treats a name that is gone at the open as
finished; an open refused with access denied is tried again for up to half a second, and the
record is then finished (gone), an ordinary record (it opens) or still unresolved, as before. A
record the scan itself resolved but could not delete because another launch's scan opened it
meanwhile is left to that scan, as a record held at the open is. Nothing that was unresolved
before becomes resolved: only a name that disappears counts as finished.

Evidence (VM2, `work/windows-vm`): `records_completed_during_admission_are_not_unresolved` runs one
thread that creates and completes 600 records while the main thread scans. Red
`gate2-journal-race-vm2-red.log` (unresolved at scan 9); still red with only the first two changes
(`gate2-journal-race-vm2-green.log`, scan 39) and with a POSIX-semantics deletion
(`gate2-journal-race-vm2-green2.log`, scans 13, 63 and 9). Checking the name's metadata after an
access-denied open passed three runs (`gate2-journal-race-vm2-green3.log`) but failed once at scan
2,913 inside the full acceptance (`gate2-acceptance-vm2-1.log`). With the retried open: 30 of 30
runs, 91,445 scans beside 18,000 completed records (`gate2-journal-race-vm2-stress.log`), and the
full acceptance runs after it.

## Language runtimes in the sandbox: decision and implementation (2026-10-05)

Decision: **MiniClaw's own Node**, from the official Node.js build, without rebuilding Node and
without any registry change. The two Cryptography keys (option N4 above) are **not** granted.
Lead-approved N1, N2 and P1 are implemented. The user's request for a repair flow is implemented
for the Winsock entries (the only machine-wide entries MiniClaw now manages).

### Why an official Node can work

The official Node.js 24.21.0 build links OpenSSL 3.5.8 statically and compiles it without
`_WIN32_WINNT`, so OpenSSL's own seed source (`rand_win.c`) calls `CryptAcquireContextW`, which
reads `HKLM\SOFTWARE\Microsoft\Cryptography\Defaults` and is denied in the sandbox. OpenSSL 3 lets a
configuration file choose the seed source by name (`seed` in the `random` section,
`crypto/rand/rand_lib.c`, `rand_new_seed`) and load it from a provider module; Node's official
build has module loading (`DSO_WIN32`, no `OPENSSL_NO_DSO` in its `configuration.h`) and reads
`OPENSSL_CONF` before its startup check `CHECK(ncrypto::CSPRNG(nullptr, 0))` (`src/node.cc`,
`InitializeOncePerProcessInternal`). MiniClaw therefore ships:

- [`openssl-seed`](../../openssl-seed/src/lib.rs): a provider of about 200 lines of Rust with one
  algorithm, `MINICLAW-SYSTEM-SEED`, which answers OpenSSL's seed requests with
  `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)`, the call OpenSSL itself makes when built for
  Vista or later. It links no OpenSSL library (provider ABI only), refuses a parent, reports
  strength 256, and wipes and frees each seed buffer it hands out.
- [`openssl.cnf`](../../openssl-seed/openssl.cnf): only a `nodejs_conf` section, which loads the
  default provider and the seed provider and names the seed source. Other OpenSSL users in the
  sandbox (the bundled Git, Python) read `openssl_conf`, which is absent, so the file changes
  nothing for them. The module is named relatively and found through `OPENSSL_MODULES`: OpenSSL
  reads `\` in a value as an escape, and an absolute Windows path silently broke the load
  (measured, below).

Not used, and why: rebuilding Node (A2) was unnecessary; the official Windows build process needs
ClangCL and the ARM64 MSVC components ([BUILDING.md](https://github.com/nodejs/node/blob/v24.x/BUILDING.md)),
and VM1 has 10 GB free. `--openssl-shared-config` only changes the section name. The JITTER seed
source is compiled out (`OPENSSL_NO_JITTER` in Node's configuration). `lpacCryptoServices` was
already shown not to help.

### Design

- **Where.** `bin\node\` of the Windows bundle (`scripts/backend-package.mjs`): the official
  `node.exe` and `LICENSE` taken from the pinned Node.js 24.21.0 archive
  (`scripts/setup/windows-node-sandbox.mjs`), the seed library built by
  `scripts/package-backend.mjs`, and `openssl.cnf`. Node's licence file (MIT and its bundled
  dependencies) ships beside it.
- **Verification.** [`private_node`](../../server/src/windows_runtimes.rs) opens and pins the
  three files once per backend: `node.exe` must have the official SHA-256 of its architecture, the
  seed library the hash the backend was built with (`MINICLAW_OPENSSL_SEED_SHA256`, set by the
  packaging script after it builds the library), and `openssl.cnf` the exact bytes compiled into
  the backend. A build without the hash, or a changed file, has no private Node, and sandboxed
  Node then behaves as before (it cannot seed).
- **Which runs use it.** [`Sandbox::add_private_node`](../../server/src/windows_sandbox_native.rs)
  grants the folder read-only, puts it first on `PATH`, and sets `OPENSSL_CONF` and
  `OPENSSL_MODULES`. Every shell, terminal and product Git run gets it (`new_shell`). A confined
  stdio MCP server gets it, and a server whose command is any `node.exe` runs the private
  `node.exe` instead (`program_for`, called from `mcp_client::spawn`); the server's own
  environment values still win. The user's Node is never started inside the sandbox.
  Not covered: an absolute path to another `node.exe` typed in the shell; `bun`, `deno`.
- **`npx`/`npm`.** Implemented later the same day: see "npm and npx in the sandbox" below (MiniClaw
  ships npm from the same archive; `npx.cmd`/`npm.cmd` are still never started).
- **N1.** Every sandbox token without approved hosts holds the installation's Winsock-read
  capability ([`token_capabilities`](../../server/src/windows_runtimes.rs)); with no configured
  service, or a name that is not an installation's capability, it holds none and the launch goes
  on. The gate is unchanged: it already requires a ready service, and a run with a broken
  Winsock entry fails closed (Winsock does not start; measured below).
- **N2.** Every sandboxed process gets `--preserve-symlinks --preserve-symlinks-main` appended
  to `NODE_OPTIONS` ([`windows_process::environment`](../../server/src/windows_process.rs)). The
  allowlist lets no inherited `NODE_OPTIONS` through, so only a value the caller deliberately
  passed (an MCP server's saved variable) is kept; a flag already present, or its `--no-` form,
  is left as written. Downside: packages reached through symbolic links (pnpm's store,
  `npm link`, some workspaces) resolve by their link path, so a package can load twice and
  pnpm's peer layout may not resolve. Ordinary `npm ci` trees, ES modules and `exports` maps work
  (measured).
- **Proxy.** `NODE_USE_ENV_PROXY=1` is set wherever the proxy variables are (shell and terminal;
  the MCP client already did), so Node's `fetch` and `https` use the approved-host proxy.
- **P1.** Before a sandboxed launch whose image is `python*.exe`/`pythonw*.exe`,
  [`prime`](../../server/src/windows_runtimes.rs) calls `CreateActCtxW` on the image (resource 1)
  and on each `python3*.dll` beside it (resource 2), and does the same for the base interpreter
  when `..\pyvenv.cfg` names a `home` (a virtual environment's `Scripts\python.exe` is a launcher
  that starts the base `python.exe`). Best effort: errors are logged at debug level and never fail
  the launch. Fragile: it relies on csrss keeping the activation context it built outside the
  sandbox, which Microsoft does not document. The native test below fails with that explanation
  if a Windows update stops it.
- **Repair flow.** The service keeps verifying and repairing its Winsock entries at start and
  before each lease. New: a signed `Repair` request on the service pipe (its own MAC domain,
  `windows-access-v1`, owner-only like every connection) runs the same check at once and answers
  with the reason when it fails. The backend reads the published status into
  `GET /api/sandbox` (`access: healthy | repaired | failed`, time, reason) and offers
  `POST /api/sandbox/repair`. The page (Windows only, in permission settings) says "Network access
  for sandboxed tools was turned off because Windows changed system settings." with **Set up
  again**; a repaired state is reported quietly. When the running service cannot repair (missing,
  stopped, or refused), the answer names `miniclaw repair`, which restarts the service from the
  installation's reviewed backend with one administrator approval (the existing update script)
  and reports the published status. The elevated payload is unchanged.

### Security review

- **What the seed provider changes.** Only where Node's OpenSSL takes seed material: from the
  operating system's preferred generator instead of the legacy CryptoAPI provider, which itself
  ends in the same system generator. No access is added to the token, no registry key or file
  outside `bin\node` is read for it, and nothing is changed on the machine.
- **Who can replace it.** `bin\node` is in the user's installation, like the shell; a program
  running as the user could change it, and the backend then refuses it (hash and byte checks)
  until `miniclaw update`. A sandboxed program can read but not write it. It could point
  `OPENSSL_CONF` at its own file for its own child; that loads code into its own sandboxed
  process only.
- **Failure mode.** A provider that does not load leaves OpenSSL without a seed source, and Node
  aborts at its startup check (measured with the backslash configuration): it never runs with a
  weak seed.
- **N1.** The offline token gains read access to the 29 Winsock catalog keys the service already
  manages, nothing else; the networked review above applies. Every socket creation fails with
  10047, every name lookup with 11003, and live receivers saw nothing (below).
- **P1.** The launcher parses the manifest of a file the user configured to run anyway, as Windows
  does at every process start. A forged `pyvenv.cfg` can only make the launcher prime another
  Python's manifests.
- **Repair request.** It changes only this installation's own entries, through the same manager
  and lock as a lease; only the owner's authenticated backend can send it.

### Evidence (VM1, Windows 11 ARM64 26100.9457, UAC off, elevated owner; `work/windows-vm`)

- **Feasibility, official Node in the production tokens** (`rtfix-vm1-a1-seed-2.log`,
  `rtfix-vm1-a1b-trace-1.log`, diagnostic harness): without a configuration, and with the
  absolute-path configuration, Node aborts with `Assertion failed: ncrypto::CSPRNG(nullptr, 0)`
  in the N1 token and in the networked token; with the forward-slash or relative-module
  configuration, `require('crypto').randomBytes(4)` works in the N1 token (exit 0). Zero
  capabilities: `WSAStartup (10107)`. The N1 token's child controls passed (no `EXPOSED`).
  macOS check of the same provider: official Node 24.21.0 calls its seed functions
  (`SEEDTRACE`), and a missing module makes Node abort.
- **Product path, run 2** (`rtfix-vm1-product-2.log`: 292 sources hash-verified against the host
  tree, `Compiling miniclaw-server`, artifacts newer than the run start; seed library
  `c5d89e1e…`, backend built with that hash; shell, ConPTY and private Node staged by the production
  packaging code from the pinned archive): 24 of 25 groups passed. Units: `windows_runtimes` (9),
  `windows_winsock_acl` (18), `windows_network_broker` (20), the two `windows_process` environment
  tests, `windows_gate` (13). Native: the private Node is verified and a changed `openssl.cnf`, a
  changed seed library and a build without the hash are refused; `offline_run_stays_closed` (token
  holds exactly the Winsock SID, `WSAStartup` 0, TCP/UDP/raw sockets over IPv4 and IPv6 all 10047,
  `localhost`, `example.com` and an `.invalid` name 11003, no TCP or datagram reached the loopback,
  IPv6 or LAN receivers, every registry, file and protected-state control denied);
  `exact_proxy_only` (before and after a service restart); `networked_run_stays_closed_elsewhere`;
  `shell_node_runs_offline_with_the_private_node` (`node check.mjs` in the shell: the private
  `node.exe`, OpenSSL 3.5.8 randomness, `randomUUID`, an Ed25519 key, file write and read, an ES
  module, a CommonJS package, `NODE_OPTIONS` as appended; connect `EAFNOSUPPORT`, lookup
  `EAI_FAIL`; the live listener saw nothing); `shell_node_https_through_the_proxy` (`fetch`:
  example.com 200, www.iana.org refused and listed in `refusedHosts`);
  `sdk_server_runs_confined_with_the_private_node` (the official `@modelcontextprotocol/sdk` 1.32.0,
  installed with `npm ci` from its lockfile, 3,649 files; configured with the user's `node.exe`, it
  ran with the private one, listed its tools and answered; offline `fetch` failed with `EAI_FAIL`);
  `sdk_server_fetches_only_its_approved_host` (200 for the approved host, refused otherwise);
  `python_starts_after_activation_priming` (a fresh copy under a name the launcher does not prime
  exits 0xC0150002; the same bytes as `python.exe` print `Python 3.13.15`);
  `python_venv_runs_offline_without_asyncio` (a virtual environment made outside the sandbox: its
  launcher and the base interpreter start, `os.urandom` and `ssl.RAND_bytes` work, `asyncio` fails
  with 10047); `access_is_set_up_again_by_the_running_service` (an entry removed from a real
  catalog key; `repair_access` put it back without elevation; `access` became `repaired` with its
  time, network available); `access_that_cannot_be_repaired_fails_closed` (the key locked against
  SYSTEM: the repair is refused with the key named, `access` is `failed`, the page's network part
  is unavailable with the reason, a networked command is refused before it starts and its marker
  file is absent, an offline command runs with no network and Winsock refusing to start; the key's
  descriptor was then restored byte for byte and the next repair reported healthy);
  `dropped_catalog_entry_is_repaired_before_the_lease`; `declared_listener_is_refused_as_unsupported`;
  `mcp_client::windows_tests` (4) and `terminal::windows_tests` with the new environment;
  `sandbox_runtime_report` (the user's `node.exe --version` and `python.exe --version` start offline).
  The one failure was the first Python MCP test, rewritten below.
- **Python MCP servers** (`rtfix-vm1-product-3.log`, `-product-4.log`): `mcp` 1.30.0 in a venv
  (installed without `cryptography`, which has no Windows ARM64 wheel for Python 3.13). Offline,
  FastMCP fails at `import _overlapped` (10047). With an approved host it imports, but asyncio's
  proactor loop builds a socket pair through a loopback listener (`socket._fallback_socketpair`,
  `lsock.listen()`), which the sandbox's default-deny listen filter refuses (10013). The test
  `python_mcp_server_cannot_start_in_the_sandbox` now asserts both reasons, so a change shows.
  This corrects the diagnostic above, which saw `asyncio` import in the networked token but did
  not run an event loop.
- **Preserved state**: runs 1 to 3 each ended with a failure and preserved their stopped
  disposable service; the next run removed it with `windows-ci-stop-network.ps1` (29 Winsock
  entries removed, 29 as recorded, each time). Run 4 removed its own. Nothing of this agent's is
  left on VM1.

### Residual risks and open items

- P1 depends on undocumented csrss caching; it is not known how long the cache lasts, whether it
  survives a reboot or a session change, or how many contexts it keeps. Python extension modules
  with their own manifest and Python started by Python inside the sandbox were not measured.
- **Resolved below** ("asyncio in the sandbox"): option (3), with named pipes instead of `AF_UNIX`.
- Python MCP servers built on `asyncio` do not start in either token (above). Options for the
  lead, none implemented: (1) a lease that lets a process listen on and connect to its own
  ephemeral loopback port, scoped to its package SID (needs WFP work, and Windows' own loopback
  isolation dropped inbound loopback to a sandboxed listener before, see "Declared listeners");
  (2) run Python MCP servers unrestricted with the user's consent, as today; (3) a patched
  `socketpair` (for example `AF_UNIX`, which Windows supports) injected through `sitecustomize`,
  which changes the user's interpreter behaviour and needs its own review.
- The private Node is Node 24.21.0 only. Projects pinned to another major version, native addons
  built for another ABI, and global packages of the user's Node do not work without changes.
- A broken Winsock entry now also stops Winsock in offline runs (Node aborts offline, BusyBox `nc`
  reports `WSAStartup failed`); it fails closed, and the page offers the repair.
- x64 hosts, a reboot, VM2 with UAC on and a standard user were not run for this change (below).

## asyncio in the sandbox (2026-10-05)

User decision: option (3) of the list above ("option 2" in the user's numbering): sandboxed Python
builds its internal wake-up channel without the network, injected by MiniClaw at launch, with no
change to filters, capabilities or access lists. **Implemented with named pipes, not `AF_UNIX`.**
`AF_UNIX` cannot be created in the sandbox without a new read entry on a machine-wide key, which
was not authorised. Python MCP servers run in the networked token under a lease whose proxy allows
their approved hosts, possibly none, because `asyncio` cannot even be imported without a socket.

### What fails, and why (measured)

All on VM1 (Windows 11 ARM64 26100.9457, UAC off, elevated owner), official ARM64 CPython 3.13.15,
in the production tokens built by the diagnostic harness: offline (Winsock read only), networked
(`internetClient` + Winsock read, with the service's lease for the proxy port), and networked
without a lease (diagnostic only). No other CPython was installed on VM1 or VM2, and none was
downloaded; 3.11, 3.12 and 3.14 were compared from source only (below).

| Step | Offline | Networked + lease | Networked, no lease | Unsandboxed |
| --- | --- | --- | --- | --- |
| `socket(AF_INET)` / `AF_INET6` | 10047 | ok | ok | ok |
| `socket(AF_UNIX, SOCK_STREAM)` (family 1) | 10047 | 10047 | 10047 | ok |
| `import _overlapped` (so `import asyncio`) | 10047 | ok | ok | ok |
| Proactor loop, unpatched | - | 10013 at `lsock.listen()` | runs, and **reached 1.1.1.1:443** | runs |
| Named pipe `\\.\pipe\python-pipe-…` (asyncio's subprocess pipes) | 5 | 5 | - | ok |
| Named pipe `\\.\pipe\LOCAL\…`, IOCP `ReadFile` on it | ok | ok | - | ok |
| `signal.set_wakeup_fd(<C descriptor>)` | - | process stopped, 0xC0000008 | - | ok |
| Hook below: loop, threads, executor, subprocess | `asyncio` still 10047 | all work; only the proxy reachable | - | all work |

- **Why `AF_UNIX` fails.** An ETW registry trace of the child (`asyncio-vm1-a2-trace.log`) shows
  `mswsock` opening each transport's helper key `HKLM\SYSTEM\CurrentControlSet\Services\<transport>\Parameters\Winsock`
  for the transports in `WinSock\Parameters\Transports` (`Tcpip6, Tcpip, vmbus, Psched, afunix, RFCOMM`).
  `afunix\Parameters\Winsock` is denied (0xC0000022) in both tokens. Its list grants read to
  Users, ALL APPLICATION PACKAGES (not an LPAC) and `registryRead`
  (`S-1-15-3-1024-1065365936-…`, derived and compared), nothing else (`asyncio-vm1-a3-pipes.log`).
  Without the helper Winsock has no provider for the family: `WSAEAFNOSUPPORT` (10047), before
  any check by `afunix.sys` or WFP. Making it work needs a read entry for the installation's
  Winsock capability on that one key (or `registryRead`, declined earlier). Not done; a decision
  for the user.
- **Why `asyncio` fails offline.** The same mechanism: `Tcpip\Parameters\Winsock` and
  `Tcpip6\…` grant read to `internetClient` (`S-1-15-3-1`), `internetClientServer`,
  `privateNetworkClientServer` and one more fixed capability, not to the offline token. CPython's
  `_overlapped` creates `socket(AF_INET, SOCK_STREAM, IPPROTO_TCP)` when it is imported, to fetch
  `AcceptEx`, `ConnectEx`, `DisconnectEx` and `TransmitFile` through `WSAIoctl`
  ([`overlapped.c`, `initialize_function_pointers`](https://github.com/python/cpython/blob/v3.13.15/Modules/overlapped.c#L174-L211),
  called from `overlapped_exec` after importing `_socket` for `WSAStartup`), and
  `asyncio/__init__.py` imports `windows_events`, which imports `_overlapped`, on Windows. This is C
  code at import: nothing in Python can redirect it. A networked token is required.
- **Why the networked loop fails.** CPython for Windows is built without `AF_UNIX`
  (`PC/pyconfig.h`: `HAVE_SYS_UN_H` is not defined, so `Modules/socketmodule.h` undefines
  `AF_UNIX`; unchanged on `main`), so `socket.socketpair` is `_fallback_socketpair`
  ([`Lib/socket.py`](https://github.com/python/cpython/blob/v3.13.15/Lib/socket.py#L598-L669)):
  a listener on 127.0.0.1, a connect and an accept. Every event loop calls it once
  (`BaseProactorEventLoop._make_self_pipe`, `proactor_events.py` line 779; the selector loop
  likewise). The sandbox's default-deny listen filter refuses `listen` (10013).
- **Internet without a lease.** With `internetClient` and no lease, the probe connected to
  1.1.1.1:443; loopback stayed closed (Windows' loopback isolation). The lease is what confines a
  networked run. The launcher already guarantees it: `Sandbox::enable_network` is called before the
  process is created and `windows_network_broker::connect_ports` (the lease) before
  `pending.resume()` in `mcp_client::spawn`; a failure returns before the suspended process is
  resumed.
- **Strict handle checks.** A sandboxed process has `ProcessStrictHandleCheckPolicy` flags 0x3
  (measured with `GetProcessMitigationPolicy`). `signal.set_wakeup_fd` probes a C descriptor with
  `getsockopt` as if it were a socket (`Modules/signalmodule.c`), which raises
  STATUS_INVALID_HANDLE there and stops the process (`asyncio-vm1-a5-hook.log`, `CAUSE`).
- **Extension modules.** Every standard-library `.pyd` carries its own side-by-side manifest
  (`pyd-manifests-vm1.txt`). On a fresh boot (VM2, `gate2-acceptance-vm2-5.log`) `_overlapped`
  failed to load with a side-by-side error, because no process had loaded it since boot: P1 primed
  only `python.exe` and `python3*.dll`.
- WFP has nothing to classify for the chosen channel: named pipes are not network traffic. Net
  events were captured for every diagnostic run; none named Python.

The asyncio code involved is the same in 3.10, 3.11, 3.12, 3.13.15 and 3.14 (compared from
source): `_make_self_pipe`, `_close_self_pipe`, `_write_to_self`, `set_wakeup_fd(self._csock.fileno())`
in `__init__`, and `windows_utils.pipe(*, duplex, overlapped, bufsize)` called by
`windows_utils.Popen`. The `mcp` package's stdio server reads and writes through
`anyio.wrap_file` worker threads, which wake the loop with `call_soon_threadsafe`, i.e. through
the self-pipe; it does not use proactor pipes for stdin/stdout.

### Design

- **Hook.** [`scripts/setup/python-sandbox/sitecustomize.py`](../../scripts/setup/python-sandbox/sitecustomize.py),
  shipped as `bin\python\sitecustomize.py`, compiled into the backend
  (`windows_runtimes::PYTHON_HOOK`), and checked byte for byte when the backend first loads it
  (`load_python_hook`, pinned like the private Node's files). It does nothing unless
  `MINICLAW_PYTHON_SANDBOX=1` and the platform is Windows. Then it adds an import hook that, when
  `asyncio` is imported (never earlier; `socket` is not imported by the hook), replaces two
  functions:
  - `BaseProactorEventLoop._make_self_pipe`: the wake-up channel is a byte-mode named pipe in the
    run's own namespace (`\\.\pipe\LOCAL\python-pipe-<pid>-<128-bit random>`, one instance,
    `FILE_FLAG_FIRST_PIPE_INSTANCE`). The loop reads it through its completion port (`ReadFile`
    for a handle that is not a socket); writers use `WriteFile` on a `PIPE_NOWAIT` end, so a full
    pipe never blocks a thread. Its `fileno()` is -1, so `set_wakeup_fd` is turned off (Ctrl+C
    does not wake an idle loop; MiniClaw's runs do not use it).
  - `asyncio.windows_utils.pipe`: the same, with the same signature, so `create_subprocess_exec`
    works.
  Then it runs the `sitecustomize` it hides (the next one on `sys.path`), and stays reachable as
  `miniclaw_sitecustomize`. Errors in the hook never stop the interpreter.
- **Launch.** `Sandbox::add_python_hook` grants `bin\python` and the file read-only for every
  shell, terminal and MCP launch (as the private Node is). `Sandbox::environment` names the folder
  in `MINICLAW_PYTHON_HOOK`; the process launcher (`windows_process::environment`, sandboxed
  launches only) puts it first on `PYTHONPATH`, keeps the caller's own entries after it, removes
  the helper variable and sets `MINICLAW_PYTHON_SANDBOX=1` unless the caller set the marker.
- **Token.** An MCP server whose command is a Python image (`python*.exe`, `pythonw*.exe`, a
  virtual environment's launcher) always gets the networked token, a proxy with its approved hosts
  (possibly none, which refuses every `CONNECT`) and the lease for the proxy port
  (`windows_runtimes::python_needs_lease`, `mcp_client::spawn`). Compared with the offline token,
  such a run holds `internetClient`, can create IPv4/IPv6 sockets, resolves `localhost`, and can
  open TCP connections to its proxy port only; the proxy answers 403. Shell and terminal runs
  keep their tokens: without approved hosts `asyncio` still cannot be imported there.
- **P1 extended.** The launcher now also primes the extension modules of the interpreter's
  `DLLs` folder and of `Lib\site-packages` (of the virtual environment and of its base
  installation), at most 512 modules and 20,000 folder entries per launch, links not followed.

### Security review

- **What the hook can do.** It runs inside the sandbox with the program's own rights. A pipe in
  `\\.\pipe\LOCAL\` of a sandboxed process lives under its package
  (`\Sessions\<n>\AppContainerNamedObjects\<package SID>\`): another run cannot open it, by its
  short name or by the full path (measured below). A program that removes or replaces the hook,
  or sets the marker to 0, changes only its own process.
- **Names.** 128 random bits per pipe and one instance; the reading end is created first, so a
  squatter would make creation fail, not intercept the channel.
- **Interpreter behaviour.** Inside the sandbox, sandboxed Python's `asyncio` uses pipes for its
  wake-up channel and subprocess pipes; outside it, nothing changes (the marker is never set for
  an unrestricted launch, and the variable naming the hook never reaches one).
- **Lease.** A Python server now holds `internetClient` without approved hosts. The lease is the
  only thing between it and the network (see "Internet without a lease"); the order of the calls
  above guarantees it before the process runs. DNS: a unique name looked up in such a run failed
  with 11001 and did not appear in the DNS client cache afterwards (0 entries), so no positive or
  negative answer was obtained for it; whether a query packet left the machine was not captured.

### Coverage gaps

- Not covered: Python in isolated or `-E` mode (ignores `PYTHONPATH`; tested on the Mac),
  embedded distributions with a `._pth` file (ignore `PYTHONPATH`), `uv run`/`uvx` (`uv.exe` is not
  a Python image), the selector event loop (`WindowsSelectorEventLoopPolicy`: `select` needs
  sockets; still fails), trio, `multiprocessing` (pipes named `\\.\pipe\pyc-…`), Python started
  in a shell or terminal without approved hosts (offline token, `asyncio` not importable), x64
  Python on ARM64 (does not start at all), third-party extension modules beyond the priming bound,
  and Ctrl+C waking an idle loop.
- A child Python started inside the sandbox prints `Failed to find real location of …python.exe`
  (it cannot resolve the real path of its own image); it runs.

### Evidence (`work/windows-vm`)

- Diagnostics, VM1: `asyncio-vm1-a1.log` (all four tokens, the table above),
  `asyncio-vm1-a2-trace.log` (registry trace, helper keys, unsandboxed control),
  `asyncio-vm1-a3-pipes.log` (pipes, key descriptors, controls), `asyncio-vm1-a4-hook.log` (first
  hook: 0xC0000008), `asyncio-vm1-a5-hook.log` (cause, final hook: loop, 3 thread wake-ups,
  executor, 20,000 cross-thread wake-ups with no writer blocked, subprocess, proxy only, a hidden
  `sitecustomize` chained, offline still 10047). Every diagnostic run removed its disposable
  service (29 Winsock entries removed as recorded) and left 0 differing registry descriptors.
- Product, VM1 (`asyncio-vm1-product-1.log`: 301 sources hash-verified, `Compiling miniclaw-server`,
  artifacts newer than the run): 11 of 11 passed. JS hook tests natively (5, none skipped);
  `windows_runtimes::` (13) and `windows_process::` (15) units; `python_hook_is_verified_and_a_changed_one_is_refused`;
  `python_mcp_server_runs_without_approved_hosts` (FastMCP from `mcp` 1.30.0 starts, lists 6 tools
  and answers; proactor loop on `PipeHandle`; no listening TCP port for its process; second
  loopback port, 127.0.0.2 and `::1` refused with 5 and their receivers saw nothing; `listen`
  10013; unique name 11001 and not cached; `https://example.com/` through the proxy 403; outside
  file 13, `HKCU\Environment` and `HKCU\Software` 5; another run: its `LOCAL` name 2, the first
  run's namespace path 5, while the unsandboxed owner opens that path and the run itself its name);
  `python_mcp_server_fetches_only_its_approved_host` (httpx over asyncio: 200, other host 403);
  `python_asyncio_script_uses_subprocess_and_the_proxy` (shell, one approved host: subprocess
  `HELLO`, `CONNECT example.com:443` 200, `www.iana.org` 403); regressions
  `python_starts_after_activation_priming`, `python_venv_runs_offline_without_asyncio`,
  `private_node_is_verified_and_a_changed_one_is_refused`, `sdk_server_runs_confined_with_the_private_node`.
- Product, VM1, after extending P1 to extension modules (`asyncio-vm1-product-2.log`: 303 sources
  hash-verified, guest-only sources removed (none found), `Compiling miniclaw-server`): 15 of 15
  passed, the above plus `python_extension_modules_load_after_priming` (a full copy of the
  installation at a new path, its interpreter and `python3*.dll` primed only: 18 extension modules
  fail with the side-by-side error, as `_overlapped` did on VM2; the same bytes as `python.exe`,
  primed by the launcher: every module loads, `_overlapped` and `_asyncio` then stop at their
  socket with 10047, offline), `mcp_client::windows_tests` (4), `offline_run_stays_closed`,
  `exact_proxy_only` and `networked_run_stays_closed_elsewhere`.
- **Not run:** VM2 (UAC on) and the standard-user sequence. Both VM2 attempts stopped before any
  test (`asyncio-vm2-product-3.log`: runtime archives missing for staging; `-product-4.log`: the
  runner's staging script did not create npm's folders); the full acceptance was not run. The new
  tests are in `scripts/windows-native-tests.psd1` (`Runtimes`, and the hook check in
  `StandardExact`) for the acceptance runners.

### Sources

- CPython [`Modules/overlapped.c`](https://github.com/python/cpython/blob/v3.13.15/Modules/overlapped.c),
  [`Lib/socket.py`](https://github.com/python/cpython/blob/v3.13.15/Lib/socket.py),
  [`Lib/asyncio/proactor_events.py`](https://github.com/python/cpython/blob/v3.13.15/Lib/asyncio/proactor_events.py),
  [`Lib/asyncio/windows_utils.py`](https://github.com/python/cpython/blob/v3.13.15/Lib/asyncio/windows_utils.py),
  [`Modules/signalmodule.c`](https://github.com/python/cpython/blob/v3.13.15/Modules/signalmodule.c),
  [`PC/pyconfig.h.in`](https://github.com/python/cpython/blob/v3.13.15/PC/pyconfig.h.in),
  [`Lib/site.py`](https://github.com/python/cpython/blob/v3.13.15/Lib/site.py) (`sitecustomize`)
- [AF_UNIX comes to Windows](https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/)
  (pathname and abstract sockets, `SOCK_STREAM` only, no `socketpair`, write access to the socket
  file needed to connect)
- [Named pipe names and `LOCAL\` in an AppContainer](https://learn.microsoft.com/en-us/windows/win32/ipc/pipe-names),
  [`CreateNamedPipeW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipew),
  [`SetNamedPipeHandleState` (`PIPE_NOWAIT`)](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-setnamedpipehandlestate)
- [`PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_strict_handle_check_policy)
- [`DeriveCapabilitySidsFromName`](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-derivecapabilitysidsfromname)

## npm and npx in the sandbox (2026-10-05)

User decision of 2026-10-05: "npx support yes". Many MCP servers are documented as
`npx -y <package>`. On Windows `npx` and `npm` are `npx.cmd` and `npm.cmd`, batch files, which
MiniClaw never starts: `CreateProcess` runs a batch file through `cmd.exe`, which parses the
command line again ([CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw),
[Rust CVE-2024-24576](https://blog.rust-lang.org/2024/04/09/cve-2024-24576/); "Batch files" in
[review-processes.md](review-processes.md)). That rule is unchanged. What changed: MiniClaw ships
npm with its private Node, and runs npm's JavaScript entry points and the package's command script
with that Node, with the arguments as an argument list.

### What ships

`bin\node` of the Windows bundle (`sandboxNodeFiles` in
[`windows-node-sandbox.mjs`](../../scripts/setup/windows-node-sandbox.mjs)) now also holds:

- `node_modules\npm\…`: npm 11.19.0 exactly as in the pinned Node.js 24.21.0 archive, 1,926 files,
  12.5 MB unpacked, taken only from the archive whose SHA-256 is pinned. The tree is the same in
  the ARM64 and x64 archives (computed from both). Its listing digest (SHA-256 over sorted
  `<sha256>  <path>` lines) is pinned twice, in the packager and in
  [`windows_runtimes`](../../server/src/windows_runtimes.rs) (`NPM_TREE_DIGEST`, `NPM_FILES`); the
  packager refuses another tree.
- `npm` and `npx`: two `sh` scripts ([`npm-launchers`](../../scripts/setup/npm-launchers)) that
  run `node.exe node_modules/npm/bin/npm-cli.js "$@"` (and `npx-cli.js`). The backend holds their
  exact bytes. They are not npm's own `npm`/`npx` bash scripts, which use bash syntax and look for
  a globally installed npm first.
- `npm-sandbox.cjs`: a 40-line preload the launchers give npm (`node --require`), explained under
  "What had to change for npm" below.
- `sh.exe`: the pinned BusyBox (the same bytes as `bin\shell\busybox.exe`, checked against the same
  pin). npm starts its script shell as `<shell> -c <script>`
  ([`@npmcli/promise-spawn`](https://github.com/npm/promise-spawn), `spawnWithShell`), and
  BusyBox chooses its applet from the program name, so `busybox.exe -c` would not work.

Package size: +13.2 MB unpacked per Windows bundle (+1,930 files: npm's 1,926, the two launchers,
the preload and `sh.exe`).

### What had to change for npm (measured on VM1)

Three things stop npm in the LPAC token; each was found natively (logs under Evidence).

1. **A child with piped standard handles hangs.** Node 24.21.0 bundles libuv 1.52.1, whose
   `uv__pipe_server` creates `\\?\pipe\uv\<n>-<pid>` and retries with the next number on
   `ERROR_ACCESS_DENIED`, which an AppContainer always gets outside its own namespace: the call
   never returns. libuv 1.53.0 (2026-09-24, "win: fix unique named pipes to work inside Windows
   AppContainer") uses `LOCAL\` there; no Node release with it was available to test. The
   section above measured the same token: a pipe outside `LOCAL\` is refused with error 5, one
   inside it works (`asyncio-vm1-a3-pipes.log`), so that fix should apply; it is not measured with Node. A child
   that shares its parent's handles (`stdio: 'inherit'`) is not affected. npm therefore gets
   `npm_config_foreground_scripts=true`, which runs package scripts with npm's own handles; `npx`
   and `npm run` already do. **This holds for every Node program in the sandbox**, not only npm: an
   MCP server or script that uses `exec`, `execFile`, `execSync`, `spawnSync` with output capture,
   or `spawn` with pipes (the default) hangs. Example: esbuild's install script (`execFileSync`
   of its binary) hangs; with `--ignore-scripts` esbuild installs and its command works.
   (Since "Child processes of sandboxed Node" below, such a child is refused at once instead.)
2. **BusyBox cannot be started from inside the sandbox.** Its manifest sets the UTF-8 code page
   (`activeCodePage`), and Windows cannot build that activation context with the sandbox's token
   (the same cause as Python's 0xC0150002 in "Language runtimes" above). Node's `spawn` of
   `sh.exe` failed with `UNKNOWN` in every variant (inherit, ignore, detached, hidden, empty
   environment) while Node starting `node.exe` worked; Node's manifest has no setting.
   `add_private_npm` therefore primes `sh.exe` (resource 1) with `CreateActCtxW` outside the
   sandbox before each run, as P1 does for Python; npm's scripts and `npx` commands then start.
   Same caveat as P1: it relies on Windows reusing a context built outside, which Microsoft does
   not document.
3. **npm cannot check folders above its own.** npm's dependency-tree code checks every folder of
   a path from the drive root for links (`@npmcli/arborist/lib/realpath.js`), except below its
   working folder, which it seeds as known (`build-ideal-tree.js`). The sandbox may not `lstat`
   `C:\Users`, so `npx <package>` (npm's cache is outside the working folder) and `npm install -g`
   failed with `EPERM ... lstat 'C:\Users'`. The launchers load `npm-sandbox.cjs` first: when
   `lstat` of a folder *above* the working folder, npm's cache, `LOCALAPPDATA` or npm's global
   folder is refused (`EPERM`/`EACCES`), it answers "a folder, not a link". Every other result,
   and every refusal of any other path, is unchanged; no access is added. It affects npm started
   through `npm`/`npx` in the shell only (an MCP install runs in its own folder and does not need
   it). The answer is wrong only if such an ancestor is itself a link, which then resolves by its
   link path, as `--preserve-symlinks` already does for Node.

### Verification and grants

- `private_node()` is unchanged (node.exe, seed library, openssl.cnf). `private_npm()` checks, once
  per backend at the first use of npm: the launchers and the preload byte for byte, `sh.exe` against the BusyBox
  pin, and npm's tree against `NPM_TREE_DIGEST` (no links, no extra or missing file). It keeps the
  launchers, `sh.exe` and the tree's root folder open. Measured cost: see Evidence. A changed tree
  leaves the installation without npm (the shell's `npm` then says so); Node keeps working.
- `Sandbox::add_private_node` now grants the folder and its own files only, no longer the whole
  folder tree, so a run without npm does not pay for npm's 1,926 files.
- `Sandbox::add_private_npm` grants `node_modules` (the folder itself) and npm's tree read-only,
  primes `sh.exe`, and sets `npm_config_script_shell` (`sh.exe`), `npm_config_userconfig`
  (`<scratch>\.npmrc`), `npm_config_prefix` (`<scratch>\npm-global`),
  `npm_config_foreground_scripts=true` and `npm_config_update_notifier=false`. npm's cache
  defaults to `%LOCALAPPDATA%\npm-cache`, which is the run's scratch.
- Every sandboxed shell command and every terminal gets npm. Git runs of the Changes view do not.

### `npm` and `npx` in the shell and terminal

BusyBox finds `npm` and `npx` first on `PATH` (the private Node folder is first; BusyBox tries the
exact name before adding `.com`, `.exe`, `.sh`, `.bat`, `.cmd`, `win32/mingw.c`
`add_win32_extension`) and runs them as `#!/bin/sh` scripts. BusyBox passes `"$@"` to `node.exe`
with the C runtime's quoting (`win32/process.c` `quote_arg`), not through `cmd.exe`. Inside npm:

- Package scripts (`npm run`, install scripts) run in BusyBox `sh` (`npm_config_script_shell`),
  never `cmd.exe`, with npm's own standard handles. Scripts written for `cmd.exe` (`if exist`, `set X=`) fail there; POSIX scripts,
  which most packages use because they also run on macOS and Linux, work.
- `npx <package>` installs into npm's cache in the run's scratch and runs the package's command
  through the same script shell: npx joins the command with its arguments, quoted for `sh`
  (`libnpmexec/lib/run-script.js`, promise-spawn's `escape.sh`), and `sh` finds the extensionless
  `sh` shim that npm's `cmd-shim` writes next to the `.cmd` one in `node_modules\.bin`.
- Configuration: the user's `%USERPROFILE%\.npmrc` is not read (the user configuration is in the
  scratch, and the sandbox cannot open the real profile anyway); a project's own `.npmrc` in the
  granted folder is read, as npm always does. The cache, global installs and logs vanish with the
  run.
- Network: through the run's approved hosts only. npm honours `HTTPS_PROXY`
  ([npm config, `https-proxy`](https://docs.npmjs.com/cli/v11/using-npm/config#https-proxy)); the
  proxy only relays `CONNECT host:443` for approved hosts and does not intercept TLS, so npm's
  bundled CA list verifies the registry as usual. Approve `registry.npmjs.org` (the packument and
  the tarballs are served from it).

### MCP servers saved as `npx` or `npm exec`

Recognition ([`npm_launch::shim`](../../server/src/npm_launch.rs)): the last path component, in
any case, is exactly `npx`, `npx.cmd`, `npm` or `npm.cmd`, bare or as a path. Nothing at that path
is ever opened or run; the name only selects MiniClaw's npm. `npx.cmd.exe` stays a native program,
`npx.bat`, `npx.ps1`, `npx .cmd` and every other batch file stay refused.

- **Import** saves the bare name (`npx`/`npm`); **saving** accepts the bare name only on Windows and
  only when MiniClaw can start the arguments. macOS still saves the absolute path of the user's
  `npx` and runs it as before.
- **Arguments** ([`npm_launch::plan`](../../server/src/npm_launch.rs)): npx options end at the first
  positional or `--`, as npm documents for npx
  ([npm exec](https://docs.npmjs.com/cli/v11/commands/npm-exec)); everything after the package is
  the server's, byte for byte. Known options: `-y`/`--yes`, `--no-install`, `--offline`, `-q`,
  `--quiet`, `--silent`, `--prefer-offline`, `--prefer-online`, `--loglevel`, `--ignore-scripts`,
  `-p`/`--package`, `--registry` (https on port 443 only). `-c`/`--call`/`--shell` (a shell string)
  and unknown options are refused with a sentence. `npm exec`/`npm x` are read the same way; since
  npm itself parses options anywhere before `--`, an option meant for the command must follow `--`.
  `npm run`, `npm start` and other npm commands are refused. Packages must be registry
  specifications (`name`, `@scope/name`, with an optional version, range or tag); paths, URLs, Git
  shorthands and aliases are refused.
- **Install** ([`mcp_npm`](../../server/src/mcp_npm.rs)): on the first start, or when the packages,
  the registry, the scripts choice or the Node/npm versions change, MiniClaw's Node runs
  `npm-cli.js install --no-audit --no-fund --save-exact <packages>` in
  `<data>\mcp-npm\<server id>\pkg`, inside a fresh sandbox of its own with the server's approved
  hosts and its saved values. The folder is under MiniClaw's protected data folder, which no
  request can grant; `Sandbox::grant_owned` grants exactly this server's folder (read-write for the
  install, with inheritance recorded for cleanup like any writable grant). npm's user and global
  configuration (`<folder>\npmrc`, created empty; `<folder>\global\npmrc` as global configuration,
  since npm refuses one file for both), cache (`<folder>\cache`, kept between installs)
  and global prefix are all in that folder; `fetch-retries=1` with 2 to 10 second back-off and a
  60-second fetch timeout replace npm's defaults (2 retries, 10 to 60 seconds, 5 minutes), and the
  whole install is stopped after 300 seconds. The registry host must be one of the server's
  approved hosts or the start fails at once naming the host. A server marked unrestricted installs
  and runs without the sandbox, still with MiniClaw's Node and npm.
- **Run**: npx's choice of command (`libnpmexec` `get-bin-from-manifest.js`: the only `bin`, the
  only script all names share, or the one named like the unscoped package; with `--package`, the
  named command), a script path that stays inside the package, and a script Node can run (its `#!`
  line names `node`, directly or through `env`, and its options are passed to Node; without that
  line only `.js`, `.cjs`, `.mjs`). The server process is the private `node.exe` with
  `[<#! options>, <script>, <server args>]`, in the server's normal sandbox, with the installed
  package granted read-only so the server cannot change its own code for a later start. No npm
  runs at start: a warm start needs no network and no registry host.
- **Updates**: an install is reused until its key changes. `npx -y pkg` without a version therefore
  stays on the version installed first (macOS re-resolves `latest` on each start). Changing the
  version in the arguments, or deleting and adding the server, installs again. Deleting a server
  removes its folder.
- **Display**: the server list (`GET /api/mcp/servers`, and the answers of add, edit and import)
  carries `runs` on Windows for such a server, for example "On Windows, MiniClaw's Node runs the
  command of @upstash/context7-mcp. MiniClaw's npm installs it into this server's private folder
  from registry.npmjs.org at the first start, which needs registry.npmjs.org approved; never
  npx.cmd or cmd.exe." The MCP settings page shows it under the command.

### Install scripts and registry trust

Install scripts run, as npm runs them by default, in BusyBox `sh` with npm's handles inside the same sandbox as the
install: the same AppContainer token, job and approved hosts, writing only to the server's npm
folder and the run's scratch. Child processes inherit the token and job (process creation in
[`windows_process`](../../server/src/windows_process.rs) assigns the job atomically and children
of an AppContainer process stay in it). `--ignore-scripts` in the saved arguments turns them off.
Chosen over `ignore-scripts=true` by default because it matches macOS, npm's own install-script
policy of 11.19 applies unchanged, and nothing a script does escapes the sandbox; a package whose
scripts need `cmd.exe` fails to install with npm's message.

Registry trust is npm's: TLS to the approved registry with npm's bundled CAs, and npm's integrity
checks of each tarball against the packument. MiniClaw adds no signature check of the package.
What the package does is limited by the sandbox, not by trusting the registry.

### Evidence

VM1 (Windows 11 ARM64 26100.9457, UAC off, elevated owner task), `work/windows-vm`. Every log
names the guest, lists 299 sources hash-verified against the host tree, shows
`Compiling miniclaw-server` and artifacts newer than the run, and stages `node\` with the
production code (`windows-native-runtimes.mjs stage`: 8 files plus 1,926 npm files). Tests are in
[`windows_npm.rs`](../../server/tests/windows_npm.rs).

- `npx-vm1-native-1.log`: 17 of 19. Units `npm_launch` (10), `mcp_import`, `mcp_servers`,
  `windows_runtimes`; `private_npm_is_verified_and_a_changed_one_is_refused` (a changed npm file, an
  added file, a changed launcher and a non-BusyBox `sh.exe` are refused; first check 3.9 s in the
  debug build); `npm_grant_cost` (debug build: a shell run's sandbox 25 ms, npm's grant +0.75 s,
  2.1 s the first time; cleanup 0.1 s); `npx_arguments_reach_the_server_unchanged` (14 arguments
  with `& | % ^ "`, spaces, an empty one, a trailing backslash, Unicode, `$(x)`, `-y` and
  `--registry=` arrive byte for byte; the server is the private `node.exe` with the `#!` line's
  `--no-warnings`; writing its own script fails with `EPERM`); regression: private Node,
  `shell_node_*`, `sdk_server_runs_confined_with_the_private_node`, `offline_run_stays_closed`,
  `exact_proxy_only`, `networked_run_stays_closed_elsewhere`, `mcp_client::windows_tests`,
  `terminal::windows_tests`. Failed: the MCP install (npm refuses one file as user and global
  configuration: fixed) and the shell case (timed out: below).
- `npx-vm1-native-2.log`, `-4.log`: `npx_server_installs_from_the_registry_and_starts_offline_afterwards`
  passes. `npx -y @modelcontextprotocol/server-sequential-thinking` (2026.8.31) from the real
  registry through the server's proxy: cold start 53 s and 60 s (install of 4,276 files including
  npm's cache, debug build), `tools/list` has `sequentialthinking`, a call with `& | %PATH% ^ "`
  in its text answers; warm start 3.2 s and 2.5 s; the same server with no approved host 2.8 s and
  2.1 s; a new server without the registry approved is refused in 2 ms naming
  `registry.npmjs.org`; an approved but unreachable registry fails in 3.2 s and 2.1 s with npm's
  `E403` from the proxy. A planted `%USERPROFILE%\.npmrc` naming another registry and a token was in
  place: the install still used `registry.npmjs.org`, so it was not read. `%LOCALAPPDATA%\npm-cache`,
  `%APPDATA%\npm` and `%APPDATA%\npm-cache` were unchanged.
- `npx-vm1-native-2.log` to `-6.log`, shell (diagnosis): `npm --version` (11.19.0, both found in the
  private Node folder), `npm config get userconfig` in the run's scratch, `registry` the default
  with the planted `.npmrc`, which `cat` cannot open; `npm init -y`; `npm ping` and
  `npm install is-number@7.0.0` through the proxy, `require` works. Then: a package script hung
  (run 2), Node's spawn probes (run 3, 5: piped spawn never returns; `node.exe` starts, `sh.exe`
  and `busybox.exe` fail with `UNKNOWN` in all variants), `npx` failed with
  `EPERM lstat 'C:\Users'` (run 3) and `spawn UNKNOWN` (run 4). With the three changes above, run
  6: the local package's `postinstall` ran in BusyBox `sh` (`sh:postinstall`) and
  `npx --yes semver@7.6.3 1.2.3` printed `1.2.3`; esbuild's install script hung (`execFileSync`
  through a pipe), which is why the final test installs it with `--ignore-scripts`.
- `npx-vm1-native-7.log` (final, 302 sources verified): **19 of 19**. The units, the four
  `windows_npm` cases and the regression subset of run 1. Shell: `npm --version` 5.7 s for the
  first npm run of the backend (npm checked first), every later step about 2 s;
  `SPAWN-INHERIT ok` and `SPAWN-PIPED exit 124` (cut off after 15 s); `postinstall` in `sh`;
  `esbuild@0.25` with `--ignore-scripts` then `npx esbuild --version` printed `0.25.12` (its Go
  binary runs in the sandbox); `npx --yes semver@7.6.3 1.2.3` printed `1.2.3`; `fetch` of
  example.com with only the registry approved was refused (`refusedHosts: ["example.com"]`);
  `npm view` with no approved host failed in 1.6 s with `EAI_FAIL`. MCP: cold start 56.3 s, warm
  2.2 s, warm with no approved host 2.3 s, cold with no approved host refused in 8 ms, dead
  registry 2.1 s. Grant cost (debug build): npm's tree adds 1.0 to 1.2 s to a shell or terminal
  run (2.3 s the first time). The npm check took 4.3 s once.
- Host: Mac `cargo test --locked` (373 passed), Windows GNU `cargo check --all-targets`,
  `npm run check`, `npm run test:web` (112), `npm run test:setup` (260 passed, 13 skipped), and the
  touched node suites with the real Node and BusyBox archives (`npx/host-*.log`).

### Residual risks and open items

- `npm link`, `file:` dependencies and workspaces create junctions; the sandbox refuses links when
  it grants a folder, so the next sandboxed run in such a workspace fails until they are removed.
- The shell's npm cache is per run: every `npm install` in the shell downloads again.
- npm reads parent folders of the working folder for configuration and workspaces; folders above
  the granted one are not readable, which npm treats as absent.
- Native package binaries (a package whose command is an `.exe`) and packages whose command is a
  shell script are refused for MCP servers; inside the shell they run if they run in the sandbox.
- Every Node program in the sandbox hangs when it starts a child with pipes (above; now refused
  at once, see "Child processes of sandboxed Node"). Remedy: a
  Node release that bundles libuv 1.53.0 or later, measured natively first (not available yet;
  the private Node is pinned to 24.21.0). Until then such MCP servers do not work sandboxed.
- `sh.exe` priming depends on undocumented csrss caching (as P1).
- Agent proposals (`miniclaw-mcp-server` blocks) still need an absolute command; an agent cannot
  propose `npx` yet.

### Sources

- [npm exec](https://docs.npmjs.com/cli/v11/commands/npm-exec) (option order for npx, `--` for
  `npm exec`, `--yes` assumed without a TTY, choice of command),
  [npm config](https://docs.npmjs.com/cli/v11/using-npm/config) (`script-shell` default `cmd.exe`
  on Windows, `userconfig`, `globalconfig`, `cache`, `prefix`, `https-proxy` from `HTTPS_PROXY`,
  fetch retry defaults, `npm_config_*` variables are case-insensitive),
  [npmrc](https://docs.npmjs.com/cli/v11/configuring-npm/npmrc),
  [scripts](https://docs.npmjs.com/cli/v11/using-npm/scripts).
- npm 11.19.0 as shipped in the Node archive: `bin/npx-cli.js`, `libnpmexec/lib/run-script.js`
  and `get-bin-from-manifest.js`, `@npmcli/promise-spawn/lib/index.js` (`spawnWithShell`),
  `@npmcli/arborist/lib/realpath.js` and `build-ideal-tree.js` (realpath cache seeded with the
  working folder), `@npmcli/config/lib/definitions/definitions.js`.
- [libuv `src/win/pipe.c`](https://github.com/libuv/libuv/blob/v1.x/src/win/pipe.c)
  (`uv__pipe_server`), [ChangeLog](https://github.com/libuv/libuv/blob/v1.x/ChangeLog) 1.53.0;
  `node.exe` 24.21.0 contains `\\?\pipe\uv\%llu-%lu` and libuv `1.52.1`.
- busybox-w32 source (`win32/mingw.c` `add_win32_extension`, `win32/process.c` `quote_arg`,
  `libbb/appletlib.c` applet from the program name); BusyBox's manifest (`activeCodePage` UTF-8).
- [CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)
  and [Rust CVE-2024-24576](https://blog.rust-lang.org/2024/04/09/cve-2024-24576/) for why batch
  files stay refused; [npm `cmd-shim`](https://github.com/npm/cmd-shim) for the `sh` shims in
  `node_modules\.bin`.

## Child processes of sandboxed Node (2026-10-05)

Every Node program inside the sandbox stopped for good when it started a child process whose
standard input, output or error is a pipe (`exec`, `execFile`, `execSync`, `execFileSync`,
`spawnSync` and `spawn` with the default `stdio`) or a `fork` channel: npm install scripts that
capture a command's output, MCP servers that ask a helper for its version, build tools. **Result:
no official Node release fixes it yet; such a child is now refused at once with a clear reason
instead of hanging, and children with inherited, ignored or file handles keep working.**
MiniClaw stays on the official, unmodified Node.js 24.21.0 build; nothing in the token, the
capabilities or any access list changed.

### Cause (libuv source, measured)

- Node starts a child through libuv's `uv_spawn`. For every piped standard handle (and the
  `ipc` channel) libuv 1.52.1 creates a named pipe server under a unique name and opens its client
  end for the child (`uv__create_stdio_pipe_pair` -> `uv__create_pipe_pair` -> `uv__pipe_server`
  in [`src/win/pipe.c`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/pipe.c)). The name is
  `\\?\pipe\uv\<n>-<pid>` (`uv__unique_pipe_name`). `uv__pipe_server` loops `for (;;)`: when
  `CreateNamedPipeA` fails with `ERROR_PIPE_BUSY` or `ERROR_ACCESS_DENIED` it takes the next
  number and tries again, with no limit. An AppContainer may create named pipes only in its own
  namespace (`\\.\pipe\LOCAL\`, [Pipe names](https://learn.microsoft.com/en-us/windows/win32/ipc/pipe-names));
  every other name is refused with `ERROR_ACCESS_DENIED`, so the loop never ends. It runs on the
  calling thread inside `uv_spawn`, so the program's event loop, its timers and `spawnSync`'s
  own `timeout` stop with it.
- The fix is libuv 1.53.0 (2026-09-24, ChangeLog entry "win: fix unique named pipes to work inside
  Windows AppContainer (Russ Cox)", commit
  [`2cadaa4`](https://github.com/libuv/libuv/commit/2cadaa401670), PR
  [#5181](https://github.com/libuv/libuv/pull/5181)): `uv__unique_pipe_name` asks
  `GetTokenInformation(TokenIsAppContainer)` once and then names the pipe
  `\\?\pipe\LOCAL\uv\<n>-<pid>`. The retry loop itself is unchanged.
- Native, VM1, the production offline token (`libuv-vm1-d1-diagnostic.log`, probes under
  BusyBox `timeout`, the same probes unsandboxed as the control): with official Node 24.21.0
  (libuv 1.52.1, OpenSSL 3.5.8), a pipe server named exactly like libuv 1.52.1's is refused
  (`EADDRINUSE`, which is how libuv's `uv_pipe_bind2` reports `ERROR_ACCESS_DENIED`) and binds
  outside the sandbox; named exactly like libuv 1.53.0's (`\\?\pipe\LOCAL\uv\<n>-<pid>`) it binds,
  accepts and answers inside the sandbox, as `\\.\pipe\LOCAL\…` does. `spawnSync` with pipes and
  `exec` were cut off by `timeout` after 20 s (exit 124) in the sandbox and finished in 0.16 s
  and 0.21 s outside. So libuv 1.53.0's change applies to MiniClaw's LPAC token as well (its own
  CI runs the tests in an ordinary AppContainer, not an LPAC); it is not measured with a Node
  that contains it, because none exists.

### Official releases as of 2026-10-05

Only nodejs.org was used: [`dist/index.json`](https://nodejs.org/dist/index.json) (last modified
2026-09-23) lists libuv **1.52.1** for every current release: 24.21.0 (LTS "Krypton", 2026-09-07,
MiniClaw's pin), 26.10.0 (Current, 2026-09-21) and every 24.x and 26.x before them; 22.23.3 has
1.51.0. `deps/uv` on Node's `v24.x`, `v24.x-staging` and `main` branches is 1.52.1 without the
change. The update is Node pull request [#66282](https://github.com/nodejs/node/pull/66282)
("deps: update libuv to 1.53.0", opened 2026-09-25, open). No official binary could therefore be
downloaded and measured; the pins (archive, `node.exe`, npm tree) are unchanged.

### What can be done without rebuilding Node

- **No setting.** libuv 1.52.1 reads no environment variable or option for the pipe name: the
  prefix is a string constant in `uv__unique_pipe_name`, and Node passes no name for `stdio` pipes.
- **No namespace trick within the rules.** Making `\pipe\uv\…` creatable would need a token,
  capability or named-pipe access change, which this work may not make. A process cannot move its
  own pipe root.
- **A JavaScript replacement of Node's piped children is possible but not chosen.** Measured in
  the sandbox (`libuv-vm1-d1-diagnostic.log`; the `LOCAL` pipe case in `libuv-vm1-product-2.log`
  and `-3.log`, after a fix to the probe): a child whose standard handles are files works
  (1 MB of output and its standard error read back), so does a child of a child, and so does a
  pipe the program names itself in `\\.\pipe\LOCAL\` with the child's end opened as a file
  descriptor (echo of its input, 1 MB of output and standard error). A preload could rebuild
  `spawn`/`exec` on such pipes and `spawnSync`/`execSync` on files. It would replace a central
  part of Node for every sandboxed program (stream types, `close` ordering, `maxBuffer`, `kill`
  and `fork` messages would all be MiniClaw's code), for a problem the next Node release
  removes. Left as an option for the lead; not implemented.
- **Chosen: fail at once, with the reason.** See below.

### What MiniClaw does now

- [`child-pipes.cjs`](../../scripts/setup/node-sandbox/child-pipes.cjs) ships beside the private
  `node.exe` (`bin\node\child-pipes.cjs`). The process launcher appends
  `--require "<that file>"` to `NODE_OPTIONS` of every sandboxed process
  ([`windows_runtimes::node_environment`](../../server/src/windows_runtimes.rs); inside quotes
  `NODE_OPTIONS` reads `\` as an escape, Node's `ParseNodeOptionsEnvVar`, so the path is written
  escaped). The variable that names the file (`MINICLAW_NODE_PRELOAD`) never reaches a process;
  an unrestricted process gets nothing.
- On Windows with a libuv older than 1.53.0 the preload replaces `ChildProcess.prototype.spawn`
  (used by `spawn`, `exec`, `execFile` and `fork`) and the module's `spawnSync`, `execSync` and
  `execFileSync` (which call Node's internal `spawnSync` directly), then
  `syncBuiltinESMExports()` so ES modules see the same functions. A call whose `stdio` makes
  libuv create a pipe, as Node's `getValidStdio` reads it (the default, `pipe`, `overlapped`,
  `ipc`, a negative number, missing entries 0 to 2, or `input`), gets error code
  `ERR_MINICLAW_SANDBOX_CHILD_PIPES` the way Node reports a child that cannot start: `error`
  then `close` on the child, the callback's error for `exec`/`execFile`, `result.error` for
  `spawnSync`, a thrown error for `execSync`/`execFileSync`. Empty standard streams and a
  `send` that reports the error keep code that touches them before the event from crashing
  first. The first refusal of a process writes one line, `MiniClaw ERR_MINICLAW_SANDBOX_CHILD_PIPES: …`,
  to standard error. Every other child (`inherit`, `ignore`, file descriptors, streams) goes to
  Node unchanged. With libuv 1.53.0 or later, or outside Windows, the file does nothing, so a
  later Node pin retires it without a code change.
- The backend checks the preload byte for byte as part of the private Node (`load_private_node`:
  `node.exe`, seed library, `openssl.cnf`, `child-pipes.cjs`); a changed or missing file leaves
  the installation without the private Node, as for the other three.
- MCP servers: the backend now reads a Windows server's standard error (it discarded it before)
  only to look for that code. When a confined server stops before it answers, or does not answer
  in time, after its Node refused a child, the start fails with: "This server starts a child
  process, which the Windows sandbox cannot support with the bundled Node yet. MiniClaw's Node
  24.21.0 cannot create the pipes for a child's input and output inside the sandbox; a MiniClaw
  update with a newer Node is needed." A server that catches the error and goes on starts
  normally. Nothing else of the standard error is kept.
- npm keeps `npm_config_foreground_scripts=true`: package scripts still need npm's own handles,
  and an install script that captures a child's output now fails within seconds with the code,
  instead of hanging npm until the shell's or the install's time limit.

### Security review

- The preload runs inside the sandboxed process with its own rights and opens nothing; it only
  decides earlier what libuv would never finish. A program can remove it from its own
  `NODE_OPTIONS` for its own children, which then hang as before; nothing outside the sandbox
  changes.
- The preload's path is in the user's installation like the private Node; the backend refuses a
  changed file before it is offered to any run, and a sandboxed program cannot write it.
- The standard-error reader keeps at most the last 31 bytes between reads and one flag.

### Evidence (VM1, Windows 11 ARM64 26100.9457, UAC off, elevated owner; `work/windows-vm`)

Every product log lists its sources hash-verified against the host tree (304 to 306), shows
`Compiling miniclaw-server` and artifacts newer than the run, and stages `node\` with the production
code; tests are in [`windows_node_children.rs`](../../server/tests/windows_node_children.rs) and
[`setup-node-sandbox-children.test.mjs`](../../tests/setup-node-sandbox-children.test.mjs).

- `libuv-vm1-d1-diagnostic.log` (before the preload existed): the cause measurements above; every
  probe in the sandbox and outside. Also measured: a child with file descriptors for its handles
  (1 MB of output, standard error, input from a file) and a child of a child work in the sandbox;
  the `LOCAL` pipe probe printed nothing there and outside (its own bug: it listened for `close`
  too late), fixed for the later runs. The run's one failure was the test's expectation (`EACCES` instead of libuv's
  `EADDRINUSE`), corrected. The first sandboxed Node start took 7.4 s (later ones about 1 s).
- `libuv-vm1-product-1.log`: stopped while staging, before any test: frippery.org answered 403 for
  the BusyBox download. Later runs use the host's copies of the pinned BusyBox and ConPTY archives,
  which the staging code checks against their pins.
- `libuv-vm1-product-2.log`: 18 of 19. JS natively (preload 6, packaging 4), units
  `windows_runtimes::`, `windows_process::` (with `sandboxed_node_gets_the_preload`), `mcp_client::`;
  `node_child_pipes_diagnostic` with the preload: piped `spawnSync` and `exec` refused in 1.3 s
  (each including the sandbox's own start) instead of being cut off at 20 s, `\\?\pipe\LOCAL\uv\`
  binds; `node_children_in_the_shell_never_hang` (`execSync`, `execFileSync`, `spawnSync`, `spawnSync`
  of 1 MB, `exec`, `execFile`, `spawn`, `fork` all refused with the code within 273 ms of program
  time, one notice line; `inherit` exit 7, a file of 1 MB plus a grandchild's output, `NODE_OPTIONS`
  ending in the escaped `--require`); `mcp_server_with_a_helper_child_fails_fast_or_works`;
  regressions `private_node_is_verified_and_a_changed_one_is_refused` (changed and missing preload
  refused), `shell_node_runs_offline_with_the_private_node`, `shell_node_https_through_the_proxy`,
  `sdk_server_runs_confined_with_the_private_node`, `private_npm_is_verified_and_a_changed_one_is_refused`,
  `npx_arguments_reach_the_server_unchanged`, `shell_npm_and_npx_work_in_a_workspace` (the piped
  probe is now refused at once, exit 0, instead of exit 124), `npx_server_installs_from_the_registry_and_starts_offline_afterwards`
  (cold 49 s with the real registry), `offline_run_stays_closed`, `terminal::windows_tests`. The
  failure was `npm_install_script_with_a_piped_child_never_hangs`: npm stopped after 3.8 s with
  the code as intended, but the test looked for the word "postinstall", which npm 11.19 does not
  print; corrected.
- `libuv-vm1-product-3.log` (Defender real-time protection on): 4 of 4. The diagnostic again; the
  shell case; npm: `npm install` of a local package whose `postinstall` captures a child's output
  ends in 3.8 s with exit 1, the notice line and npm's "command failed"; MCP: the piped server's
  start fails in 11.4 s cold (the first sandboxed start of the test process; 7.4 to 9.3 s were
  seen for a plain first Node start, also before the preload) and in 0.91 s warm, with the
  sentence above; the server that catches the refusal starts in 0.88 s and reports
  `refused ERR_MINICLAW_SANDBOX_CHILD_PIPES`; the server whose helper writes to a file starts in
  0.93 s and gets `helper 1.0`. It also removed the two disposable services that runs d1 and
  product-2 had preserved after their failures (`windows-ci-stop-network.ps1`, exit 0), and its own.
- Host: Mac `cargo test --locked` 376 passed, 0 failed, 1 ignored; Windows GNU
  `cargo check --all-targets`; `npm run test:setup` 280 tests, 267 passed, 13 skipped;
  `npm run check` 0 errors; red evidence `libuv/host-rust-red.log` (new units before the code),
  `libuv/host-js-packaging-red.log`, `libuv/host-js-red-install-disabled.log` (the preload tests
  with `install` disabled: 4 of 6 fail). Logs in `work/windows-vm/libuv/`.
- Not run: VM2 (UAC on), a standard user, x64, the native acceptance script (the new tests are not
  in `scripts/windows-native-tests.psd1`, whose hash two scripts pin).

### Residual risks and open items

- Node programs that need a piped child do not work in the sandbox until MiniClaw ships a Node
  with libuv 1.53.0 or later; then update the three pins (`scripts/package-release.mjs`,
  `scripts/setup/windows-node-sandbox.mjs`, `server/src/windows_runtimes.rs`), the npm tree digest
  if npm changed, run `node_child_pipes_diagnostic` and the `windows_node_children` tests (they
  expect working children once libuv is fixed), and only then consider dropping
  `foreground_scripts` for npm.
- Not covered by the preload: a program that calls Node's internal bindings directly
  (`process.binding('spawn_sync')`), and native addons that call libuv's `uv_spawn` with pipes;
  those still hang until the shell's or the server's time limit.
- The MCP reason relies on the server's standard error; a server that closes or redirects it
  before the refusal gets the ordinary "did not answer" message.

### Sources

- libuv [`src/win/pipe.c` at v1.52.1](https://github.com/libuv/libuv/blob/v1.52.1/src/win/pipe.c)
  (`uv__unique_pipe_name`, `uv__pipe_server`, `uv__create_pipe_pair`, `uv_pipe_bind2` mapping
  `ERROR_ACCESS_DENIED` to `UV_EADDRINUSE`) and [at v1.53.0](https://github.com/libuv/libuv/blob/v1.53.0/src/win/pipe.c)
  (`uv__is_appcontainer`), [ChangeLog](https://github.com/libuv/libuv/blob/v1.x/ChangeLog) 1.53.0,
  [PR #5181](https://github.com/libuv/libuv/pull/5181).
- [AppContainer for legacy applications](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-for-legacy-applications-)
  (`TokenIsAppContainer`), [Pipe names](https://learn.microsoft.com/en-us/windows/win32/ipc/pipe-names).
- nodejs.org [`dist/index.json`](https://nodejs.org/dist/index.json) (`uv` of every release);
  Node's `deps/uv` on `v24.21.0`, `v26.10.0`, `v24.x`, `v24.x-staging` and `main`; Node
  [PR #66282](https://github.com/nodejs/node/pull/66282).
- Node v24.21.0 `lib/child_process.js` (`execSync`, `execFileSync` call the module's own
  `spawnSync`), `lib/internal/child_process.js` (`ChildProcess.prototype.spawn`, `getValidStdio`,
  the ENOENT path's `error` then `close`), `src/node_options.cc` (`ParseNodeOptionsEnvVar`), and the
  [`module.syncBuiltinESMExports`](https://nodejs.org/api/module.html#modulesyncbuiltinesmexports) documentation.

## Folders with items the user cannot manage (2026-10-06)

On the second test guest (UAC on, standard token) pressing Check connection on the Roblox bundle's
`roblox_studio` server, whose folder was `%LOCALAPPDATA%\Roblox`, left a sandbox record each time,
and the next start of MiniClaw stopped: "Windows sandbox recovery needs attention". Thirteen
WebView2 folders under Studio's profile were owned by `BUILTIN\Administrators` with access for
SYSTEM and Administrators only (Studio had once run elevated while UAC was off). The grant walk
stopped at the first of them after the record, the profile and the folder's inheritable entry
already existed; cleanup and every later recovery walked the folder again, could not open those
13 folders, and kept the record. All 802 recorded objects opened and none carried an AppContainer
entry (`work/windows-vm/final-test/journal-diag-vm2-1.log`, `roblox-folder-diag-vm2-1.log`).

Three changes, each with a native test: a launch is refused before anything exists; recovery
retires a record whose only uncertainty is an object MiniClaw never granted; and a record that
genuinely stays uncertain pauses sandboxed work only, not the backend.

### A refused folder leaves nothing behind

[`Sandbox::build`](../../server/src/windows_sandbox_native.rs) now walks every requested grant
first: the same identity-checked, link-refusing walk with the same limits (100000 objects, 64
levels, 120 seconds) opens every object for exactly what its entry needs (`guarded_acl_path` or
`guarded_linked_file` for the pin and identity, then `READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES`
for the ACL handle) and queues it. Only then are the record, the profile and the scratch folder
created; the writable roots are recorded, and the queue is recorded and applied in batches of 512
as before (one flush, one ACL-lock acquisition per batch; nothing is changed before its record is
flushed). An object that cannot be opened with access denied ends the launch with
[`windows_sandbox::unmanageable`](../../server/src/windows_sandbox.rs): "The folder contains items
the current Windows user cannot manage, starting with <path> (access denied). MiniClaw cannot share
that folder with the sandbox and changed nothing. …". A sandbox dropped at that point has no
profile, record or entry, so its drop does nothing. Other errors keep their own words.

A read-only grant is refused the same way. It stamps an entry on every object as a writable one
does, and MiniClaw has no read mode without entries: an LPAC token is not matched by `ALL APPLICATION
PACKAGES`, and MiniClaw does not rely on the broader package SIDs to give a run access. (An object
that already admits `ALL RESTRICTED APPLICATION PACKAGES`, such as most of `C:\Program Files`, is
readable by every LPAC process without an entry; skipping the stamp for such objects would be a
possible later refinement. It is not implemented.) Leaving unmanageable objects out silently would
hand the program a different folder than the user approved.

The refusal is an error type of its own (`Refusal`), so callers that otherwise keep reasons to the
log pass these words on: MCP Check connection and every server launch (`mcp_client::unprepared`,
also for MiniClaw's npm installs), the terminal (`api::terminal`, HTTP 409 `terminal_refused`), and
the shell and Git views, which already printed the error (`Sandboxed command refused: …`,
`Sandboxed Git refused: …`). The words name a path inside a folder the user granted, nothing else.

The walk now opens everything before the first entry, so a file created inside a granted folder
during that walk is neither stamped nor inherited (the root's inheritable entry is written with the
first batch). Before, the same happened to files created before the first batch of 512. The program
then lacks access to such a file; nothing is widened.

### Recovery: what an object nobody granted can carry

Grant records are flushed before their entries are written, and the grant code writes an entry
only through a handle it opened with `WRITE_DAC` before the record was written (`acl_handle` runs in
`allow`, `record` in `flush`). Revocation only removes entries. Under a writable root, a cleanup walk
also records the children it finds; those records now say `"discovered": true`.

An object that this user cannot open now, with access denied, and that has no grant record, can
carry the run's entry only in these ways:

1. MiniClaw wrote it. Impossible without a grant record (above).
2. It inherited the root's entry because it was created under a writable root after that entry was
   written. A program running as this user creates objects this user owns, and an owner may always
   open for `READ_CONTROL | WRITE_DAC` unless an `OWNER RIGHTS` entry says otherwise. So the object
   was created either by another account or an elevated program (the trusted-host boundary that
   "Retained unknown identities" already accepts), or by the sandboxed program itself with an
   explicit security descriptor that takes this user's access away: creating an object with any
   DACL needs no `WRITE_DAC`, only the right to add a child, which a writable grant gives.
3. A program with `WRITE_DAC` on it added the entry. The sandboxed program cannot: its rights never
   include `WRITE_DAC`, and an AppContainer token never gets more than its user part allows.

Existing objects never receive a parent's new entry: `NtSetSecurityObject` changes one object and the
kernel does not propagate. So for the incident's 13 folders (older than the run, never recorded) the
answer is "never changed". But the walk cannot tell an old object from a new one without trusting
timestamps, and case 2 shows that a run can make an object that keeps its own entry and that this user
cannot open. Before this change such an object kept the record forever and, with the old startup rule,
stopped MiniClaw at every start: a denial of service available to any sandboxed program with a writable
folder. It is not a cleanliness proof either way, so recovery records it as what it is.

The entry such an object may keep is inert in the same sense as an unknown identity's. The run's job
is proven stopped (the existing proofs), its profile is deleted and its name is reserved for ever in
`retired/`, so no MiniClaw run can hold that SID again. Another program that created a profile of the
same name would run as some user, and an AppContainer token never gets more than its user part allows:
that SID never adds access a token's user does not already have.

The rule, in [`recover_directory`](../../server/src/windows_sandbox_journal.rs) and
[`revoke::tree`](../../server/src/windows_sandbox_revoke.rs):

- The walk opens each child for ACL access as before. If that fails with access denied, it opens the
  child again relative to its folder with `FILE_READ_ATTRIBUTES` only. MS-FSA grants that right to
  anyone who may list the folder, whatever the child's own list says, so the open proves that the name
  is an existing object and gives its full identity, and nothing can be changed through it. Such a
  child is reported as unreadable, not as clean and not as an error. Any other failure, including the
  second open, is an error as before.
- A recorded object that cannot be reopened with access denied is an error (the record is kept) unless
  its record is a `discovered` one, which is reported as unreadable instead.
- An unreadable object whose identity has a grant record (not `discovered`) keeps the record: its entry
  was written and cannot be checked ("… was given the run's access entry and this Windows user can no
  longer open it to remove it").
- Otherwise, once every readable object is clean, scratch is removed and the profile deleted, each
  unreadable object is flushed into the record (as `discovered`) and a retirement certificate of
  version 2 names it as `{"kind": "Unreadable", "error": 5, "object": …}`. Version 1 certificates (only
  unknown identities) are unchanged byte for byte. Validation accepts `Unreadable` only in version 2,
  only for a `discovered`, non-root object that the record holds; it is re-validated at every start.
  ACL removal is not claimed for these objects. A directory entry stands for its subtree, which the
  walk could not list.

A per-object "applied" marker written after each entry was considered and not added: a crash between
the entry and the marker would leave an applied entry without a marker, so a missing marker proves
nothing. The record-before-change order and the rule above do not depend on one.

### Startup with unresolved records

`main` no longer stops when recovery keeps a record. The outcome is kept
(`windows_sandbox::set_recovery`); the gate's first check after the switch is
`Missing::Recovery(<reason>)`, so `/api/sandbox`, the terminal, the shell, Git views and sandboxed MCP
servers are refused with "Sandboxed commands, the terminal, Git views and sandboxed MCP servers are
paused: MiniClaw could not finish cleaning up after an earlier sandboxed run, and its records and
permissions were kept. Windows sandbox recovery retained N unresolved records; … First: <profile>:
<path or reason> … Chat still works. …". Chat, settings and everything that does not create a sandbox
work. Each launch's own admission check (`Journal::create`) keeps refusing as before and updates the
same state. `/api/sandbox` marks this case with `"recovery": true`; the permission settings then show
the reason with a Try again button ([`SandboxAccess.svelte`](../../web/src/SandboxAccess.svelte)),
which calls `POST /api/sandbox/repair`, which runs recovery again and reports the result. A restart
does the same. The records, the entries and the profiles are never touched by hand.

### Roblox Studio on Windows

`roblox_studio` cannot run inside the sandbox on Windows at all today: `StudioMCP.exe` listens on
`127.0.0.1:13469` for Studio's plugin and a lease with listening ports is refused ("Declared
listeners"); on ARM64, Studio and `StudioMCP.exe` are x64 and x64 programs do not start in the
sandbox ("x64 programs on ARM64"). The MCP client now refuses a sandboxed server with listening ports
before anything is prepared ([`declared_listener`](../../server/src/mcp_client.rs)), naming the way to
run it, "Run outside the sandbox" in its settings, and on ARM64 the x64 limit too. Before, the sandbox
was prepared first, which stamped the server's folder only to refuse the lease afterwards.

The bundle now registers the server without a folder on Windows: `cmd.exe /d /c
<LOCALAPPDATA>\Roblox\mcp.bat` (Roblox's documented entry point, rewritten by Studio at each update,
so no version-specific `StudioMCP.exe` path is pinned). A server's folder is the one folder a sandboxed
server may write, and Studio's folder holds its WebView2 sign-in profile, `mcp.bat` itself and, for a
per-user Studio installation, `Versions\version-*\StudioMCP.exe`; write access there would let a run
change programs that later run outside any sandbox (in MiniClaw's unrestricted mode or in another MCP
client). A path containing a character `cmd.exe` interprets keeps the previous working-folder form.
Resolving the newest `Versions\version-*\StudioMCP.exe` at launch would need product code that knows
Roblox's layout (`mcp.bat` itself does this, including a registry fallback); a static definition cannot.
The narrowest sandboxed set would be the version folder read-only (MiniClaw's install grant of the
program's folder) and a private working folder; it is moot until listening is supported, and a
per-machine Studio under `C:\Program Files` would be refused by the check above for a standard user.

The `.exe`-only rule is not bypassed by an explicit `cmd.exe`. The rule exists because
`CreateProcessW` hands a batch file to `cmd.exe`, which parses the arguments a second time (an
argument could start a second command). With `cmd.exe /d /c <launcher>` the program is a native image
and the whole command line is fixed in the server's saved definition: MCP tool calls travel over
stdio and never reach a command line. Whoever can edit that definition can already name any program.
The batch file's own `%*` receives nothing.

### Evidence (`work/windows-vm/journal-fix`)

Each native run applied the host tree with every file hash-checked, stamped changed files and printed
`Compiling miniclaw-server`; VM1 is Windows 11 ARM64 26100.9457 with UAC off (elevated owner), VM2 the
same build with UAC on (standard token).

- Red, VM1, the product as it was plus the new tests (`native-red-vm1-1.log`, `-2.log`): both startup
  tests fail because the backend exits ("recovery needs attention"); the refusal test fails with a bare
  "Access is denied" and leaves 2 records; the unreadable-descendant record is retained; the
  unreadable-grant reason is not marked for the user.
- Green, VM1 (`native-green-vm1-1.log` … `-4.log`): startup 6/6, `windows_gate::` 15, `windows_sandbox::`
  33 (journal 21, among them `unreadable_descendants_retire_and_unreadable_grants_stay`), `mcp_client::`
  tests, the three new integration tests, and the existing recovery and ACL tests
  (`inherited_orphan_recovers`, `new_descendants_are_revoked`, `blocked_subtree_is_unchanged`,
  `sandbox_links_do_not_block_cleanup`, `backend_exit_recovers_without_restart`,
  `failed_cleanup_retries_without_restart`, the three hard-link tests, `file_made_by_another_run_is_revoked`,
  `large_tree_is_revoked`). `stamp_cost_report` with the walk before the record: 50051 objects stamped in
  12.8 and 17.4 s, cleanup 3.6 and 2.7 s (VM1; earlier VM2 figures 15.9-17.0 s and 2.6-3.1 s).
- The first package with this change (`vm2-first-start-0.1.3-1.log`) started in the new paused state and
  kept both records: the identifying open asked for `SYNCHRONIZE`, which the real Administrators-only
  folders deny; the VM1 fixture had granted it to the owner. The open now asks for
  `FILE_READ_ATTRIBUTES` only, and the fixtures give the owner `DELETE` alone, as the real folders do.
- VM2, the user's installation updated to 0.1.3 (`vm2-update-0.1.3-jf3-1.log`): the first start
  retired both records (`vm2-first-start-0.1.3-2.log`, `vm2-journals-after.log`: certificates of version 2,
  proof `Prelaunch`, 13 `Unreadable` folders each, the 13 named in the incident, plus 3 unknown
  identities of a deleted Roblox cache file); the sandbox is available. A temporary server with folder
  `%LOCALAPPDATA%\Roblox` was refused with the unmanageable-item message naming
  `…\AutoLaunchProtocolsComponent\1.0.0.12`, and no record was written (`vm2-preflight-roblox-1.log`).
  The user's own `roblox_studio` entry answers the listening-port message with the ARM64 sentence and
  writes no record (`vm2-roblox-studio-check-2.log`). The release start check passed 8 of 8
  (`vm2-start-check-0.1.3-1.log`).

### Sources

- [MS-FSA 2.1.5.1.2.1, Algorithm to Check Access to an Existing File](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/82b364ce-6d7b-422f-8d88-4db32eea809a):
  `FILE_READ_ATTRIBUTES` is granted when the parent grants `FILE_LIST_DIRECTORY`, and `DELETE` when the
  parent grants `FILE_DELETE_CHILD`.
- [ACE strings: `OW` (OWNER RIGHTS)](https://learn.microsoft.com/en-us/windows/win32/secauthz/ace-strings) and
  [SID strings](https://learn.microsoft.com/en-us/windows/win32/secauthz/sid-strings).
- [CreateFileW, `lpSecurityAttributes`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew):
  the creator supplies the new object's security descriptor.
- [Roblox Studio MCP server](https://create.roblox.com/docs/studio/mcp): `%LOCALAPPDATA%\Roblox\mcp.bat`
  on Windows; stdio transport. The generated `mcp.bat` runs `Versions\version-<hash>\StudioMCP.exe` with a
  `ContentFolder` registry fallback ([Roblox DevForum report](https://devforum.roblox.com/t/generated-mcpbat-has-invalid-batch-syntax-%E2%80%94-mcp-server-fails-to-start-when-the-windows-path-contains-a-space-such-as-my-username/4659938)).
- `cmd /?`: quote handling after `/C` (rule 1 keeps a quoted executable path with spaces) and its
  special characters.
