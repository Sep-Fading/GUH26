# Claude Code installation during setup

Sources were read on 2026-10-04. Applies to the Windows ARM64, Windows x64 and
macOS (Apple silicon) releases. Code: `scripts/setup/claude-install.mjs`,
tests: `tests/setup-claude-install.test.mjs`.

## Decision

MiniClaw does not ship Anthropic's binary. When Claude Code is missing, setup
prints one notice and, without a question, downloads Anthropic's build from
Anthropic in the background, verifies it and runs Anthropic's own installer for
the current user while the wizard asks its next question. The wizard waits for it
only before the provider step. Sign-in stays the user's action in MiniClaw.

Rule applied: bundle only if the official terms clearly permit third-party
redistribution. They do not:

| Source | Text |
| --- | --- |
| npm `@anthropic-ai/claude-code` `LICENSE.md` (2.1.289) | "© Anthropic PBC. All rights reserved. Use is subject to the Legal Agreements outlined here: https://code.claude.com/docs/en/legal-and-compliance." `package.json` has `"license": "SEE LICENSE IN README.md"`. |
| winget `Anthropic.ClaudeCode` locale manifest (2.1.286) | `License: Proprietary`, `Copyright: © 2026 Anthropic PBC` |
| [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) D.4 | Customer may not "resell the Services except as expressly approved by Anthropic" or "reverse engineer or duplicate the Services". |
| Commercial Terms F | "these Terms do not grant either party any rights to the other's content or intellectual property, by implication or otherwise." |
| [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) 10 | "Other than the rights of access and use expressly granted in our Terms, our Terms do not grant you any right, title, or interest in or to our Services." |
| [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), "Can customers offer Claude Code in their products?" | "Unless we've mutually agreed otherwise, preinstalling or running Claude Code in your products or services (e.g. in hosted sandboxes or other agent infrastructure) requires agreeing to our Commercial Terms of Service and complying with the conditions below". |

The last entry is the only text that allows a third party to supply Claude Code
at all. It is conditional: the publisher must have agreed to the Commercial
Terms, the binary must be unmodified, no sign-in method may be removed, each end
user signs in with their own account, and the name may only be used in plain
text ("has Claude Code preinstalled"). It speaks of preinstalling in products
and hosted infrastructure, not of redistributing copies in a downloadable
archive, and no open licence grants a right to copy. That is not a clear
permission, so the release does not contain the binary. If the publisher of
MiniClaw has a Commercial Terms agreement and wants to rely on that section,
bundling becomes a separate decision (about 240 MB per archive, and the bundled
version ages because native installs update themselves).

The installer path meets the conditions of that section anyway: the binary is
installed and run exactly as Anthropic publishes it, and MiniClaw never handles
the sign-in.

## Official install methods

