# Independent review: private Git LPAC patches (2026-10-04)

Subject: Git for Windows v2.56.0.windows.1 + `git-lpac-path.patch` + `git-lpac-leading-directories.patch`,
ARM64 candidate `miniclaw-git-2.56.0-lpac1-arm64.zip`
(SHA256 `b7fbe019aa72dbdaeeccb68bb0345b5d910f216349dee541d0dfb1443c74d125`, digest re-computed on the host).

Method: static review only. Read the patched tree `work/windows-vm/git-lpac-source/`, diffed `path.c`
against the pinned original tarball (the only difference is the 12 added lines), read all callers, the
mingw wrappers, the three acceptance logs, `server/src/windows_git.rs`, `execute_git` in
`server/src/executor_windows.rs`, `git_arguments` in `server/src/workspace_commands.rs`, the acceptance
test, the ZIP listing and `git-corresponding-source/`. Nothing was run in a Windows guest; every
statement about Windows runtime behaviour beyond the retained logs is reasoning from source and
documentation and is marked with a confidence.

## Verdict

**The ARM64 candidate must NOT be pinned for production as built. Rebuild with the changes below.**

| Item | Verdict |
| --- | --- |
| `git-lpac-leading-directories.patch` | **Approve with required changes** (L1, L2). No sandbox escape or new link-following found; the condition is looser than the provenance claims and has one side-effecting path. |
| `git-lpac-path.patch` | **Approve** (concur with the earlier approval). Residual notes P1-P3, none blocking. |
| `-c safe.directory=<workspace>` in `execute_git` | **Not unconditionally.** Keep Git's refusal by default with a clear message; an exact-path exception is acceptable only after a server-side owner check (S1). |
| Package | **Gaps to close before distribution** (G1-G6). |

## 1. `git-lpac-leading-directories.patch`

Patched code: `git-lpac-source/path.c:862-890` (added lines 866-877) in
`safe_create_leading_directories_1`.

### What the condition really is

The provenance says the patch "accepts exactly this pair: stat denied and a repeated mkdir reporting
that the name exists". The code does not track which call failed; it reads `errno` once at
`path.c:867`. Two different histories reach the second `mkdir`:

- A (intended): first `mkdir` -> `EEXIST`, `stat` at line 864 fails -> `EACCES`, second `mkdir` -> `EEXIST` -> accept.
- B (unintended): first `mkdir` itself -> `EACCES` (line 864 `stat` is short-circuited and never runs),
  second `mkdir` is attempted anyway.

errno sources are reliable for history A: `mingw_mkdir` (`compat/mingw.c:747-768`) returns the UCRT
`_wmkdir` result untouched on failure, and `mingw_stat` (`compat/mingw.c:1446-1477`) assigns
`errno = err_win_to_posix(err)` from a saved `GetLastError()` after its inner `mingw_lstat` fallback, so
the lstat errno cannot leak. `EEXIST` from `_wmkdir` only comes from `ERROR_ALREADY_EXISTS` /
`ERROR_FILE_EXISTS`; reserved device names never reach it (`is_valid_win32_path` -> `EINVAL`, line 752).

### Findings

**L1 (required change) - second `mkdir` runs when the first `mkdir` was denied; it can create a directory
and still return failure.** `path.c:867`.
Scenario: history B where the name is in delete-pending state (another Git process, an indexer or AV
still holds a handle on a just-removed empty ref/object directory). Windows reports access denied for
the create; the patched code immediately retries; if the delete completes in between, the second
`mkdir` succeeds. The `&&` chain is then false, control falls to `else if (errno == ENOENT)` with a stale
errno and returns `SCLD_FAILED`, having created the directory and skipped `adjust_shared_perm`
(line 891). Unpatched Git returns `SCLD_FAILED` without side effects. If the second `mkdir` instead
reports `EEXIST` (a racing creator), the name is accepted with no type check at all. The patch comment
"Nothing was created or changed" is therefore false on this path. Impact: spurious failure plus a
stray directory, and an acceptance outside the documented pair; no boundary crossing.
Confidence: high that the code path exists (pure control flow); medium that delete-pending produces
exactly denied-then-success on this Windows build (not run).
Required change: remember the first mkdir's errno and the stat result; never call `mkdir` twice:

