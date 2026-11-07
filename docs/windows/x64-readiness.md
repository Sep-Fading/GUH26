# Windows x64 readiness (static audit, 2026-10-04)

**Status: Windows x64: builds and passes host checks; not verified natively in the sandbox.**

All native verification of the port ran on Windows 11 ARM64 (UTM guests on an Apple silicon Mac). This
audit was done on the Mac only. It checks every place where the port chooses something by CPU
architecture, recomputes the x64 pins from the real files, and examines the one failing x64 run.
Scripts and logs are in `work/x64-audit/` (outside the repository).

## Verdict

| | Result |
| --- | --- |
| Proven by bytes | x64 BusyBox, ConPTY (`conpty.dll`, `OpenConsole.exe`) and Git (`miniclaw-git-2.56.0-lpac2-x64.zip` in this audit; the current pin is `lpac5`, see the note in section 1) match their pinned sizes and SHA-256. Every executable and DLL among them is PE machine 0x8664: 32 of 32 in the Git ZIP. In the Git ZIP, every import that is not a system DLL resolves to a file in the same folder. The x64 and ARM64 `git.exe` have identical import lists and PE header values (stack and heap reserve, DLL characteristics). |
| Proven by host tests | x64 branches of the pins, PE machine mapping, Node, Claude and Codex selection (`tests/setup-x64.test.mjs`, plus existing tests). The Windows GNU x64 `cargo check --all-targets` and `cargo clippy --all-targets` both pass. |
| Earlier native x64 evidence | On the ARM64 guest under x64 emulation, outside the sandbox, from source snapshot `bacac2b5`, which is older than the current tree: the x86_64-pc-windows-msvc build compiled, including aws-lc. `platform_fs` 8/8, `secrets` 3/3 and `workspace_commands` 4/4 passed, and a run of library filters passed. One `windows_process` EOF test failed while a compile was competing for CPU, then passed alone (`x64-native-first.log`, `x64-eof-rerun-passed.log`). x64 Git built and passed its clean-PATH check under emulation (`git-buildpkg-x64-v3.log`). |
| Reasoned | The Rust Windows code has no x64-specific risk. Both targets are 64-bit LLP64: struct sizes come from `size_of`, buffers are `usize`/`u64` aligned, there is no thread-context or arch-specific FFI, and the registry views are identical. The x64 `git.exe` failure inside the sandbox under ARM64 emulation is most likely an emulation artifact (section 4). |
| Unknown | Any sandboxed run on real x64 hardware (shell, ConPTY, Git, WFP service, standard user, installed release). The x64 CI job has never run, and as written it cannot pass its Git or package steps (section 3). |

## 1. Architecture-dependent decisions

"Host test" means a Mac or Linux host can run the test without Windows.

