# Terminal

Every conversation with a working folder has one shell, shown in the "Terminal" tab of the right
sidebar. The user types into it, and the agent's approved commands run in it too. It
is `zsh` on a pseudo-terminal, started in the working folder under the same Seatbelt profile as
the conversation's commands, with a private home and a fixed PATH. Nothing typed there reaches
more than a command could.

Sequence: [sequence.md](sequence.md).

## The shell

[`terminal.rs`](../../server/src/terminal.rs) `Shell::spawn`:

- A pseudo-terminal from `rustix-openpty` 0.2; the shell is the session leader with the
  terminal as its controlling tty (`setsid`, `TIOCSCTTY` in `pre_exec`), and it starts in the
  working folder through the same pinned descriptor as a command (`executor::open_folder`).
- A private `Scratch` folder (as each command gets) holding `home/` with a generated `.zshrc`,
  `run/` for the agent's command files, and the shell's temp files. `HOME` and `ZDOTDIR` are that
  home; the user's real home is never given, and the protected roots are blocked as ever.
- `zsh -i`, not `-l`: a login shell runs `/etc/zprofile`, whose `path_helper` widens PATH
  beyond the sandbox's `/usr/bin:/bin:/usr/sbin:/sbin`.
- One `egress::Proxy` for the shell's life, on a port baked into the profile. Its allowlist can
  change (`set_hosts`): outside an agent's command it holds the hosts allowed by rule, only while
  the conversation's Network switch is on (`Backend::conversation_grants`,
  `permissions::host_grants`); otherwise nothing.
- `seatbelt::command_terminal` is `command_with` plus read, write and ioctl on the shell's own
  tty device (a `-D` parameter, never in the profile text) and on `/dev/tty`. Without those the
  line editor's `tcgetattr` fails. `(allow pseudo-tty)` is not granted: `tmux`, `script` and
  `expect` do not work in it (TODO).
- The prompt names the folder under the working folder (`site`, `site/src`), because the real
  path is long and the private home makes `%~` useless. The `.zshrc` also prints an invisible
  marker at every prompt (`ESC ] 7331 ; prompt BEL`; xterm shows unknown OSC numbers as nothing),
  which slice 2 uses to wait for a prompt before typing an agent's command.

## Output and the page

Output goes into a `Ring` of the newest 256 KiB with a running byte offset. A pump task reads the
terminal, appends, and bumps a `watch` that readers wait on. When the shell ends (the user typed
`exit`) the exit code is recorded and the next open starts a fresh shell with an empty ring.

`GET /api/threads/{id}/terminal/stream?after=N` ([`api/terminal.rs`](../../server/src/api/terminal.rs))
is its own server-sent event feed, apart from the chat's, so keystrokes never crowd out chat
events. `output` events carry base64 bytes with the offset reached as their id, `reset` says the
offset had left the ring (the whole ring follows), `exit` the code, `notice` words to show once.
`EventSource` reconnects by itself with `Last-Event-ID`, so the page keeps no reconnect state.

| Route | Does |
|---|---|
| `POST …/terminal/open {cols, rows}` | starts the shell if needed, sizes it; `409 no_working_folder` without a folder, `409 terminal_unavailable` in a VM whose worker predates the terminal |
| `POST …/terminal/input {text}` | types, 4 KB at most |
| `POST …/terminal/resize {cols, rows}` | 20..500 by 5..200 |
| `POST …/terminal/restart` | ends the shell and starts one with an empty screen |
| `POST …/terminal/close` | ends it |

The shell is closed with the conversation, after 30 minutes without use, and at backend exit.
`Terminals::ensure` restarts a shell whose grants or folder changed and leaves a notice.

The page: [`terminal-stream.ts`](../../web/src/terminal-stream.ts) wraps the feed;
[`Terminal.svelte`](../../web/src/Terminal.svelte) loads `@xterm/xterm` 6 and the fit addon by
dynamic import (jsdom cannot load them), sends what is typed, fits on resize, and offers "Restart
shell". Its colours are chosen so every ANSI colour reads at 4.5:1 on the background; axe checks
the terminal like any text and rejected xterm's default blue. CSP is unchanged.

Activity records nothing of what the user types. Both providers: the shell belongs to the
conversation, not to a provider; the agent path (slice 2) is tested with both.

## The agent's commands