```c
} else if (mkdir(path, 0777)) {
	int mkdir_errno = errno, stat_failed = 0;
	if (mkdir_errno == EEXIST &&
	    !(stat_failed = stat(path, &st)) && S_ISDIR(st.st_mode))
		; /* somebody created it since we checked */
#ifdef GIT_WINDOWS_NATIVE
	else if (mkdir_errno == EEXIST && stat_failed && errno == EACCES)
		; /* exists, attributes unreadable (sandboxed ancestor) */
#endif
	else if (errno == ENOENT)
	...
```

This also removes the dependence on `errno` surviving a *successful* `stat` (today a non-directory is
rejected only because nothing in the success path of `mingw_stat` happens to set `errno` to `EACCES`).

**L2 (required change) - no regression test for the patch; documentation overstates it.**
`git-lpac-tests/` only covers the path patch. The acceptance test proves the positive case only.
Required: (a) a native test in the sandbox with an attribute-denied *file* as the last leading component
and as a middle component, asserting Git fails cleanly and creates nothing; (b) correct the comment and
`git-build-provenance.md` wording (see L3: the name is not proven to be a directory).

**L3 (accepted risk, document it) - "denied + exists" is true for non-directories.** `path.c:866-877`.
Microsoft documents `_wmkdir` `EEXIST` as: "dirname is the name of an existing file, directory, or
device" (https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/mkdir-wmkdir).
`CreateDirectoryW` documents only `ERROR_ALREADY_EXISTS` "The specified directory already exists" and
`ERROR_PATH_NOT_FOUND`, and says nothing about ordering against the access check
(https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createdirectoryw); the
existence-before-access ordering the patch relies on is observed behaviour
(`git-accept-arm64-red-ancestors.log`: error 5 / error 183 at six ancestor depths), not a documented
guarantee. So a file, a junction/symlink, or a directory with a deny ACE, inside or outside the grant,
is accepted when its attributes are unreadable. The acceptance is not limited to ancestors above the
grant.
Downstream effect, traced through the callers:
- Non-directory in the middle: the next component's `stat` and `mkdir` fail with `ENOENT`, giving
  `SCLD_VANISHED` instead of `SCLD_FAILED`/`SCLD_EXISTS`. Retrying callers are bounded
  (`refs/files-backend.c:1186,1228` three attempts; `refs/files-backend.c:837` `attempts_remaining`).
  No infinite loop; the error text changes ("vanished"/ENOENT instead of "exists").
- Non-directory as the last leading component: returns `SCLD_OK`; the caller's own open/lock then
  fails with ENOENT/ENOTDIR. Wrong diagnosis, nothing written.
- Reparse point: no new link following. The function has always used `stat` (follows links,
  `compat/mingw.c:1455` opens without `FILE_FLAG_OPEN_REPARSE_POINT`), so a junction or symlink to a
  readable directory was already accepted before the patch; the patch extends that to an unreadable
  target. All later opens are still resolved and access-checked by the kernel against the LPAC token.
- Checkout safety is untouched: `verify_path`, `has_symlink_leading_path` and `entry.c`'s
  `create_directories` do not call this function. Worktree-side callers are `apply.c:4608`,
  `builtin/mv.c:659`, `builtin/difftool.c:277`, `builtin/clone.c:1110`, `builtin/worktree.c`; the rest
  are gitdir paths.
- Masking a real permission problem inside the worktree: a directory the sandbox cannot read is now
  reported one step later (at the next component or the final open) with that call's error, instead of
  "unable to create directory". Confusing, not unsafe.
Because Git runs inside the same LPAC sandbox as the agent shell (`executor_windows.rs:544`,
`Sandbox::new_shell` with the same grants), Git cannot reach anything the agent could not already reach;
I found no way for the looser check to write outside the granted tree.
Confidence: high for the control flow; medium for exact Win32 error codes in the file/junction cases
(covered by the L2 test).

