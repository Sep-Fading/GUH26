# VM lifecycle: power, idle shutdown, deletion and orphans

A VM is Master or a project VM. This page covers what happens to a VM after setup: who may
start and stop it, when it shuts itself down, and how its disk is deleted. Setup is in
[installation](../installation/README.md); the budget is in
[resource admission](../resource-admission/README.md).

## Power controls

[`VmControls.svelte`](../../web/src/VmControls.svelte) shows one VM's state with Start, Open
window, Shut down and the idle time. It appears in three places: Master and every project VM
under Settings, Virtualisation, and a project's own VM in its Project settings. There is no
separate page for running project VMs.

`POST /api/vms/{id}/power` with `{ "action": "start" | "stop" | "show" }`
([`vm_power`](../../server/src/api.rs)) relays the signed `start`, `stop` and `show` control
requests. It needs no approval, because the user pressed the button. A message that needs a
stopped VM still goes through the prepare and start approval routes.

## Idle shutdown

[`idle.rs`](../../server/src/idle.rs) keeps one limit per VM from a closed list: off, 15, 30,
60 or 120 minutes. Off is the default. The limits are saved in `<data>/vm-idle.json`, mode
0600. `GET` and `POST /api/vms/{id}/idle` read and save one limit.

A watcher checks every 20 s. For each running VM with a limit, `step` decides:

| Situation | Result |
|---|---|
| A turn is in progress on a chat that runs in the VM | Wait, and count it as use |
| Last use is more recent than the limit | Wait |
| Past the limit, no warning open | Warn |
| Warning open for less than 60 s | Wait |
| Warning open for 60 s | Stop |

Use is: a turn on a chat in the VM, opening the desktop view, a desktop action, a power
request, saving the idle time, and "Keep running". Frames of an open desktop view do not
count, because a menu bar clock alone would keep a VM awake for ever. A stopped VM starts a
fresh idle period when it next runs.

A warning is temporary state, not a saved notification. `GET /api/vms/idle` lists open
warnings with the seconds left. [`IdleWarnings.svelte`](../../web/src/IdleWarnings.svelte)
polls it every 10 s and shows an alert with "Keep running", which posts to
`/api/vms/{id}/idle/keep`. With no answer the watcher sends the normal `stop` request, which
is the clean shutdown path, and saves a `vm_idle_stopped` notification.

Known limit: typing in the VM's native window is not seen by the backend. Opening that window
from MiniClaw counts as use once; after that, only the warning protects a session in the
native window.

## Deleting a project

`POST /api/projects/{id}/delete` takes `{ revision }` and optionally
`{ deleteVm: true, confirmName }` ([`delete_project`](../../server/src/api.rs)).

- The project's settings and project-scoped permission rules are deleted.
- Its conversations are deleted too, at their provider and in MiniClaw, the same way a single
  conversation is deleted. Every conversation is checked before the first one is deleted: a
  busy one refuses the request, and one whose VM is off refuses it with
  `conversations_unreachable`, because its provider cannot be asked. The page then says to
  start the VM or to delete the VM with the project.
- A conversation that runs on the VM being deleted needs no provider: its copy goes with the
  disk, and MiniClaw drops its own record and its offline transcript.
- `deleteVm` is accepted only for a project with its own VM, only when `confirmName` equals
  the project's name, and only while the VM is not running, starting or stopping. Master can
  never be named. The VM is deleted first, so a failed VM deletion keeps the project.

[`ProjectDelete.svelte`](../../web/src/ProjectDelete.svelte) leaves "Also delete this
project's VM" unchecked, disables it while the VM runs, and asks for the typed name. After
the deletion the page drops the project's conversations and leaves one that was open.

## Deleting a VM's disk

Deletion is permanent. Nothing goes to the Trash. The chain is:

1. The backend checks the state and sends `miniclaw/vm/delete` to its Mac worker.
2. The worker signs a `delete` control request. The
   [control service](../../scripts/vm/control.mjs) requires a VM identifier, so Master cannot
   be deleted, and refuses a VM that is running, starting or stopping.
3. The session calls [`registry.remove`](../../scripts/vm/dedicated-registry.mjs). It refuses
   a VM whose setup is still running, deletes `dedicated-vms/<vmId>/`, then removes the
   record. The folder goes first, so a failed deletion can be tried again.
4. The session forgets the deleted guest in the ledger, which frees its share of the budget.
5. The backend drops the VM's worker link and its idle limit.

## Orphaned VMs

A project VM that no project owns is orphaned. `GET /api/virtualisation` lists every project
VM as `{ vmId, state, projectId, projectName }`; an orphan has `null` for both. Settings,
Virtualisation shows orphans with the same controls plus "Delete VM", which needs a
confirmation and a stopped VM. `POST /api/vms/{id}/delete` refuses a VM that a project still
owns with `vm_owned`. There is no way to give an orphan to another project.

## Tests

| Behaviour | Test |
|---|---|
| Registry removal, refused during setup | `tests/dedicated-registry.test.mjs` |
| Signed `delete`, never Master, never a running VM | `tests/vm-control.test.mjs` |
| Session deletes the folder and record end to end | `tests/vm-status.test.mjs` |
| Project delete, orphan listing, power, VM delete rules | `server/tests/environment_routing.rs` |
| Idle rule, warning, keep, stop and notice | `server/tests/environment_routing.rs` |
| Project controls, delete dialog, orphans, idle alert | `tests/browser/vm-lifecycle.spec.ts` |

None of this has been observed with a real project VM yet.
