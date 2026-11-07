# Bundled Windows Git runtime

Windows workspace commands use MiniClaw's own build of Git for Windows v2.56.0.windows.1 with three
sandbox patches (`git-lpac-path.patch`, `git-lpac-leading-directories.patch`, and since lpac5
`git-lpac-console-handle.patch`; review: [git-patch-review.md](git-patch-review.md)). There is no public download for this build, so it travels
inside the portable release, like BusyBox and ConPTY. Setup never downloads Git on Windows: the
download allowlist in `scripts/setup/download.mjs` approves no GitHub host (the official MinGit
release path and `release-assets.githubusercontent.com` were removed, review item R1).

The official MinGit pin (`windowsGitPackage` in `scripts/setup/windows-git.mjs`) is kept only as a
test fixture for CI and `scripts/windows-native-acceptance.ps1`, which pass it explicitly.
`installWindowsGit` has no default pin: every caller names the archive's pin, so a forgotten argument
can never install the unpatched Git that fails inside the sandbox. The switch is confined to `scripts/setup/windows-git-bundle.mjs` and its callers
(`plan.mjs`, `install.mjs`, `view.mjs`, `update-runtime.mjs`, `scripts/backend-package.mjs`,
`scripts/package-release.mjs`).

## Pins

| Architecture | File | Bytes | SHA-256 | Prefix |
| --- | --- | --- | --- | --- |
| arm64 | `miniclaw-git-2.56.0-lpac5-arm64.zip` | 29526262 | `ca7c05a029d02682bb90b2b8eeb9adde157bfa3c5e389b53ed51e54201fc19a3` | `git/` |
| x64 | `miniclaw-git-2.56.0-lpac5-x64.zip` | 29632338 | `66f3c549d5a5d995c562efdf4c722a468006e683d0c15e0d3a36f1f744f30d1c` | `git/` |