**L4 (note) - not inert for unsandboxed Git.** `#ifdef GIT_WINDOWS_NATIVE` (`compat/posix.h:98`) is
correct: Windows-only (MSVC and MinGW), absent on Cygwin/POSIX. On Windows it changes behaviour for any
token, e.g. an ordinary user working beneath an ancestor they can traverse but not list. That matches
POSIX semantics and is acceptable, but this `git.exe` must stay private to MiniClaw and never be put on
a user PATH. `windows_git.rs:14-27` pins it under the private data root, which is sufficient.

## 2. `git-lpac-path.patch` (fresh look)

Files: `compat/miniclaw-lpac-path.h`, call sites `compat/mingw.c:1716-1721` (realpath) and
`compat/mingw.c:1758-1760` (getcwd). **I concur with the approval.**

- Identity: compares `FILE_ID_INFO.VolumeSerialNumber` (64-bit) and the full 128-bit `FileId`; both
  handles are opened the same way (links followed). Complete for one machine.
- Fail-closed cases verified by reading: UNC and `\\?\UNC\` input (drive-letter test fails), mapped
  network drive, `subst` drive and junction/mount point to another volume (candidate is the wrong path,
  so the reopen fails or the identity differs -> `ERROR_INVALID_DATA`), file systems without
  `FileIdInfo`. Buffer arithmetic (`length + 7 > capacity`, result `length + 6`) is correct. The
  fallback is entered only on `ERROR_ACCESS_DENIED`, so normal Git is unaffected.
- TOCTOU: the result is a path string, exactly as with the original `GetFinalPathNameByHandleW`; a
  rename after the check is the same exposure unpatched Git has. No handle is kept, nothing new.

Residual notes, none blocking:
- **P1** (low, confidence medium): two simultaneously mounted block-level clones of one volume share
  serial number and file IDs. If the input reaches volume Y through a junction on drive X and X is a
  clone of Y, the candidate `X:\suffix` passes the identity test and the wrong drive is returned. Needs
  an unusual disk setup; result stays inside the sandbox ACLs.
- **P2** (functional): a workspace reached through `subst` or a cross-volume junction makes
  getcwd/realpath fail in the sandbox. Safe, but should surface as a clear product message.
- **P3** (cosmetic): `mingw_strbuf_realpath` allocates `fallback_input` on every call
  (`mingw.c:1716`); `miniclaw_same_file` leaves the caller to infer the error when the first
  `GetFileInformationByHandleEx` fails. No change required.

## 3. Ownership check and `-c safe.directory=<exact workspace>`

What the check protects: Git's documentation says that by default Git "will refuse to even parse a Git
config of a repository owned by someone else, let alone run its hooks", and that `safe.directory` is
honoured only from protected configuration (system, global, command line)
(`Documentation/config/safe.adoc` in the source tree; https://git-scm.com/docs/git-config#Documentation/git-config.txt-safedirectory).
It was introduced for CVE-2022-24765: on a multi-user machine another user plants a `.git` (for example
`C:\.git`) so that the victim's Git reads attacker-controlled config and runs attacker-chosen commands
(https://github.blog/open-source/git/git-security-vulnerability-announced/, GHSA-vw2c-22j4-2fh2).
On Windows the test is `is_path_owned_by_current_sid` (`compat/mingw.c:3966-4033`): owner equals the
token user, or owner is BUILTIN\Administrators and the token (or its linked token) is an enabled
member. `ensure_valid_ownership` (`setup.c:1470-1500`) checks gitfile, worktree and gitdir, but the
`safe.directory` exemption is keyed on the worktree path alone.

What MiniClaw's launch actually neutralises (verified in code):
- `workspace_commands.rs:54-61`: `--no-pager`, `core.fsmonitor=false`, `core.hooksPath=<empty file>`
  (hooks cannot resolve), `diff.external=`, `core.pager=cat`, `core.untrackedCache=false`.
- `windows_git.rs:42-43`: `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=<empty file>`;
  `executor_windows.rs:550-552`: `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `GIT_LITERAL_PATHSPECS=1`.

