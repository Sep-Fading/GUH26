# Backups (experimental)

> **Experimental.** This is a small first version that is not being built out for now. It has
> only been run against test fixtures, never against a real VM. Do not rely on it as your only
> copy of anything.

A backup copies the Workspace folder of one VM to this Mac. It is not a whole-disk backup.

## Off by default

Nothing happens until the user acts. A VM nobody configured has schedule `off`, no settings
file is written, and no backup folder is created. Each VM has its own switch; turning one on
leaves every other VM off. A schedule never starts a VM: it runs only while MiniClaw runs and
the VM is running, and a missed run happens at the next chance.

## What it does

[`backups.rs`](../../server/src/backups.rs) reuses the [file transfer](../file-transfer/README.md)
job to copy `workspace` into `<data>/backups/<vm>/<time>-<id>.incomplete/files/Workspace`.
It then writes `manifest.json` with each file's size and SHA-256, and renames the folder to
its final name. A folder without a manifest is not a backup and is removed on failure.

- **Schedule:** Off, Daily or Weekly, plus "Back up now". Checked every 20 s by the same
  watcher as idle shutdown.
- **Retention:** the newest 3 stay (1 to 10). Older ones move to the Trash after a new backup
  succeeded, never before.
- **Failure:** three retries, 60 s apart, then a `backup_failed` notification. The reason is
  shown in the VM's backup section. A failed schedule waits a full period before trying again.
- **Restore:** every file is checked against the manifest first; a mismatch refuses the
  restore. The files are sent to `Transfers/restore-<id>` in the VM. The Workspace is never
  overwritten.
- One backup per VM runs at a time.

The section appears under each VM's controls
([`VmBackups.svelte`](../../web/src/VmBackups.svelte)) with an "Experimental" label.

## Routes

`GET /api/vms/{id}/backups`, `POST .../backups/settings` `{ schedule, keep }`,
`POST .../backups/run`, `POST .../backups/{backup}/restore`.

## Not built

Whole-VM backups, a destination picker and external drives, choosing folders in the page
(the settings hold a `folders` list, but only the whole Workspace is offered), backups in the
global disk budget, guest-side checksums, and VM start or stop failures in Notifications.

## Tests

`server/tests/backups.rs` (off by default, whole backups with checksums, retention to the
Trash, retries, checked restore), `backup_routes_start_switched_off` in
`server/tests/environment_routing.rs`, and "backups are labelled experimental..." in
`tests/browser/vm-lifecycle.spec.ts`.
