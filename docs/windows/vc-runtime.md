# Microsoft Visual C++ runtime on Windows

Sources were read on 2026-10-04. Code: `scripts/setup/vc-runtime.mjs`,
`scripts/setup/windows-vc-runtime-install.ps1`; tests: `tests/setup-vc-runtime.test.mjs`,
`tests/setup-update.test.mjs` (update order).

## Decision

MiniClaw does not ship Microsoft's runtime DLLs (`vcruntime140*.dll`, `msvcp140*.dll`, ...). The user decided
on 2026-10-04 to close review finding G4 (Microsoft DLLs beside the GPLv2 `git.exe`) by not distributing
them. Setup installs Microsoft's own Visual C++ Redistributable from Microsoft instead, the way it installs
Claude Code from Anthropic: one notice, the download runs in the background while the wizard asks its
questions, and the installation runs with administrator approval as the first step of the approved plan.

## Which shipped binaries need it

Import tables were read on 2026-10-04 (`work/windows-vm/vcredist-vm1-probe.log`, `dumpbin /dependents` on
VM1; `objdump -p` on the host for ConPTY and BusyBox).

| Binary | Imports | Needs the redistributable |
| --- | --- | --- |
| `miniclaw-server.exe` (release and debug, `aarch64-pc-windows-msvc`) | `VCRUNTIME140.dll`, UCRT `api-ms-win-crt-*` | **Yes.** No `crt-static` is configured (no `.cargo/config*`, no `RUSTFLAGS`); Rust's MSVC targets link the C runtime dynamically by default. Setup itself runs it (`--secure-data-dir`, `--write-private-file`) and the network service runs a copy of it. |
| Git `git.exe`, helpers and vcpkg DLLs | `VCRUNTIME140.dll`; `msvcp140.dll` (ICU); on x64 also `vcruntime140_1.dll` | **Yes.** Dynamic CRT (vcpkg `*-windows` triplets). |
| `conpty.dll`, `OpenConsole.exe` | UCRT `api-ms-win-crt-*` and Windows DLLs only | No (vcruntime linked statically). |
| BusyBox (mingw) | `msvcrt.dll` (part of Windows) | No. |
| Bundled Node | Node's official build | No. |

The UCRT (`ucrtbase.dll`, `api-ms-win-crt-*`) is part of Windows 10 and later; only the VC runtime part is
installed by the redistributable.

Because the backend needs the runtime and setup starts the backend right after copying it, the runtime is
installed before the backend is built or started (stage `vc-runtime`, before `build`). The update path
installs it before any binary is replaced.

Lead decision offered: linking the backend with `-C target-feature=+crt-static` (Rust reference, "Static and
dynamic C runtimes": https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes) would make
the backend and the network service independent of the redistributable. Only Git would still need it, so the
runtime could then move into the network service's elevated step (one approval prompt instead of two on a
machine without the runtime). Not changed here.

## Official facts used

