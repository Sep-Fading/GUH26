# Host resource admission

The VM session reserves host capacity for each VM just before it boots; nothing is
reserved when MiniClaw starts. It uses one private ledger per host user at
`~/Library/Application Support/MiniClaw/Resources/reservations.sqlite`. Every
installation on this Mac shares that ledger.

See the [admission sequence](sequence.md).

## Entry points and settings

[`startPair`](../../scripts/vm/session.mjs) calls
[`reservePair`](../../scripts/vm/resource-admission.mjs) before creating relay
sockets or starting child processes. Admission failure reports the blocking
resource and starts neither VM.

```js
    reservation = await admit(pair, { initialLimits: limits });
    signal?.throwIfAborted();
```

The defaults are 50% of host CPU count, 50% of physical RAM and 25% of filesystem
capacity. CPU capacity rounds down. The installed launcher passes saved limits
only to initialize a new ledger. Existing global limits take precedence on later
starts, including starts from another installation.

The [setup wizard](../../scripts/setup/wizard.mjs) reads the current global limits
for its initial choices. Its paired-installation review states that the selected
limits apply to all MiniClaw installations managed by the host user. After approval,
[`prepareMacVm`](../../scripts/vm/prepare-mac.mjs) persists those percentages through
`configureResourceLimits`. This settings update can precede a failed provisioning
attempt. Changing percentages preserves existing reservations and does not stop
guests or delete files.

## Measured input

`reservePair` reads each selected guest's `Resources.json` and checks the actual
`Disk.img` length against its allocation. It requires owned regular files and
rejects group- or world-writable inputs. The collected storage includes the VM's disk,
resource configuration, native identity/owner files, auxiliary storage, and the attached
installation media.

Files are identified by device and inode, so repeated references count once. Disk
reservations use logical size, including unallocated sparse growth. Before each
admission, the collector refreshes every recorded file's physical allocation. A
missing or replaced recorded file keeps its full logical reservation. Known
retained files remain recorded even when compute admission is refused.

Host CPU count, physical RAM and filesystem capacity come from local Node OS and
filesystem APIs. The collector accepts only storage on the registry filesystem's
device. It does not combine separate APFS volumes. The free-space check compares
available bytes with remaining recorded sparse growth; reported reservations are
separate from observed CPU or resident-memory usage.

The inventory covers these selected and previously recorded files. It does not
discover unregistered installations, separate backup destinations or other images.
The gate applies to paired startup; provisioning, import and resizing use their
existing paths.

## Durable ownership

[`ResourceStore`](../../scripts/vm/resource-store.mjs) uses Node's built-in SQLite
module. Each change holds `BEGIN IMMEDIATE` through the inventory read, calculation
and commit. This serializes competing host processes. The database uses rollback
journaling, `synchronous=EXTRA` and `fullfsync=ON`.

```js
      this.#db.exec('PRAGMA busy_timeout=5000; PRAGMA journal_mode=DELETE;'
        + ' PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON;');
```

Every admitted start receives an opaque lease token. All states except `stopped`
retain compute. The session releases compute only after the corresponding native
supervisor reports `stopped` or `forced_stopped`, or when that supervisor was never
spawned. A plain process exit, signal or `force_stop_failed` keeps ownership
unresolved. Late completion from an earlier lease cannot release a newer owner.

```js
        if (vm.lease === lease) vm.state = confirmedStopped.includes(vm.id) ? 'stopped' : 'unknown';
```

The session waits for child shutdown before settling reservations. If ledger
settlement fails, session ownership remains locked. Closing a database connection
does not release its reservations. After a supervisor crash, committed reservations
remain; the next start refuses unresolved ownership. This path does not infer a
confirmed VM stop from a dead process ID.

## Inspecting and repairing the ledger

`miniclaw ledger` lists every recorded guest with its state, CPU and RAM, and totals
against the budget. A guest whose supervisor died stays `unknown` and keeps counting
against the CPU and RAM budget, which can refuse every later start.

`miniclaw ledger --reconcile` releases such guests. It acts on `unknown` rows only,
and only when no running process names the guest and `lsof` finds none of its files open.
It asks for each guest, copies the database to `reservations.before-<time>.sqlite`, then
marks the row `stopped`:

```js
// scripts/vm/resource-store.mjs, reconcile
if (vm?.state !== 'unknown') throw new Error('Only a guest in the unknown state can be reconciled');
```

Disks stay reserved and no row or file is deleted. `--yes` accepts every idle guest for
non-interactive use.

A deleted file keeps its full reservation, so deleting a VM's folder does not free its
disk budget. `miniclaw ledger --forget` drops a guest from the ledger when the guest is
`stopped` and its folder or its `Disk.img` no longer exists, together with its recorded
files and every other recorded file that is gone.
It copies the database first and touches nothing on disk:

```js
// scripts/vm/resource-store.mjs, forget
if (data.vms.find(row => row.id === id)?.state !== 'stopped') {
  throw new Error('Only a stopped guest can be forgotten');
}
```

A Mac-backend pair has no controller guest, so
[reservePair](../../scripts/vm/resource-admission.mjs) inventories only the guests the
pair has and refuses a role it lacks. The setup wizard reads the ledger before it builds
anything and stops with the exceeded budget named.

## From the settings page

The page cannot open the ledger, which belongs to the Node VM session. The backend asks
the session through its control socket with four signed requests: `ledger`, `limits`,
`reconcile` and `forget` ([control.mjs](../../scripts/vm/control.mjs),
[ledger_request](../../server/src/worker.rs)). They name no VM. `limits` carries
`"cpu,memory,disk"` as whole percentages from 1 to 100, and that text is part of what is
signed:

```js
// scripts/vm/control.mjs
+ `${requestId ? `:${requestId}` : ''}${value.limits ? `:${value.limits}` : ''}`).digest();
```

The page asks the user first, so the session accepts every idle or deleted guest it finds,
as `--yes` does. `GET /api/virtualisation` adds the Master VM's status and whether its
worker's protocol number (`worker::PROTOCOL`) is current.

## Verification

[`tests/vm-resources.test.mjs`](../../tests/vm-resources.test.mjs) covers persistence,
pair admission, separate-process contention, committed-owner crashes, rollback of
an interrupted transaction, global limits, stale leases, retained disks, free space,
invalid inputs and private database paths. Disposable native-file fixtures exercise
the collector; fake supervisors exercise startup refusal and shutdown confirmation.
These tests do not boot real VMs.
