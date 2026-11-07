# MiniClaw for Windows: how to install, and how to build it yourself

Everything here was written against release 0.1.7. The ARM64 build was verified on Windows 11 ARM64; the x64
build was compiled and checked file by file but never run. What changed in each release is in
[RELEASE-NOTES.md](RELEASE-NOTES.md). The original project README
(macOS, architecture, design notes) is at [docs/PROJECT-README.md](docs/PROJECT-README.md); the Windows port's
status and evidence are in [WINDOWS_PORT.md](WINDOWS_PORT.md) and [docs/windows/](docs/windows/).

## macOS (Apple silicon)

Release 0.1.7 also has a macOS package built from the same source: `miniclaw-0.1.7-macos-arm64.tar.gz` on the
Releases page (unsigned, like the September macOS release). Unpack it, then in Terminal run `./miniclaw setup` in
that folder for a new installation, or `./miniclaw update` to update an existing one (stop MiniClaw first), and
start MiniClaw with `./miniclaw`. Its `READ ME FIRST.txt` explains the "cannot check the app" warning. What a Mac
user notices since the September release is listed under macOS in [RELEASE-NOTES.md](RELEASE-NOTES.md). To build
it yourself on an Apple silicon Mac: `npm ci`, then `node scripts/package-release.mjs <new folder>` (needs Rust,
the Xcode command line tools, and the Go toolchain a VM installation unpacks at
`<installation>/network-tools/go`; see `scripts/package-release.mjs`).

## Part 1: just install the ready-made build (no compiling)

1. Download the package for your PC from this repository's Releases page: `miniclaw-<version>-windows-arm64.tar.gz`
   for ARM64 PCs or `miniclaw-<version>-windows-x64.tar.gz` for Intel/AMD PCs, plus its `.sha256` file if you want to
   check it. Extract it somewhere permanent, for example `C:\Users\<you>\MiniClaw` (`tar -xzf` works in PowerShell).
   Keep that folder: its `miniclaw.cmd` is what starts, updates and uninstalls MiniClaw.
2. Open **Command Prompt** or **PowerShell** in that folder and run the setup wizard:

       .\miniclaw.cmd setup

   Press Enter to accept each default. The wizard asks for the install location, the port and
   whether to connect the Codex Computer Use helper. Type `?` or pick "Show me how to find this" at any
   question to get a numbered guide. Expect one Windows administrator prompt ("Windows PowerShell") for the
   network isolation service; a second one only if the Microsoft Visual C++ runtime is missing. Setup takes
   about a minute. If Claude Code is missing it is downloaded from Anthropic (about 250 MB) during setup.
3. Start MiniClaw and open the page:

       .\miniclaw.cmd

   then open http://127.0.0.1:8787 in your browser. Sign in to Claude Code (or Codex) from the plug icon. For
   Claude, MiniClaw opens Claude's sign-in page in your browser and notices by itself when you have approved it.
   If no browser window appears (or it opens behind other windows), use the "Open the Claude sign-in page" link
   shown in MiniClaw; if Claude's page ends by showing a code, paste it into the "Code from Claude" box.
4. Later: `.\miniclaw.cmd update` (stop MiniClaw first, then run it from the newer release's folder),
   `.\miniclaw.cmd repair` (if Windows changed the network settings MiniClaw relies on), `.\miniclaw.cmd uninstall`
   (keeps your data; add `--delete-data` to remove it), `.\miniclaw.cmd guide` (prints the setup guides).

## Part 2: build the release from this repository

You need a Windows 11 machine of the architecture you are building for (ARM64 for the ARM64 package; an x64
package can also be cross-built on ARM64, see step 6). Building takes about 5 minutes on a warm machine, longer
the first time (the Rust dependencies compile once).

### Prerequisites

| Tool | What to install | Where |
| --- | --- | --- |
| Visual Studio Build Tools 2022 or newer | Workload "Desktop development with C++" with the Windows 11 SDK. On ARM64 also add "C++ Clang Compiler for Windows" and the LLVM toolset (the TLS library needs clang-cl on ARM64). On x64 add NASM or use the prebuilt assembly objects described at https://aws.github.io/aws-lc-rs/requirements/windows | https://visualstudio.microsoft.com/visual-cpp-build-tools/ |
| Rust (stable, MSVC) | Install with rustup; this release was built with Rust 1.99. For an x64 package built on ARM64 also run `rustup target add x86_64-pc-windows-msvc` | https://rustup.rs |
| Node.js 24 LTS | Any 24.x; the package itself bundles the pinned Node 24.21.0 it downloads | https://nodejs.org |
| Internet access, or the pinned files | The packager downloads and checks these against pinned SHA-256 values: Node 24.21.0 zip (nodejs.org), BusyBox FRP-6075 binary and source (frippery.org), the ConPTY 1.24.260710001 NuGet package (nuget.org), and MiniClaw's patched Git runtime and its corresponding source (this repository's v0.1.5 release, see step 3) | |
| GitHub CLI, while this repository is private | Signed in with an account that can read the repository; the packager uses its token to fetch the Git files | https://cli.github.com |

