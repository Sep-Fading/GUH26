# File transfer between the Mac and a VM

Files move in both directions through the interface. There is no shared folder and no
general file API: the VM offers two folders, and the Mac side is whatever the user chooses in
the macOS panel.

## What the user sees

The desktop panel of a VM chat has a "Transfer files" button
([`Transfers.svelte`](../../web/src/Transfers.svelte)).

- **To VM.** "Choose files on this Mac" or "Choose folders on this Mac" opens the macOS
  panel. The files land in `/Users/Shared/MiniClaw/Transfers/<date>/` in the VM.
- **From VM.** A small list shows the VM's Workspace and Transfers folders and nothing else.
  After ticking files or folders, "Save to this Mac" opens the macOS panel for the
  destination folder.
- A name that already exists asks each time: Replace, Keep both ("name 2.ext") or Skip, with
  "Apply to all" for the rest of that transfer.
- Progress shows files and bytes. "Cancel transfer" stops between two pieces.

## Why an agent cannot start a transfer

The backend opens the panel with `/usr/bin/osascript` (`choose file`, `choose folder`) in
[`Picker::choose`](../../server/src/transfer_jobs.rs). The panel belongs to macOS, so neither
the page nor a model can answer it. The chosen paths stay in the backend under a selection
identifier that lasts ten minutes and serves one transfer. The page receives names and sizes
only. `POST /api/vms/{id}/transfers` has no field for a Mac path; a request that adds one is
refused as malformed. A selection made for sending cannot be used as a destination.

Not yet observed on a real Mac: whether the panel opens in front of the browser. The dialog
says it may open behind the window.

## The VM side

The VM worker answers `miniclaw/transfer/{list,exists,room,read,write,discard}`
([`transfer.rs`](../../server/src/transfer.rs)), and only in a VM worker.

- Two folders are offered by name: `workspace` (read) and `transfers` (read and write).
- A path is relative, holds plain names only, and may not pass through or name a link.
  Listings leave links out. Files are opened with `O_NOFOLLOW`.
- A write appends to `<name>.miniclaw-partial` and must arrive at the partial copy's current
  length. The last piece renames it to its real name. An existing file is kept unless the
  user chose Replace.
- `room` refuses a transfer that would leave less than 1 GiB free on the VM's volume.
- A piece is at most 512 KiB.

This needs worker protocol 3. A VM with an older worker answers the list request with an
error, and the page says to run `miniclaw update-vm`.

## The job

[`Transfers`](../../server/src/transfer_jobs.rs) runs one background job per transfer.

| Case | Result |
|---|---|
| Link on the Mac side | Counted as skipped, never copied |
| More than 10,000 files or 32 folder levels | The job fails before copying |
| File changes while it is copied | Size and change time are compared before and after; the job fails |
| Worker disconnects | The job fails with "The VM disconnected during the transfer." |
| Cancel | The job ends as `cancelled`; the partial file is removed |
| Any failure | Files that arrived whole stay; the unfinished file never gets its name |

Starting a transfer counts as use of the VM for [idle shutdown](../vm-lifecycle/README.md).

## Routes

| Route | Purpose |
|---|---|
| `POST /api/vms/{id}/transfers/pick` `{ kind: files, folders or destination }` | Open the macOS panel |
| `GET /api/vms/{id}/transfers/files?folder=&path=` | List an offered VM folder |
| `POST /api/vms/{id}/transfers` | Start a transfer from a selection |
| `GET /api/transfers/{job}` | Progress, open conflict or outcome |
| `POST /api/transfers/{job}/resolve` `{ choice, all }` | Answer a name conflict |
| `POST /api/transfers/{job}/cancel` | Stop the transfer |

## Tests

- `server/tests/transfer.rs`: traversal, links, piece order, replace rule, partial files,
  free space.
- `server/tests/transfer_jobs.rs`: both directions with real files against the same code the
  worker runs, all three conflict answers, "apply to all", a refused path, cancellation.
- `server/tests/environment_routing.rs`: the routes, selection reuse and a forged path.
- `tests/browser/transfers.spec.ts`: the dialog in both directions, conflict, failure, cancel.

Not observed yet: a transfer with a real VM, the real macOS panel, and a worker disconnect
in the middle of a real copy.