What is NOT neutralised: the repository's own `.git/config` is fully honoured. Command-running keys that
remain live include `filter.<name>.clean/smudge/process` (run by `add`, `status`, `diff`),
`diff.<driver>.textconv` and `diff.<driver>.command`, `merge.<driver>.driver`, `core.sshCommand`,
`credential.helper`, `core.editor`/`sequence.editor`, `gpg.program`, `core.alternateRefsCommand`,
`include.path`, and `core.worktree`. That is acceptable when the config author is the agent or the user
(same sandbox, same grants). It is not acceptable when the author is a different principal, and the
ownership check is the only thing that distinguishes the two.

Concrete failure with an unconditional `-c safe.directory=<workspace>`: user A opens a shared folder
owned by standard user B (or one containing a `.git` gitfile pointing at B's gitdir; the exemption
covers it because only the worktree path is matched). B's `.git/config` sets a `filter` or `textconv`
command. MiniClaw's own status/diff view runs it as A inside A's sandbox, which has every other folder
in `grants` and A's proxied network access. That is CVE-2022-24765 reproduced inside the sandbox; the
sandbox limits the blast radius but does not remove it.

Recommendation (S1):
1. Default: keep Git's refusal. Detect "dubious ownership" and show a product message naming the folder
   and its owner, instead of raw stderr.
2. An exact-path exception is acceptable only when the server, outside the sandbox and by handle
   without following reparse points, has verified that the workspace root, `.git` and (if `.git` is a
   file) the resolved gitdir are each owned by the current user SID, BUILTIN\Administrators or SYSTEM.
   Administrators-owned is the common benign case here (folders created from elevated processes or on
   machines with UAC off, as on VM2); an administrator can already act as the user, so trusting that
   owner adds no principal. Any other owner: refuse, or require an explicit per-folder user decision.
3. Never `*` or `/*`; pass the canonical path only; also set `GIT_CEILING_DIRECTORIES` to the workspace
   parent so discovery cannot walk up to a foreign repository.
4. Add an acceptance case for an Administrators-owned workspace and one for a foreign-owned workspace.
   The fixture change (`own_as_user`, `windows_git_commands.rs:79-98`) is legitimate for the common
   case but currently leaves the refusal path untested.
Confidence: high on the config surface (read from code); medium on how often Administrators-owned
workspaces occur in the field.

## 4. Packaging obligations (gaps only; not legal advice)

Present and adequate: GPLv2 text (`git-COPYING.txt`), LGPL-2.1 text, both patches (ZIP copy is
byte-identical to the reviewed patch), NOTICE naming version, modification and hashes, per-dependency
copyright files, and in `git-corresponding-source/`: original tarball, patched source, both patches,
build and packaging scripts, the vcpkg tree, and source archives for every linked library. `git.exe`
and the helpers do not import OpenSSL directly (only through `libcurl.dll`).

Gaps:
- **G1** GPLv2 section 3: the ZIP itself carries no source and no written offer. NOTICE says source "is
  distributed beside this archive"; that only satisfies 3(a) if the release process really publishes
  `git-corresponding-source/` at the same place as the binary for every distribution channel
  (installer download included). Not yet implemented or tested. Otherwise add a 3(b) written offer valid
  three years. The same applies to the LGPL libraries (libiconv, libidn2, libunistring).
- **G2** `git/licenses/pcre2-copyright.txt` is a 97-byte stub that points to a `LICENCE` file that is
  not shipped. The PCRE2 BSD licence text, and the sljit licence (JIT is enabled and sljit source is in
  the dependency set), must be included.
- **G3** GPLv2 section 2(a): the modified files should carry a prominent change notice with a date.
  `miniclaw-lpac-path.h` and the `path.c` comment say "MiniClaw" but give no date; NOTICE gives no date.
- **G4** `vcruntime140.dll` and `msvcp140.dll` ship next to a GPLv2 executable. NOTICE mentions the
  Visual Studio redistribution terms but no Microsoft licence text is included, and GPLv2 section 3's
  system-library exception does not apply to a component that "itself accompanies the executable".
  Needs a legal decision (official Git for Windows avoids this by not shipping the MSVC runtime).
- **G5** No licence notice for the CA bundle (Mozilla data, MPL-2.0) or for the Public Suffix List
  compiled into `psl-5.dll` (MPL-2.0; `libpsl-copyright.txt` is MIT only).