| Place | x64 branch | Pinned | Host test |
| --- | --- | --- | --- |
| `scripts/setup/plan.mjs` `checkHost` | accepts `win32/x64` | n/a | `tests/windows-setup.test.mjs` (x64 host) |
| `scripts/setup/pe-machine.mjs` | 0x8664 → `x64` | n/a | `tests/setup-x64.test.mjs`, `tests/backend-package.test.mjs` |
| `scripts/setup/windows-shell.mjs` | `busybox-w64u` (Unicode build) | 675840 B, `6e263d15…` | `tests/setup-x64.test.mjs`; real bytes with `MINICLAW_X64_ARTIFACTS` |
| `scripts/setup/windows-conpty.mjs` | `runtimes/win-x64/native/conpty.dll`, `build/native/runtimes/x64/OpenConsole.exe` | `39fba271…`, `b7fd936c…` (same nupkg as ARM64) | `tests/setup-conpty.test.mjs`, `tests/setup-x64.test.mjs` |
| `scripts/setup/windows-git-bundle.mjs` | `miniclaw-git-2.56.0-lpac5-x64.zip` | 29632338 B, `66f3c549…` (see the note below) | `tests/setup-windows-git.test.mjs`, `tests/setup-x64.test.mjs` |
| `scripts/setup/windows-git.mjs` `windowsGitPackage` (official MinGit, test fixture only) | `ucrt64`, `MinGit-2.56.0-64-bit.zip` | `064b440f…` | `tests/setup-windows-git.test.mjs` |
| `scripts/setup/claude-install.mjs` | `win32-x64` | per-release manifest checksum | `tests/setup-claude-install.test.mjs` |
| `scripts/setup/providers.mjs` | `codex-win32-x64` / `x86_64-pc-windows-msvc` | n/a | `tests/windows-setup.test.mjs` |
| `scripts/setup/windows-computer.mjs` | accepts `codex-computer-use.exe` or `-arm64.exe` by name only | Authenticode signer | `tests/windows-setup.test.mjs` (no architecture check, see gap G5) |
| `scripts/setup/prebuilt.mjs` | refuses a release whose `arch` ≠ `process.arch` | release manifest | `tests/setup-prebuilt.test.mjs` |
| `scripts/setup/update-runtime.mjs` | refuses an installed vs new architecture mismatch (new, R2) | n/a | `tests/setup-update.test.mjs` |
| `scripts/backend-package.mjs` | architecture from backend PE header; selects shell, ConPTY and Git for it | via the pins above | `tests/backend-package.test.mjs` (0x8664 and 0xAA64) |
| `scripts/package-release.mjs` | `node-v24.21.0-win-x64.zip` (37618919 B, `158f7685…`); x64 READ ME status line | yes | `tests/setup-release.test.mjs` |
| `server/src/windows_shell.rs:49-52` | `cfg(not(target_arch = "aarch64"))` → x64 size and hash | same values as JS | Windows-only native tests |
| `server/src/windows_conpty.rs:81-90` | same pattern, x64 hashes | same values as JS | Windows-only native tests |
| `server/src/windows_network_boot.rs:65` | `KEY_WOW64_64KEY` on an HKLM key; does nothing in a 64-bit process on either architecture | n/a | Windows-only |
| `server/tests/windows_winsock_acl_diagnostic.rs` | `Catalog_Entries64`, the view a native 64-bit process reads on x64 and ARM64 alike (`Catalog_Entries` is the 32-bit view) | n/a | diagnostic only |
| `.github/workflows/windows.yml` | runner `windows-2025` (x64); x64 is the only architecture CI builds | uses the pins | `tests/setup-ci.test.mjs`; the workflow has never run |
| `scripts/windows-native-acceptance.ps1` | `-Target x86_64-pc-windows-msvc` → machine 0x8664, `x64` staging, PE check of every artifact | via the pins above | not runnable on a host |

The Rust code has no other architecture branches. There is no `IsWow64Process2`, `GetNativeSystemInfo`, inline assembly or thread-context code, and no `build.rs`. The model briefing reports `std::env::consts::ARCH` (`briefing.rs:82,164`).

Pointer-size review: clippy with `cast_possible_truncation`, `cast_possible_wrap`, `cast_sign_loss` and `cast_ptr_alignment` reports 163 cast warnings. 83 are `usize`→`u32`. All of them apply equally to x64 and ARM64 because both have 64-bit pointers. The DWORD lengths they produce are `size_of` constants or bounded buffers, for example `secrets_windows.rs:34`, which is checked at line 28. The one alignment warning, `windows_sandbox_revoke.rs:340`, is a false positive: the buffer is `Vec<u64>`. Pointer-to-integer casts for job handles are bounded by `isize::MAX` (`windows_network_broker.rs:53`). An unchecked `as u16` for terminal `cols`/`rows` (`terminal.rs:680`) is unrelated to architecture and applies only to the Mac VM guest path. Log: `work/x64-audit/crosscheck-clippy.log`, list `clippy-casts.txt`.