| Topic | Source | Text / fact |
| --- | --- | --- |
| Download | [Latest supported Visual C++ Redistributable downloads](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) (ms.date 2025-12-03, updated 2026-08-04) | ARM64 `https://aka.ms/vc14/vc_redist.arm64.exe`, X64 `https://aka.ms/vc14/vc_redist.x64.exe`: "Permalink for latest supported ... version". |
| x64 on ARM64 | same | "The X64 Redistributable package contains both ARM64 and X64 binaries." The ARM64 release installs the ARM64 package; an x64 release installs the x64 package, which also covers an ARM64 device running it emulated. |
| Minimum version | same | "a Microsoft Visual C++ Redistributable package at least as recent as the build tools used to build your app must be installed"; "The version of the Redistributable installed on the machine must be the same or later than the version of the MSVC Build Tools used to create your application." |
| Minimum version | [C++ binary compatibility 2015-2026](https://learn.microsoft.com/en-us/cpp/porting/binary-compat-2015-2017) | "the Redistributable version must be at least as new as the latest build tools used by any app component." Backend and Git were built with MSVC 14.51.36231, so `VC_RUNTIME_MINIMUM = '14.51.36231'`. Raise it when the build machine's toolset changes. |
| Detection | [Redistribute Visual C++ files](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files) (ms.date 2026-04-13) | Version in `HKEY_LOCAL_MACHINE\SOFTWARE\Wow6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\{x86\|x64\|arm64}`; "stored in the REG_SZ string value Version and also in the set of Major, Minor, Bld, and Rbld REG_DWORD values". Setup reads `Installed` and the DWORDs through `reg.exe` in both the 32-bit and 64-bit views and takes the newer. On VM1 the key is present in both views. |
| Switches | same | `/install`, `/quiet` ("doesn't display a user interface"), `/norestart` ("suppresses any attempts to restart"). Setup runs `/install /quiet /norestart`. |
| Elevation | same | "If you aren't running from an elevated command prompt, you'll need to respond to a User Account Control prompt to allow the installer to run with administrative privileges." Machine-wide install: administrator rights required. |
| Newer installed | same, and binary-compat page | "If a more recent version is found, the package isn't installed ... an error result is returned"; error `0x80070666` (= 1638). Setup skips the installer when the registry already shows the minimum, and treats 1638 as success. |
| Exit codes | [MsiExec.exe and InstMsi.exe error messages](https://learn.microsoft.com/en-us/windows/win32/msi/error-codes) | 0 success; 1602 user cancelled; 1603 fatal error; 1618 another installation in progress; 1638 another version already installed; 1641 restart initiated (success); 3010 "A restart is required to complete the install. This message indicates success." 1223 is Win32 `ERROR_CANCELLED` (the declined approval prompt). |
| Trust | [Troubleshoot Visual C++ Redistributable installation](https://learn.microsoft.com/en-us/cpp/windows/troubleshoot-vc-redistributable-installation-issues) (ms.date 2026-02-12) | "Never install a Visual C++ Redistributable installer that wasn't downloaded from a Microsoft site. Never install a Visual C++ Redistributable installer that Microsoft didn't sign." |
| Central deployment | Redistribute Visual C++ files | Redistributable packages "enable automatic updating of the Visual C++ libraries"; app-local DLLs are not recommended "for servicing reasons". |

Observed redirect chain (2026-10-04): `aka.ms/vc14/vc_redist.<arch>.exe` -> `aka.ms/vs/18/release/vc_redist.<arch>.exe`
-> `download.visualstudio.microsoft.com/download/pr/.../VC_redist.<arch>.exe` (ARM64: 11,870,816 bytes).

## What setup does

1. `inspect` reads the registry (no download). The checks panel and `--preview` show
   "Microsoft Visual C++ runtime: installed (<version>)", "<version> is too old, setup installs it
   automatically from Microsoft" or "setup installs it automatically from Microsoft". Mac: no row.
2. Missing or older than 14.51.36231: one notice, no question; the download starts at once into a private
   `%TEMP%\miniclaw-vc-runtime-*` folder, removed when setup exits.
3. Download: HTTPS only, every redirect hop restricted to `aka.ms`, `download.visualstudio.microsoft.com`,
   `download.microsoft.com` (port 443, no credentials), at most 5 redirects, bounded at 64 MiB by
   `Content-Length` and while streaming. Before anything runs, the file must have a `Valid` Authenticode
   signature whose subject has `CN=Microsoft Corporation, O=Microsoft Corporation` and whose issuer has
   `O=Microsoft Corporation` (`windows-computer-signature.ps1`, which now also reports the issuer).
4. Before the "Ready to install?" review the wizard waits for the download. A failure (offline, HTTP error,
   wrong signature) stops setup with one message naming the official page; nothing was changed.
5. The review shows "Microsoft runtime: Visual C++ Redistributable (<arch>) from Microsoft, installed with
   administrator approval"; the stage list starts with `vc-runtime`.
6. Stage `vc-runtime` (before the backend is built or started): a clearly labelled approval request.
   `Start-Process -Verb RunAs` starts an elevated Windows PowerShell with the fixed script
   `windows-vc-runtime-install.ps1`, passing only the installer path and its SHA-256. The elevated side creates
   a new `%ProgramData%\MiniClawVcRuntime-<GUID>` folder whose protected access list grants only SYSTEM and
   Administrators (created with that list in one call), reads the file once, re-hashes those bytes, writes them
   into that folder, re-checks Microsoft's signature on the copy, runs only the copy with
   `/install /quiet /norestart`, and deletes the folder.
7. Exit 0 or 1638: continue. 3010 or 1641: continue; setup ends with "Restart Windows when convenient".
   1223/1602: approval declined. 1618: another installation is running. Anything else: Microsoft's exit code.
   All failures name the official page; the unfinished installation folder is removed. After success the
   registry must show 14.51.36231 or later, otherwise it is a failure.
8. `miniclaw update` (`refreshRuntime`) calls `ensureVcRuntime` for the new bundle's architecture before any
   file is replaced; when the runtime is present nothing is downloaded.

Elevation count: on a machine without the runtime, setup asks for administrator approval twice (runtime,
then network service). Folding the runtime into the network service's elevated step is not possible while
the backend links the runtime dynamically: setup runs the backend (`--secure-data-dir`,
`--write-private-file`) before that step, and that step's payload is already at its 30,000-character limit.
With the runtime present (any machine with Visual Studio or a recent app that installed it) there is one.

## Native checks (2026-10-04)

Both VMs already have the machine-wide runtime 14.51.36247 (ARM64, x64 and x86, from Build Tools). Nothing
was uninstalled, so **the real silent installation (`/install /quiet /norestart`) has not been run natively**,
and neither has a click on the UAC consent prompt.

- VM1 (UAC off), `vcredist-native-vm1.log`: production `vcRuntimeStatus` found 14.51.36247.0 for arm64 and x64
  (sufficient); `ensureVcRuntime` downloaded nothing. A real `downloadVcRuntime` for both architectures followed
  aka.ms -> aka.ms -> download.visualstudio.microsoft.com (ARM64 11,870,816 bytes, SHA-256 `b70ef586...`;
  x64 18,731,856 bytes, `84306899...`), both Authenticode `Valid`, `CN=Microsoft Corporation`, issuer
  `Microsoft Code Signing PCA 2024, O=Microsoft Corporation`, file version 14.51.36247.0. Through production
  `installVcRuntime` and a real `Start-Process -Verb RunAs`, the elevated step refused a wrong digest
  (exit 536870913) and an unsigned file (exit 536870914) without running anything, and left no
  `%ProgramData%\MiniClawVcRuntime-*` folder. `wizard.mjs --preview` shows
  "Microsoft Visual C++ runtime   installed (14.51.36247.0)".
- VM2 (UAC on), `vcredist-native-vm2-limited.log`: a RunLevel Limited task (medium integrity, S-1-16-8192)
  ran detection, both real downloads and signature checks and `--preview`; no consent process appeared and
  nothing requested elevation. `vcredist-native-vm2-elevated.log`: a RunLevel Highest task ran the elevated
  step's two refusal paths with the same results as VM1.
- LPAC: ARM64 `git.exe` and `git-remote-https.exe` from the lpac3 ZIP load the runtime from `System32` inside an
  LPAC token, and the ARM64 sandboxed Git acceptance passes 5/5 with it (see git-runtime.md).