Nothing changes before a command runs: the model calls `run_shell`, the permission lookup and the
popup with the exact text and folder are as before, `coding_session` re-asks, hosts are approved
with the command. What changes is where it runs. In `scoped_tool_result`
([`chat.rs`](../../server/src/chat.rs)), a shell command of a Mac conversation with a working
folder goes to `Backend::run_in_terminal`; anything else, and a shell that could not start, takes
the one-shot path exactly as before.

`Terminal::run` waits for the shell to be at a prompt (the prompt marker is the last marker and no
line was accepted since; `ESC [?2004l`, which zsh prints when a line is accepted, is the tell),
for up to 60 s, else the model is told the terminal is busy with what the user is running. It
writes the command to `<scratch>/run/<call>.zsh` (0600; the first line is the approved folder as
a comment), sets the proxy's hosts to the approved ones, and types `Ctrl-U __mc <call>` as the
user would. `__mc` erases its own echoed line, prints the folder and the command dimmed so the
user sees what runs, `cd`s to the approved folder, prints the start marker, `source`s the file,
and prints the end marker with `$?`. The command runs in the shell itself, so `cd` and `export`
persist, and `exit` inside a command ends the shell: the output so far comes back with the
shell's code and a note, and the next use starts a fresh shell.

The capture is the ring between the two markers, `\r\n` turned into `\n`, cut at 60 KiB with a
note (under the tool result's own 65 000-byte bound); the command is never stopped for size.
After 120 s (`MINICLAW_TEST_TERMINAL_TIMEOUT_MS` in debug builds) the result carries the output so
far with no exit code and "still running in the terminal"; a task keeps watching and finishes the
Activity entry under the same id when the end marker, or the shell's exit, arrives
(`activity::Log::finish_late`). The result JSON keeps the one-shot shape, so Activity and the
tool-result text are unchanged. The proxy's hosts return to the idle set afterwards.

A change of grants or folder restarts the shell at the next command or open, with a notice on
the stream. Both providers take this path: Codex through `item/tool/call`, Claude through the
shim, and both are tested.

## In a VM

A VM conversation's shell runs inside the VM, started in the VM's shared workspace with the
grants its commands get, under the same code: the worker answers `miniclaw/terminal/open`,
`input`, `resize`, `close`, `restart`, `read {after, wait}` and `run {id, text, cwd, hosts}`
([`terminal.rs`](../../server/src/terminal.rs) `guest_call`) from its own `Terminals`, keyed by
the conversation id the Mac names. The Mac keeps the ring: `Terminals::ensure_remote` opens the
shell in the VM and starts a pump that long-polls `read` (a second at a time, like desktop
frames), so the page's routes and the agent's path are the same as for a Mac shell. A command runs
through `run`, whose answer the Mac shapes like the one-shot executor's; a late finish inside the
VM has nobody to tell, so the partial result is what the model gets and the terminal shows the
rest.

The worker names the feature: `miniclaw/worker/info` now answers `{protocol: 6, tools,
features: ["terminal"]}`, and `Backend::machine_carries(assignment, "terminal")` gates both the
tab and the agent path. A worker installed before this answers no features: the tab says to run
`miniclaw update-vm`, and the conversation's commands take the one-shot path as before. The
worker runs the terminal methods only when it applies the guest policy, which the real guest
worker and the tests' stand-in both do; the Mac's own worker does not.

## Checked by

`tests/terminal.test.mjs` with the real shell and sandbox: the prompt and `pwd` in the working
folder, the fixed PATH and private HOME, `cat` of a file outside the folder and of MiniClaw's own
files refused, a write inside allowed, a late reader gets the screen, offsets, `exit` and restart,
input and size bounds, closed with the conversation, and no terminal without a folder. Agent
commands: an approved `run_shell` is captured with its exit code and shown dimmed in the terminal,
`exit 3` inside it ends the shell and is reported, `cd` and `export` persist for the user, a
command's cwd is the approved one whatever the shell was in, 100 KB of output is cut at 60 KiB for
the model and whole in the terminal, a command past the limit is reported as still running and
its Activity entry finishes later, a permission change restarts the shell with a notice, and the
same through the fake Claude CLI. Unit tests: the ring's offsets and reset, the marker parser
across chunks, the capture between markers. `tests/browser/terminal.spec.ts`
on both sizes with axe; `web/tests/terminal-stream.test.ts`. VM: `tests/host-vm.test.mjs` through the
stand-in worker, which is the real worker code: the shell echoes from the VM, starts in the VM's
workspace, the agent's approved command runs in it and is captured, `workerCurrent` is true; a
worker reporting the old protocol gets `409 terminal_unavailable` naming `update-vm` and its
commands take the one-shot path. Not seen in a real VM yet.