VC++ runtime, checked because the lead asked for it: every x64 Git executable and DLL imports `VCRUNTIME140.dll`. The x64 `icuuc78.dll` also imports `MSVCP140.dll` and `VCRUNTIME140_1.dll`. `VCRUNTIME140_1.dll` is the x64-only C++ exception-handling DLL (the ARM64 `icuuc78.dll` does not import it), and `vc_redist.x64.exe` provides it. ConPTY and BusyBox import no VC++ runtime DLL (`work/x64-audit/vcrt-imports.log`). Rust MSVC targets link the C runtime dynamically unless `crt-static` is set ([Rust reference, static and dynamic C runtimes](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)), and no `crt-static` setting exists in this repository. So `miniclaw-server.exe` most likely imports `VCRUNTIME140.dll` on both architectures. No built backend exists on the host to confirm this. Microsoft documents the runtime registry key as `HKLM\SOFTWARE\Wow6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\{x86|x64|arm64}` ([Redistributing Visual C++ files](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files)). The in-progress `scripts/setup/vc-runtime.mjs` (vcredist work) reads both the `/reg:32` and the
`/reg:64` view, selects `vc_redist.x64.exe` for `win32/x64` (`vcRuntimeArch`, `vcRuntimeUrl`; host test
in `tests/setup-vc-runtime.test.mjs`), and installs the runtime before setup first runs the backend
(`install.mjs`).

Note: the Git pin was `lpac4` (2026-10-05) and is now `lpac5` (update at the end of this note): `lpac3` dropped the Microsoft runtime DLLs (lead decision 2026-10-04), and `lpac4` adds OpenSSL, libssh2 and curl rebuilt so that libcrypto seeds from `BCryptGenRandom` (docs/windows/git-runtime.md, "HTTPS inside the sandbox"). The Git executables are the same builds, so the PE machine and import results above still apply to them. The byte and hash results in section 2 apply to `lpac2` only. For `lpac4` x64 (29629989 bytes, `f3f1525751a6f82fbde8da3c1c7b09533a69789ad7c5d2d40f25e76177766d94`, 68 entries), `tests/setup-x64.test.mjs` with `MINICLAW_X64_ARTIFACTS=work/windows-vm` re-checked the pin and that every executable and DLL (28 of 28) is PE machine 0x8664 (2026-10-05, `work/windows-vm/git-https/host-setup-x64-real-bytes.log`), and its clean-PATH check including HTTPS `ls-remote` passed under x64 emulation on the ARM64 guest (`work/windows-vm/git-https-pkg-x64-vm1.log`). Update 2026-10-05 (git-console agent): the pin is now `lpac5`, Git recompiled from the same recipe with the third source patch (`git-lpac-console-handle.patch`; docs/windows/git-runtime.md, "Git on the sandboxed terminal"); x64 `lpac5` is 29632338 bytes, `66f3c549d5a5d995c562efdf4c722a468006e683d0c15e0d3a36f1f744f30d1c`, 69 entries. `tests/setup-x64.test.mjs` with `MINICLAW_X64_ARTIFACTS=work/windows-vm` re-checked the pin and every executable and DLL (28 of 28) as PE machine 0x8664 (`work/windows-vm/git-console-host-setup-x64-real-bytes-1.log`); the packager's own per-file check and the clean-PATH check including HTTPS `ls-remote` passed under x64 emulation on VM1 (`work/windows-vm/git-console-build-x64-vm1-1.log`). Sandboxed x64 acceptance still needs x64 hardware.

## 2. Bytes recomputed on the host

`node work/x64-audit/verify-bytes.mjs` (log `verify-bytes.log`, 26/26 PASS):