- **G6** `git-corresponding-source/git-build-provenance.md` and `git-build-inputs.json` are stale copies
  (6878 vs 7663 bytes; missing the Results section), the first paragraph of the provenance still says
  the source ZIP contains only the path patch, and NOTICE is labelled "candidate". Regenerate at release.
  There is no x64 package or x64 dependency notice set yet.

Not a licence matter but worth doing in the rebuild: the ZIP ships `git-daemon.exe`,
`git-http-backend.exe`, `git-shell.exe`, `git-imap-send.exe`, `git-p4`, Perl modules and shell scripts
that cannot run (no shell/Perl shipped) or are not needed by the product; removing them reduces surface.

## Not done

No guest execution, no build, no dynamic test of L1 or L3. Dependency source archives were listed, not
hash-verified against vcpkg. x64 was not reviewed (no package exists).

---

# v3 re-review (2026-10-04, later the same day)

Subject: rebuilt runtime `miniclaw-git-2.56.0-lpac2-<arch>.zip`; digests re-computed on the host:
arm64 `f255f30cb7dfa29d6b748dcac34c514395f5bdf01f84cdc83260d61db587ae17`,
x64 `6693d5e04aced9683052bec2ec7c08c689db91acd6334d06257ed4ad979f52d5`,
source v3 `66e379c979dae62fa9e6a044bb4ca1ec6cbffece7f5b102474ce0ffefd57bac4`. All three match the pins.

Method: static review plus host-side JS tests. Not re-run in a guest; native results are read from
`git-accept-arm64-v3.log` and `gitpkg-setup-accept-arm64.log`. I cannot prove from the binary that
`git.exe` was compiled from the v3 source; that rests on the build log and the pinned source digest.

## Verdict

**Yes: the ARM64 v3 pin may stay in production setup. No required code changes.** One open item is not
mine to close: G4 (MSVC runtime DLLs beside a GPLv2 executable) still needs the owner's legal decision
before the release is distributed to third parties. The x64 pin is integrity-correct but has no passing
sandboxed acceptance and should not be treated as production-validated.

## Checks

