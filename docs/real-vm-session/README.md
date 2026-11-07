# Checking MiniClaw against the real VM

Much of what was built in September 2026 has only run against fixtures and real models, never
inside the VM. This page is the session that closes that gap. It takes about 20 minutes, most
of it the VM's own update.

## 1. Update the VM (you)

The VM's worker must be the current one (protocol 4) for file transfer, backups, Files and
Changes in a VM conversation, and for creating new files from a conversation in the VM.

1. Shut the VM down if it runs: the power button of the desktop panel, or Settings,
   Virtualisation, Shut down.
2. In a terminal: `miniclaw update` and then `miniclaw update-vm`. Add `--computer-use` to
   the second if you want Computer Use in the VM. Neither takes extra disk space.
3. Restart `miniclaw`.
4. Start the VM: Settings, Virtualisation, Start. Log in to macOS in its window.
5. In the VM, open the MiniClawSetup disk and run "Install Codex.command". It installs the new
   worker. Approve Screen Recording for the worker if macOS asks.

## 2. Run the checks (script)

```text
miniclaw verify          no model call
miniclaw verify --chat   also one small message to the VM's Codex (never the largest model)
```

[`verify-live.mjs`](../../scripts/vm/verify-live.mjs) uses the same local API as the page. It
never starts or stops a VM, and it deletes the one conversation it creates. Each line says
what to do when it fails.

| Check | What it proves |
|---|---|
| Running and signed in | The backend answers and both providers are signed in |
| Master VM running, logged in, Codex signed in | The worker reached the Mac over the VM link |
| Worker is current | The guest's worker reports the protocol this MiniClaw expects (4) |
| Desktop stream | At least 20 KB of frames arrive within 15 s |
| Transfer folders | The guest's confined file code lists Workspace and Transfers |
| Idle setting | Saved per VM and restored to what it was |
| VM conversation | A chat is created on the VM, not on the Mac |
| Files | The VM's Workspace is listed through the sandboxed executor |
| Changes | Git runs sandboxed inside the VM |
| Backup and restore | A Workspace backup appears; its checked restore lands in `Transfers/restore-<id>` |
| `--chat` | The Codex inside the VM answers one message |

## 3. What needs your eyes (you)

The script cannot see these. Each takes a minute.

| Try | Expect |
|---|---|
| Desktop panel, Transfer files, Choose files on this Mac | The macOS panel opens. Note whether it opens in front of the browser. After Send to VM the files are in `/Users/Shared/MiniClaw/Transfers/<date>` in the VM |
| Same dialog, From VM, tick a file, Save to this Mac | The macOS folder panel opens; the file arrives |
| Send the same file again | The name conflict asks: Replace, Keep both, Skip |
| Settings, Virtualisation, idle time 15 min, then leave the VM alone | After 15 min an alert counts down 60 s; "Keep running" keeps it; otherwise the VM shuts down cleanly and a notification says so. Set it back to Never afterwards |
| In a VM conversation with an agent, switch Coding session on and ask for a small edit in the Workspace | The review shows the diff; nothing changes in the VM before Apply |
| Workspace panel, Files and Changes in that conversation | The VM's Workspace, and its git changes if it is a repository |
| Shut the VM down from the panel | It stops within about 10 s, without the 90 s forced stop |

Tell me what differs from "Expect" and I will fix it.