| File | Bytes | SHA-256 | PE machine |
| --- | --- | --- | --- |
| `busybox-w64u-FRP-6075-g169694ebd.exe` | 675840 | `6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c` | 0x8664 |
| nupkg `runtimes/win-x64/native/conpty.dll` (= `conpty-x64/conpty.dll`) | 109920 | `39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8` | 0x8664 |
| nupkg `build/native/runtimes/x64/OpenConsole.exe` (= `conpty-x64/OpenConsole.exe`) | 1066296 | `b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160` | 0x8664 |
| `conpty.1.24.260710001.nupkg` | 1732296 | `175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e` | n/a |
| `miniclaw-git-2.56.0-lpac2-x64.zip` | 30042556 | `6693d5e04aced9683052bec2ec7c08c689db91acd6334d06257ed4ad979f52d5` | 32/32 entries 0x8664 |

The ARM64 controls also pass, with 31/31 entries 0xAA64. `pe-imports.mjs` (log `pe-imports.log`) confirms that every non-system import resolves to a file in the same folder. `tests/setup-x64.test.mjs` repeats the check through the production readers (`shellFiles`, `conptyFiles`, `gitBundleFiles`) when `MINICLAW_X64_ARTIFACTS` names the evidence folder (log `setup-x64-real-bytes.log`, 3/3).

## 3. Gaps

Fixed in this change, each with a failing test written first:

- **G1**: no host test pinned the x64 BusyBox and ConPTY choices; the existing tests used `process.arch`. Added `tests/setup-x64.test.mjs`.
- **G2 (R2)**: `miniclaw update` trusted the new bundle's architecture. It now refuses a mismatch before changing any file.
- **G3 (R1)**: `installWindowsGit` defaulted to the official MinGit pin. The default is removed, and the MinGit release path and the GitHub release-asset host are no longer approved download hosts.
- **G4**: the x64 READ ME and this folder's README did not state that x64 is unverified in the sandbox. They do now.

Reported for other owners or for a lead decision:

- **CI (unowned workflow)**: `windows.yml` stages the official MinGit as `MINICLAW_TEST_GIT_ARCHIVE`. Official MinGit is known to fail `git_arguments_and_boundary` inside the sandbox, so "Native Windows acceptance" fails on x64. "Package Windows release" requires `MINICLAW_GIT_ARCHIVE` and `MINICLAW_GIT_SOURCE`, which have no public download, so `package-backend.mjs` stops. The GitHub image also already contains the VC++ runtime and Visual Studio, which would hide a missing-runtime defect. Making CI an x64 lane needs a fetchable, pinned copy of the x64 Git build and its source, plus a lead decision. Since 0.1.6 the packager fetches both from the v0.1.5 release assets when the variables are unset, checked by size and SHA-256 (a token is needed while the repository is private; docs/windows/git-runtime.md).
- **G5 (minor, `windows-computer.mjs` / `computer.rs:351-357`)**: the helper is accepted by file name, with no PE machine check. An ARM64 helper chosen on x64 fails only at launch, with error 193.
- **Git architecture source**: `plan.mjs` chooses Git by Node's `process.arch`, not the backend's PE machine. On real x64 they always match. They can differ only on ARM64 running an x64 Node from a checkout, and then setup fails closed because the archive is missing.
- **vcredist (in progress)**: `installVcRuntime` defaults `arch` to `'arm64'` (`vc-runtime.mjs:181`). Current callers pass the plan's architecture, but a caller that omits it on x64 would install the ARM64 package, the same class of risk as R1; make `arch` required. On both architectures, confirm natively that LPAC processes can load `System32\vcruntime140.dll` once the Git folder no longer carries a copy. Windows grants ALL RESTRICTED APPLICATION PACKAGES read access in System32 by inheritance, but this has not been checked here.

## 4. The x64 `git.exe` STATUS_NO_MEMORY failure