**L1 (closed).** `git-lpac-source/path.c:862-882` matches the required logic exactly: `mkdir_errno` and
`stat_failed` are captured at line 863, `mkdir` is called once, the Windows-only branch (lines 867-882)
accepts only `mkdir_errno == EEXIST && stat_failed && errno == EACCES`. The upstream "created since we
checked" branch is semantically unchanged. Diff against the original tarball is 21 changed lines, all in
this block. The comment is now true (dated, "not proven to be a directory", "mkdir() is not retried and
nothing is created"), and so are the provenance and NOTICE wording.

**Path patch (unchanged in substance).** v3 `git-lpac-path.patch` (SHA256 `8154f50e...`) differs from the
approved `d93aaa2f...` only by one dated comment line in `compat/mingw.c` and a three-line dated header in
`compat/miniclaw-lpac-path.h` (hunk offsets shift accordingly). The v2 and v3 source ZIPs differ in
exactly `path.c`, `compat/mingw.c`, `compat/miniclaw-lpac-path.h` and the two patch files; the ZIP's
copies of those three sources are byte-identical to `git-lpac-source/`. The patches shipped in
`git/licenses/` are byte-identical to the reviewed ones. Approval stands.

**L2 tests (closed).** `server/tests/windows_git_commands.rs`:
- `git_leading_directories_reject_denied_file` (lines 141-170): `git init` beneath an out-of-grant file,
  as last and as middle leading component. Asserts non-zero exit, "cannot mkdir", the parent listing
  unchanged, the blocker still a file with its sentinel content, and an empty workspace. These are real
  negative assertions. Limitation (accepted): the test cannot tell whether the accepting branch or an
  earlier failure produced the result, since both end in ENOENT; it proves the outcome, not the path.
- `git_refuses_administrators_and_foreign_owned_repositories` (lines 174-210): a user-owned control
  passes, then BUILTIN\Administrators and LOCAL SERVICE owners each give exit 128, empty stdout,
  "detected dubious ownership" and the owner SID. No `safe.directory` is passed anywhere in
  `execute_git`; this matches recommendation S1 option 1.
Native evidence: 5/5 in `git-accept-arm64-v3.log`, 10/10 in `gitpkg-setup-accept-arm64.log`.

**Package (ARM64 ZIP listed, 78 entries).**
- G1 closed: `SOURCE-OFFER.txt` in the ZIP; `scripts/package-release.mjs:119-165,360-366` verifies the
  source folder against `SOURCE-MANIFEST.json` and the runtime pin (unlisted, changed or missing file
  stops the release), writes `<release>-git-corresponding-source.tar.gz` and `GIT-SOURCE-OFFER.txt`, and
  refuses to start without `MINICLAW_SOURCE_OFFER_CONTACT`.
- G2 closed: `pcre2-LICENCE.md` and `sljit-LICENSE.txt` present.
- G3 closed: dated notices in both modified files and NOTICE.
- G4 OPEN: `vcruntime140.dll` / `msvcp140.dll` still shipped; `microsoft-visual-studio-Redist.txt` is a
  190-byte pointer, not licence text. Correctly recorded as open in the provenance.
- G5 closed: `MPL-2.0.txt` plus NOTICE lines for the CA bundle and the Public Suffix List.
- G6 closed: the provenance copy in `git-corresponding-source/` is byte-identical to the current file;
  NOTICE no longer says "candidate".
- Surface reduction done: no `git-daemon`, `git-http-backend`, `git-shell`, `git-imap-send`,
  `git-sh-i18n--envsubst`, scripts, mergetools or `share/perl5`. Remaining helpers are `git-http-fetch`,
  `git-http-push` and the four remote helpers.

**Setup path.**
- Pin before decode: `gitBundleFiles` (`scripts/setup/windows-git-bundle.mjs:36-58`) and
  `verifiedEntries` (`scripts/setup/windows-git.mjs:69-75`) both check length and SHA-256 on the in-memory
  bytes before `gitArchiveEntries` runs, and extraction uses those same bytes.
- No download: the bundle pin has no `url`; `gitBundleFiles` throws when neither bytes nor
  `MINICLAW_GIT_ARCHIVE` is given; `plan.mjs:105-106` puts no Git package in the download list;
  `install.mjs:145-150` and `replaceWindowsGit` always pass the bundle pin and the bundled archive path.
  The native log shows "fetch attempts: 0" for install and update.
- Architecture: `backend-package.mjs:53-55,72-75` derives the architecture from the backend PE header and
  requires that architecture's pinned archive; the other architecture's ZIP fails its pin (unit test at
  `tests/setup-windows-git.test.mjs:116-117`); `prebuilt.mjs:26` refuses a release built for another
  architecture.
- Rollback: `replaceWindowsGit` installs beside the live runtime, switches the record by rename, and
  `update-runtime.mjs:112-127` rolls the record and runtime back if the service update fails, committing
  only after the group succeeds. Native log: "after failed update: record unchanged=true".
- Host tests run by me: `node --test` on `setup-windows-git`, `setup-update`, `backend-package`,
  `setup-install`, `setup-release`: 51 tests, 50 pass, 0 fail, 1 skipped.

## Recommended (not blocking)

- **R1** `installWindowsGit` still defaults `pin` to `windowsGitPackage` (official MinGit, with a download
  URL), and `scripts/setup/download.mjs:74` still allow-lists the MinGit URL. No current caller uses the
  default, but a future caller that omits `pin` would silently install the unpatched Git that fails in the
  sandbox. Make `pin` a required argument and drop the allow-list entry and the "downloads" wording in
  `view.mjs:73`.
- **R2** `refreshRuntime` trusts the new bundle's `manifest.json` for the architecture and does not compare
  it with the installed manifest. The release path is covered by `prebuilt.mjs`; a one-line comparison
  would cover checkout builds too.
- **R3** A crash between the two renames in `replaceWindowsGit` (lines 90-91) leaves no live record until
  the next update, which then orphans the old runtime folder. Fails closed (Git unavailable), no action
  needed beyond noting it.
- **x64** `git-accept-x64emu-v3-red-status-no-memory.log`: the x64 `git.exe` exits 0xC0000017 inside the
  sandbox under ARM64 emulation. Needs acceptance on x64 hardware before x64 is called production-ready.
- Still not performed: fresh-account / standard-user run.

## Follow-up to R1-R3 and the source offer (2026-10-04, x64 audit)

- **R1 done.** `installWindowsGit` has no default pin and refuses a call without one. `scripts/setup/download.mjs`
  no longer approves the MinGit release path or `release-assets.githubusercontent.com`; nothing else used them.
  The official MinGit pin remains a test fixture passed explicitly by CI, `tests/support/harness.mjs` and
  `scripts/windows-native-acceptance.ps1`. The setup review never says that setup downloads Git.
- **R2 done.** `refreshRuntime` refuses a new build whose architecture differs from the installation's
  (installed manifest `arch`, or the installed backend's PE header) before changing any file, for release and
  checkout builds alike.
- **R3 done.** The live record now exists at every moment of a switch: the earlier record is saved as `.previous`
  through the protected writer, and one rename replaces the live record. `recoverWindowsGit` runs first on every
  switch. It drops leftover `.next` and `.previous` files, rolls back the earlier protocol's half-switched state,
  and removes generated runtime folders the live record does not name. Tests: `tests/setup-windows-git.test.mjs`.
- **Source offer.** The statement above that the release "refuses to start without `MINICLAW_SOURCE_OFFER_CONTACT`"
  no longer holds. The contact defaults to the distributor name "Sep" the user supplied; the variable overrides it.
  Only a name was supplied, with no address; see [git-runtime.md](git-runtime.md).
- **x64.** See [x64-readiness.md](x64-readiness.md) for the analysis of the STATUS_NO_MEMORY failure under emulation.

---

# G4 status update (2026-10-04, vcredist agent; not a re-review)

The user decided to close G4 by not distributing Microsoft's files. `miniclaw-git-2.56.0-lpac3-<arch>.zip`
repackages the same v3 build trees without `vcruntime140*.dll`, `msvcp140.dll` and the Redist.txt pointer
(arm64 `2bf25eb8...`, 74 entries; x64 `393cbae5...`, 68 entries); the packager now refuses any Microsoft runtime
file. Setup installs Microsoft's own Visual C++ Redistributable from Microsoft (docs/windows/vc-runtime.md).
Native evidence on VM1 with the system runtime: clean-PATH checks for both architectures, an LPAC load probe
(ARM64 `git.exe` and `git-remote-https.exe` run in LPAC and load the runtime from System32), and the ARM64
sandboxed acceptance 5/5 (`work/windows-vm/vcredist-*.log`).

---

# lpac4 update (2026-10-05, git-https agent; not a re-review)

No Git source or patch changed. `miniclaw-git-2.56.0-lpac4-<arch>.zip` (arm64 `4ed169a9...`, 74 entries; x64
`f3f15257...`, 68 entries) carries OpenSSL, libssh2 and curl rebuilt in the pinned vcpkg tree with overlay triplets
that compile only OpenSSL with `_WIN32_WINNT=0x0A00`, so libcrypto seeds from `BCryptGenRandom` instead of the
legacy CryptoAPI that fails in the sandbox (details and source citations: [git-runtime.md](git-runtime.md)). NOTICE
gains a dated note on that build-option change; the overlay files are in the corresponding source. S1 item 3 is now
implemented: sandboxed runs get `GIT_CEILING_DIRECTORIES` set to the parent of each outermost grant. Native evidence
on VM1: sandboxed Git acceptance 6/6 (the five earlier tests plus `git_discovery_stops_at_the_grant`) and bundled
Git HTTPS through the approved proxy (`work/windows-vm/git-https-native-vm1-green-2.log`).

---

# Third patch: `git-lpac-console-handle.patch` (2026-10-05, git-console agent; self-review, independent review pending)

**Defect.** In the sandbox, processes run with strict handle checks: a call on a closed handle raises
STATUS_INVALID_HANDLE (0xC0000008) instead of failing with ERROR_INVALID_HANDLE
([PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_strict_handle_check_policy)).
`compat/winansi.c` `enable_virtual_processing()` (called from `winansi_init()`, `compat/mingw.c:4478`) stored
`GetStdHandle(STD_OUTPUT_HANDLE)` (or the stderr handle) in `std_console_handle` and registered
`atexit(reset_std_console)`. `git.c:524-529` (`run_builtin`) closes stdout when it is not a pipe or socket, which
closes that same OS handle; a pager's `dup2()` over fd 1 does too. `reset_std_console()` then called
`SetConsoleMode()` on the closed handle at exit. Outside the sandbox that failed silently; inside, Git ended with
0xC0000008 after its work (shell status 8) whenever stdout was the ConPTY terminal. Upstream Git for Windows `main`
(2026-10-05) has the same code, introduced by ae48d95 / PR #4700 (2023); no upstream fix or issue was found.

**Change** (one function pair, dated notice in the file): `enable_virtual_processing()` selects the handle exactly as
before into a local `std_handle`, then stores `DuplicateHandle(GetCurrentProcess(), std_handle, GetCurrentProcess(),
&std_console_handle, 0, FALSE, DUPLICATE_SAME_ACCESS)` (NULL on failure); the start-up `SetConsoleMode()` uses the
original handle, which is valid then. `reset_std_console()` restores the mode through the duplicate only when there is
one, closes it and clears the variable; the code page restore is unchanged. A duplicate refers to the same console
object, so the restore has the effect upstream intended
([DuplicateHandle](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle) lists
console input buffers and screen buffers among the duplicable objects).

**What could go wrong, and why it does not:**
- *Handle leak.* One duplicate per process: `enable_virtual_processing()` runs once (from `winansi_init()` at start-up)
  and only after `GetConsoleMode()` succeeded. It is closed by the `atexit` handler; a process that ends without running
  `atexit` handlers (`_exit`, a crash) has its handles closed by the system. Nothing else stores it.
- *Double close.* Only `reset_std_console()` closes it, then sets it to NULL; the handler is registered once. The
  patch never closes the original standard handle (CRT fd 1/2 keep owning it).
- *Inheritance.* `bInheritHandle` is FALSE, so the duplicate is never inherited, even by children created with
  `bInheritHandles` TRUE; it is never placed in a `STARTUPINFO`. Children keep receiving the standard handles as before.
- *stdout not a console.* Unchanged selection: stdout first, then stderr; if neither is a console the function returns 0
  before duplicating and registers nothing (pipes, files: MiniClaw's own Git commands and the model's shell tool).
  If only stderr is a console, the duplicate is of stderr, which Git does not close: harmless.
- *Duplicate fails.* The mode is then not restored at exit (VT processing stays enabled in that console); the earlier
  code would have attempted the restore through a possibly closed handle. Not observed; DuplicateHandle on the
  current process's own handle needs no extra access.
- *Behaviour outside the sandbox.* Only the restore after `fclose(stdout)` now succeeds instead of failing silently
  (the console mode is put back as upstream intended). It also removes a latent hazard: the closed handle value could
  have been reused for another object before the `atexit` handler ran.
- *Other closed-handle uses checked in the pinned source:* `console` (from `is_console`) and `hconsole1/2` are used only
  on the legacy pipe-thread path, where `swap_osfhnd()` already substitutes winansi.c's own duplicates; `winansi_exit`
  uses its own pipe and thread handles and runs before `reset_std_console` (LIFO `atexit` order, unchanged);
  `winansi_get_osfhandle` returns those duplicates or the live fd handle; `compat/terminal.c` opens and closes its own
  `CONIN$`/`CONOUT$` handles; stdin's handle is never stored. Not changed: `GIT_REDIRECT_STDOUT=off`
  (`compat/mingw.c:4333-4336`) closes the standard handle before `winansi_init()` runs, so start-up
  `GetConsoleMode()` would touch a closed handle; MiniClaw never sets that variable.

**Evidence.** The patch applied to the pinned original tarball reproduces the archive's `compat/winansi.c` byte for
byte. Native (VM1, ARM64): see `work/windows-vm/git-build-provenance.md`, "Runtime package (lpac5)", for the build,
the red control with lpac4 (`GIT_RC=8`) and the green run of `git_in_the_terminal_exits_with_its_own_status`.