From [Advanced setup](https://code.claude.com/docs/en/setup):

| Method | Command | Result |
| --- | --- | --- |
| Native installer, Windows | `irm https://claude.ai/install.ps1 \| iex` | `%USERPROFILE%\.local\bin\claude.exe`, versions under `%USERPROFILE%\.local\share\claude`; updates itself |
| Native installer, macOS | `curl -fsSL https://claude.ai/install.sh \| bash` | `~/.local/bin/claude`, a symlink into `~/.local/share/claude/versions/`; updates itself |
| WinGet | `winget install Anthropic.ClaudeCode` | portable package, `claude.exe` link in `%LOCALAPPDATA%\Microsoft\WinGet\Links`; no self-update |
| Homebrew | `brew install --cask claude-code` | `/opt/homebrew/bin/claude`; no self-update |
| npm | `npm install -g @anthropic-ai/claude-code` (Node 22+) | native binary from `@anthropic-ai/claude-code-win32-x64` or `-win32-arm64`, copied over `bin/claude.exe` |

- Requirements: "Windows 10 1809+ or Windows Server 2019+", "x64 or ARM64 processor".
  "You do not need to run as Administrator."
- ARM64 Windows has a native build. `install.ps1` says "Use native ARM64 binary on
  ARM64 Windows, x64 otherwise"; the release manifest lists `win32-arm64` and
  `win32-x64`; the winget manifest lists both architectures with the same URLs.
- Both scripts redirect to `https://downloads.claude.ai/claude-code-releases/bootstrap.{ps1,sh}`
  and do the same four things: read `latest`, read `<version>/manifest.json`,
  download `<version>/<platform>/claude[.exe]` and compare its SHA-256 with the
  manifest, then run `<download> install [target]` and delete the download.
- Signatures: "macOS: signed by "Anthropic PBC" and notarized by Apple",
  "Windows: signed by "Anthropic, PBC". Verify with `Get-AuthenticodeSignature`".
  The Homebrew build on the development Mac shows
  `Authority=Developer ID Application: Anthropic PBC (Q6L2SF6YDW)`.
- The manifest also has a detached GPG signature (`manifest.json.sig`, key
  `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`). Setup does not check it:
  neither Windows nor macOS ships `gpg`. The platform code signature is checked instead.

## Git Bash

No prerequisite has to be installed. "Installing Git for Windows is optional. It
provides Git Bash"; "Without Git for Windows, Claude Code runs shell commands
via the PowerShell tool." `CLAUDE_CODE_GIT_BASH_PATH` is documented only as the
path to Git for Windows' `bash.exe` (`C:\Program Files\Git\bin\bash.exe`).
MiniClaw's MinGit has `usr\bin\sh.exe` but no `bash.exe`, and BusyBox is not a
documented value, so setup does not set the variable.

## What setup does

1. `detectProviders` looks for an existing Claude Code: `CLAUDE_BIN`, the native
   installer path, `~/.claude/local`, and on Windows the WinGet link and the npm
   per-architecture package, then `PATH`. Only `.exe` files count on Windows. If
   one is found, nothing below runs and setup behaves as before.
2. One notice (what, from where, about 250 MB) and no question. The installation
   starts at once; the destination question is asked meanwhile. Before the
   provider questions the wizard waits, showing the stage and download percentage
   if it is still running. `--preview` never downloads; it lists Claude Code as
   "not found, setup installs it automatically from Anthropic".
3. Refuses a terminal started with "Run as administrator" (high integrity while
   User Account Control is on), SYSTEM, or root on macOS; the installation is per
   user and never asks for elevation. With User Account Control off an
   administrator has only a high-integrity token, so that case is allowed.
4. Fetches `latest`, the manifest and the build from `downloads.claude.ai` over
   HTTPS with redirects refused. The download is bounded by the manifest size and
   its SHA-256 must match.
5. Checks the code signature before running anything: Authenticode `Valid` with
   `O="Anthropic, PBC"` on Windows; `codesign --verify --strict` against the
   Developer ID requirement for team `Q6L2SF6YDW` on macOS.
6. Runs `<download> install latest` as the user, without a shell, then deletes
   the download. No remote script is executed.
7. Checks the signature of the installed launcher and its `--version` output,
   detects providers again and continues. The provider shows as "found; sign in
   from MiniClaw later".
8. On any failure: one warning with the reason and the official one-line
   command, then setup continues with Codex if it is installed. With no provider
   at all the existing "Install Codex or Claude Code first" stop remains.
9. Cancelling setup (Ctrl+C or a cancelled question) aborts the download, stops
   the installer and removes the partial file.

Targets: `win32-arm64`, `win32-x64`, `darwin-arm64`.

## Native check

2026-10-04, Windows 11 ARM64 guest VM2, user S, User Account Control off (so the
token is high integrity; the medium-integrity and UAC-on cases have not run).
Claude Code was removed first with the documented uninstall.

- Background path through `startClaude`: 2.1.289 installed in 35 s to
  `C:\Users\S\.local\bin\claude.exe` (PE machine `0xaa64`, Authenticode valid,
  `O="Anthropic, PBC"`), detected as signed out, no question asked.
- The real interactive wizard, driven through its console: notice, destination
  question answered while the download ran, "installed" line, then the port
  question; Ctrl+C there ended it with exit code 130 and no leftover download.
- With Claude Code present the wizard shows no notice and installs nothing.

The x64 and macOS install paths have run only against fakes.