### Steps

1. Clone this repository to a path without spaces, for example `C:\dev\miniclaw` (spaces work, but the commands
   below are easier). Open **Developer PowerShell for VS** (so `cl.exe`, `link.exe` and the SDK are on PATH).

2. Install the JavaScript dependencies:

       cd C:\dev\miniclaw
       npm ci

3. Let the packager reach the patched Git runtime. The release bundles MiniClaw's own build of Git for Windows
   2.56.0 with three sandbox patches, and setup refuses to install any other Git. The packager downloads that ZIP
   (`miniclaw-git-2.56.0-lpac5-<arch>.zip`) and its Git corresponding-source archive from this repository's v0.1.5
   release and checks their sizes and SHA-256 values against `scripts/setup/windows-git-bundle.mjs`. While the
   repository is private, GitHub only serves them with a token, so give the packager yours:

       $env:GH_TOKEN = (gh auth token)

   (`GITHUB_TOKEN` works too.) The token is sent only to `api.github.com`. To build offline instead, download the
   two files from the v0.1.5 release yourself and point the packager at them:

       $env:MINICLAW_GIT_ARCHIVE = "C:\pins\miniclaw-git-2.56.0-lpac5-arm64.zip"       # or -x64.zip
       tar -xzf C:\pins\miniclaw-0.1.5-6457c45-windows-arm64-git-corresponding-source.tar.gz -C C:\pins
       $env:MINICLAW_GIT_SOURCE  = "C:\pins\git-corresponding-source"

   `MINICLAW_SOURCE_OFFER_CONTACT` names who honours the written source offer (default `Sep`). The corresponding
   source is what the GPL requires you to publish beside your own release; the packager writes it next to the
   package. To rebuild Git itself, follow `git-build-native.ps1` and `git-build-inputs.json` inside that source
   folder; `docs/windows/git-runtime.md` explains the pins and patches. A rebuilt Git needs its pin updated in
   `scripts/setup/windows-git-bundle.mjs`.

4. Optional, to work offline: download the four pinned files yourself and pass them instead of letting the
   packager fetch them:

       $env:MINICLAW_NODE_ARCHIVE    = "C:\pins\node-v24.21.0-win-arm64.zip"     # or win-x64
       $env:MINICLAW_BUSYBOX_BINARY  = "C:\pins\busybox-w64a-FRP-6075-g169694ebd.exe"   # w64u for x64
       $env:MINICLAW_BUSYBOX_SOURCE  = "C:\pins\busybox-w32-FRP-6075-g169694ebd.tgz"
       $env:MINICLAW_CONPTY_ARCHIVE  = "C:\pins\microsoft.windows.console.conpty.1.24.260710001.nupkg"

   The URLs are in `scripts/setup/windows-node-sandbox.mjs`, `windows-shell.mjs` and `windows-conpty.mjs`.

5. The package takes its version from `server/Cargo.toml` (0.1.7 in this source). For a build of your own, edit
   `version = "..."` there, run `cargo update -p miniclaw-server --offline` inside `server` so `Cargo.lock`
   matches, and set the same version in `package.json`.

6. Build and package in one step. This compiles the OpenSSL seed library (`openssl-seed`), the backend in release
   mode, the web page with Vite, verifies every pinned file, and writes the portable folder, its archive and the
   Git source archive next to each other:

       node scripts\package-release.mjs C:\dev\out\miniclaw-windows-arm64

   gives `C:\dev\out\miniclaw-windows-arm64\`, `miniclaw-windows-arm64.tar.gz` and
   `miniclaw-windows-arm64-git-corresponding-source.tar.gz` (the folder must not exist yet). The released packages
   are named `miniclaw-<version>-<commit>-windows-<arch>`; their `.sha256` files are written with
   `Get-FileHash -Algorithm SHA256`. For an x64 package on an ARM64 machine, set
   `$env:CARGO_BUILD_TARGET = "x86_64-pc-windows-msvc"` first (with the offline variables, use the x64 Git ZIP).

7. Run the installer from the build you just made: extract the archive (`tar -xzf` works in Windows PowerShell)
   and follow Part 1 from inside the extracted folder.

### Checking your build before you hand it out

Quick host checks, from `C:\dev\miniclaw` (a few minutes):

    npm run check
    npm run test:web
    npm run test:setup
    cd server; cargo test --locked; cd ..

The native Windows acceptance script exercises the sandbox, the network service and the standard-user cases
(it creates and removes a disposable user and service, and needs an elevated session):

    powershell -ExecutionPolicy Bypass -File scripts\windows-native-acceptance.ps1

`docs/windows/README.md` describes what each group covers and what still counts as unverified (x64 in the
sandbox, Codex live chat, real Computer Use actions). The full history of decisions and evidence is in
`WINDOWS_PORT.md` and `docs/windows/`.