lpac5 (2026-10-05; 75 and 69 entries: lpac4's file set plus the third patch) is Git recompiled with the third patch;
see "Git on the sandboxed terminal" below. lpac4 (2026-10-05, superseded) was the same Git build as lpac3 (74 and 68 entries, same file set) with OpenSSL rebuilt; see
"HTTPS inside the sandbox" below. lpac3 (superseded) was the v3 build packaged without any Microsoft Visual C++
runtime file (review G4, closed by not distributing them); lpac2 shipped them. `git.exe` and its DLLs load `vcruntime140.dll`,
`msvcp140.dll` (and on x64 `vcruntime140_1.dll`) from the system-wide Microsoft Visual C++ Redistributable,
version 14.51.36231 or later, which setup installs from Microsoft when it is missing
([vc-runtime.md](vc-runtime.md)).

Build record, licences and gaps: `work/windows-vm/git-build-provenance.md` (outside the repository).

## HTTPS inside the sandbox (lpac4)

With lpac3, `git ls-remote https://…` in a networked sandboxed run stopped with "Insufficient randomness". OpenSSL
picks its Windows seed source at compile time: `providers/implementations/rands/seeding/rand_win.c` (lines 24-27 in
3.6.5) uses `BCryptGenRandom` only when `_WIN32_WINNT >= 0x0600`, and `include/internal/e_os.h` (lines 86-100) sets
`0x0501` when the build defines nothing. The vcpkg port configures `VC-WIN64A` / `VC-WIN64-CLANGASM-ARM` with the
triplet's flags, and neither the stock triplets nor the vcpkg toolchain nor those OpenSSL targets define
`_WIN32_WINNT`. libcrypto therefore seeded through `CryptAcquireContextW` + `CryptGenRandom`, and
`CryptAcquireContext` fails in the LPAC token (0x80090017) while `BCryptGenRandom` succeeds.

lpac4 (and lpac5, which links the same OpenSSL, libssh2 and curl builds) builds OpenSSL with `/D_WIN32_WINNT=0x0A00`
through overlay triplets: the pinned vcpkg tree's
`triplets/<triplet>.cmake` byte for byte plus a per-port block for `openssl` only
([per-port customization](https://learn.microsoft.com/en-us/vcpkg/users/triplets#per-port-customization)). No
source or archive changes. Only OpenSSL and the two ports linked against it (libssh2, curl) were rebuilt; Git was
not recompiled, and the libssh2 and libcurl import and export tables are unchanged. The new libcrypto imports
`bcrypt.dll!BCryptGenRandom` and no longer imports `CryptGenRandom`. It still imports `CryptAcquireContextW`, used
only by OpenSSL's CAPI engine (`engines/e_capi.c`), which Git never loads. The packager refuses a libcrypto without
the `BCryptGenRandom` import, and NOTICE carries a dated note on the build-option change. The overlay files are in
the corresponding source (`vcpkg-overlay-triplets/`).

Certificate checks are unchanged: Git uses the bundled `etc/ssl/certs/ca-bundle.crt` (`GIT_SSL_CAINFO`), and a
host with an expired or self-signed certificate is refused even when it is an approved host
(`server/tests/windows_tools.rs`).

## Git on the sandboxed terminal (lpac5)

With lpac4, every Git command whose stdout was the conversation terminal (ConPTY) did its work and then exited with
0xC0000008 (STATUS_INVALID_HANDLE); the shell saw status 8, so `git init && git add . && git commit` stopped after
`git init` and agents got a wrong exit status. Sandboxed processes run with strict handle checks, which turn a call on
a closed handle into an exception (with `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` the process then ends at once).
Git for Windows enables VT processing at start-up (`compat/winansi.c`, `enable_virtual_processing`), keeps the
standard handle and restores the console mode at exit (`reset_std_console`, an `atexit` handler); `git.c`
(`run_builtin`) closes stdout before exit, which closes that same handle, so the restore used a closed handle.
Outside the sandbox the call fails silently. Upstream `main` has the same code (no fix to port).

`git-lpac-console-handle.patch` keeps a private, non-inheritable `DuplicateHandle` copy for the restore and closes it
afterwards. Output through pipes (MiniClaw's own Git commands, the model's shell tool) never reached that path; only
the terminal did. Native regression: `git_in_the_terminal_exits_with_its_own_status` in
`server/tests/windows_git_commands.rs` (ConPTY terminal: `git --version`, an `init && add && commit` chain, `git
status` and `git init` in a new folder each report status 0 through the shell; with lpac4 the same test fails with
`GIT_RC=8`).

## Repository discovery inside the sandbox

Outside a repository, Git looks for `.git` in every parent folder and, inside the sandbox, stopped with
"failed to stat '<parent>'" at the first parent it may not read. Every sandboxed run that has the bundled Git
(`Sandbox::new_shell`: the shell, the terminal and MiniClaw's own Git commands) now gets
`GIT_CEILING_DIRECTORIES` (`windows_git::ceiling_directories`): the parent of each granted folder that is not inside
another granted folder, after an empty first entry. Git documents the variable as the list of directories it must
not move up into while discovering a repository, and the empty entry as "do not resolve the following entries"
([git(1)](https://git-scm.com/docs/git), "Environment Variables"; `Documentation/git.adoc` lines 555-568 in the
pinned source); the sandbox could not resolve
them. Git for Windows separates entries with `;` and compares them as text with its working directory (forward
slashes, canonical case), so the drive letter is listed in both cases, and a parent whose path contains `;` is left
out. Discovery therefore never looks above a grant, and inside a grant it still walks up to the repository root,
also from a nested grant. This is item 3 of recommendation S1 in [git-patch-review.md](git-patch-review.md).

## Pager, editor and credentials inside the sandbox

The bundled Git has no pager program, so `git log`, `git diff` and `git show` typed in the sandboxed terminal
failed with "cannot spawn less" (exit 128). Every sandboxed run with the bundled Git now gets
`windows_git::POLICY`:

- `GIT_PAGER=cat`. Git chooses its pager from `GIT_PAGER`, then `core.pager`, then `PAGER`, then the built-in
  `less`, and starts none when the value is empty or `cat` ([git(1)](https://git-scm.com/docs/git), `GIT_PAGER`;
  [git-config(1)](https://git-scm.com/docs/git-config), `core.pager`), so no program named `cat` is needed.
  A command can still set its own `GIT_PAGER`; no inherited variable carries one (the sandbox environment
  allowlist, `windows_process::inherited_environment`, passes no pager, editor or askpass variable).
- `GIT_TERMINAL_PROMPT=0`. A credential request fails at once with "terminal prompts disabled"
  ([git(1)](https://git-scm.com/docs/git), `GIT_TERMINAL_PROMPT`) instead of trying the console; before, it
  failed with the less clear "could not read Username ...: No such file or directory".
- No editor variable. Git looks for `GIT_EDITOR`, `core.editor`, `VISUAL`, `EDITOR`, then `vi`
  ([git-var(1)](https://git-scm.com/docs/git-var)); none exists in the sandbox, so `git commit` without `-m`
  already stops at once with Git's own "Please supply the message using either -m or -F option" (exit 1).

Native test: `git_never_waits_for_a_pager_an_editor_or_a_password` (server/tests/windows_git_commands.rs).

## Building a Windows package or release

| Variable | Required for | Value |
| --- | --- | --- |
| `MINICLAW_GIT_ARCHIVE` | optional, `package-backend.mjs`, `package-release.mjs`, checkout installs | Local path of the ZIP for the backend's architecture (offline override) |
| `MINICLAW_GIT_SOURCE` | optional, `package-release.mjs` on Windows | The unpacked corresponding-source folder (`git-corresponding-source/`, with `SOURCE-MANIFEST.json`) |
| `GH_TOKEN` or `GITHUB_TOKEN` | needed while the repository is private | A GitHub token that can read `Sep-Fading/miniclaw-windows` (for example `gh auth token`) |
| `MINICLAW_SOURCE_OFFER_CONTACT` | optional, `package-release.mjs` on Windows | Who honours the written source offer; default `Sep` (see below) |

The bundle architecture comes from the backend executable's PE header. With `MINICLAW_GIT_ARCHIVE` set,
the packager reads the ZIP through a bounded offline reader. Without it, the packager fetches the same
ZIP from MiniClaw's v0.1.5 GitHub release (`scripts/setup/windows-git-bundle.mjs` pins the file, size,
SHA-256 and release asset id per architecture). Either way it checks the pinned size and SHA-256 before
decoding and stops when the bytes differ or belong to the other architecture. The bundle receives
`bin/git/<file>` and `bin/git/licenses/*` (GPLv2/LGPL texts, the three patches, NOTICE, licence texts and
`SOURCE-OFFER.txt`), all recorded in the bundle manifest.

The fetch (`downloadReleaseAsset` in `scripts/setup/download.mjs`) is narrower than setup's download list,
which still approves no GitHub host (setup never downloads Git, and the official MinGit URL stays out of it):

- Only the repository `Sep-Fading/miniclaw-windows`, and only this asset's own URL:
  `https://api.github.com/repos/Sep-Fading/miniclaw-windows/releases/assets/<id>` with
  `Accept: application/octet-stream` when `GH_TOKEN` or `GITHUB_TOKEN` is set (the only route for a private
  repository), otherwise `https://github.com/Sep-Fading/miniclaw-windows/releases/download/v0.1.5/<file>`
  anonymously ([GitHub REST API, release assets](https://docs.github.com/en/rest/releases/assets#get-a-release-asset)).
- Redirects are followed by hand and only to GitHub's asset storage over HTTPS
  (`release-assets.githubusercontent.com`, which GitHub uses today, or `objects.githubusercontent.com`,
  its earlier host; path `/github-production-release-asset...`). The token goes to `api.github.com` alone,
  never to the storage host.
- While the repository is private, an anonymous request gets HTTP 404 (checked 2026-10-07, also with a
  token on the `github.com/.../releases/download/...` URL, which is why the token uses the API route).
  The error names `GH_TOKEN`/`GITHUB_TOKEN` and the offline variable.

A Windows release build takes the corresponding-source folder from `MINICLAW_GIT_SOURCE`, or else downloads
`miniclaw-0.1.5-6457c45-windows-arm64-git-corresponding-source.tar.gz` (154469901 bytes, SHA-256
`b3e5fb5b...cfdfea0`; its manifest describes both architectures' ZIPs) from the same release through the same
rules, checks the pin and unpacks it into the build's temporary folder. It then verifies the folder against its
manifest (every file's size and SHA-256, no unlisted file, and the runtime entry equal to the bundled pin) and
writes it beside the release archive:

```
<release>/                                   portable folder (prebuilt/bundle/bin/git/...)
<release>.tar.gz                             portable archive
<release>-git-corresponding-source.tar.gz    complete corresponding source (about 160 MB)
```

`<release>/GIT-SOURCE-OFFER.txt` names the source archive, its SHA-256 and the distributor contact.
Publish the source archive at the same place as every copy of the release archive.

The distributor contact defaults to `Sep`, the distributor name the user supplied on 2026-10-04, so a
release build no longer stops when `MINICLAW_SOURCE_OFFER_CONTACT` is unset; a non-empty value of that
variable replaces it. Note: a GPLv2 section 3(b) written offer is normally expected to tell a third
party how to reach the distributor (a postal or e-mail address). Only a name was supplied, so the
default offer names "Sep" without an address. Set the variable to a name plus address before
distributing to third parties if the source archive is not published beside every copy of the
release (section 3(a)). This is not legal advice.

## Setup and update

Setup copies the bundle into `runtime/`, then installs Git from `runtime/bin/git/<file>`: the archive
is hash-checked again, decoded strictly, materialised under `data/windows-git-runtime/<UUID>/git/`, and
`data/windows-git.json` (`git/bin/git.exe`, `git/libexec/git-core`) is published only afterwards.

`miniclaw update` first refuses a new build whose architecture differs from the installation's
(installed `manifest.json` `arch`, or the installed backend's PE header for an older manifest), for a
release and for a checkout build alike (review item R2). It then replaces `bin/git/*` in the same native
recovery group as the backend, ConPTY and BusyBox. After the files are in place it installs a new runtime from the new archive, switches the
private record to it, and only then updates the network service. A failed Git install or a failed or
declined service update restores the earlier files, the earlier record and runtime, and removes the new
runtime. The earlier runtime folder is removed only after the whole group succeeded.

Crash safety (review item R3): the live `windows-git.json` exists at every moment of a switch. The new
record is written as `windows-git.json.next`, the earlier record is saved as `windows-git.json.previous`
(same protected writer), and a single rename replaces the live record. Every switch starts with
`recoverWindowsGit`: the live record wins, a leftover `.next` (never published) and `.previous` (a
finished switch) are removed, the earlier protocol's half-switched state (no live record, only
`.previous` and `.next`) is rolled back to `.previous`, and every generated
`windows-git-runtime/<UUID>` folder the live record does not name is removed (best effort, retried on
the next update). Recovery currently runs at the next `miniclaw update`; until then an interrupted
update leaves either the earlier or the new runtime live, and at worst unused folders on disk.