Evidence (`work/windows-vm/git-accept-x64emu-v3-red-status-no-memory.log`, VM1, 2026-10-04): the ARM64
backend, broker and test executables ran with the x64 Git ZIP installed by the same production
installer. Inside the same LPAC sandbox, the ARM64 child diagnostic ran normally: it read the working
directory and confirmed file identity. Then `git --version` (x64, emulated) exited `3221225495` =
0xC0000017 STATUS_NO_MEMORY, with empty output. The same binary passes the clean-PATH check, including
init, commit, grep and HTTPS, under emulation outside the sandbox (`git-buildpkg-x64-v3.log`).

What MiniClaw itself imposes: the job object sets only `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
(`server/src/windows_process.rs:41-46`). There is no process or job memory limit and no address-space
or mitigation policy (no `PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY`). The only process attributes are
the job list, handle list or pseudoconsole, security capabilities, and the
ALL_APPLICATION_PACKAGES opt-out that makes the token LPAC (`windows_process.rs:118-145`). A job
commit limit, the one MiniClaw setting that would also fail on real x64 hardware, therefore does not
exist ([JOBOBJECT_EXTENDED_LIMIT_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information)).

How emulation differs: Microsoft documents that x64 code on Arm is JIT-translated, that "A service
caches these translated blocks of code", and that for x64 "there's no WOW64 layer"; system binaries are
Arm64X files loaded from System32 ([How emulation works on Arm](https://learn.microsoft.com/en-us/windows/arm/apps-on-arm-x86-emulation),
[Arm64X PE files](https://learn.microsoft.com/en-us/windows/arm/arm64x-pe)). Independent research
describes the emulated process telling the XtaCache service about each loaded image over ALPC. The
service then maps cached translations into the process from `%SystemRoot%\XtaCache`, which only
the service can open
([FFRI, Black Hat EU 2020](https://i.blackhat.com/eu-20/Wednesday/eu-20-Nakagawa-Jack-In-The-Cache-A-New-Code-Injection-Technique-Through-Modifying-X86-To-Arm-Translation-Cache.pdf)).
An emulated process therefore depends on an ALPC port, mapped sections and an emulator DLL that a
native process never touches. LPAC denies objects that lack an ALL RESTRICTED APPLICATION PACKAGES or
capability grant. No Microsoft document or public bug reports this exact case (x64 under LPAC on Arm
fails with 0xC0000017). Chromium's sandbox does special-case processes running under emulation
(`IsRunningEmulatedOnArm64` in `sandbox/policy/win/sandbox_win.cc`).

Judgement: **most likely an emulation-in-LPAC artifact that would not occur on real x64 hardware.**
Confidence: **medium-high (about 80%)**. Supporting facts:

- The failure is limited to emulated and LPAC together: emulated outside the sandbox works, and native inside the sandbox works.
- The x64 and ARM64 `git.exe` have the same imports and PE header values, and every import resolves.
- MiniClaw sets no memory or mitigation limit that real hardware would also apply.

The remaining 20% covers an x64-only startup path that needs an object LPAC denies. That would surface as
access denied or a missing DLL rather than STATUS_NO_MEMORY, but it has not been excluded by a run.

**The single test on real x64 hardware that settles it**: on an x64 Windows 11 machine, run
`scripts/windows-native-acceptance.ps1 -Target x86_64-pc-windows-msvc` elevated, with
`MINICLAW_TEST_GIT_ARCHIVE` set to the pinned x64 MiniClaw Git ZIP (the script selects the bundled pin
by file name). It runs `windows_git_commands git_arguments_and_boundary` inside LPAC. If `git --version`
exits 0 there, the failure is an emulation artifact. If it exits 0xC0000017, it is a real x64 defect.
On the existing ARM64 VM, a cheaper test would separate the two causes without x64 hardware: launch the
same x64 `git.exe` in an AppContainer without the LPAC opt-out. If that works, the cause is
LPAC access to emulator resources.

## What was not done

No Windows guest and no x64 hardware were used. The x64 MSVC target was not built here: the
cross-check uses the GNU target with the same `windows-sys` code paths. No sandboxed x64 process has
run. The workflow was not changed or run.
