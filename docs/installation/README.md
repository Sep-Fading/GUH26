# Installation

`miniclaw setup` installs MiniClaw on an Apple silicon Mac from the source checkout.
The backend always runs on the Mac. A macOS VM for untrusted projects is optional and
off by default. See the [sequence](sequence.md).

## Commands

```text
miniclaw                  start the remembered installation
miniclaw setup [PATH]     install; --preview, --mac-only
miniclaw start [PATH]     start an installation
miniclaw resume [PATH]    finish an interrupted VM setup
miniclaw ledger           show the shared VM budget; --reconcile repairs it
miniclaw package [FOLDER] build a self-contained release (from a checkout)
miniclaw verify [--chat]  check a running MiniClaw and its VM, feature by feature
miniclaw update [PATH]    rebuild the backend, the page and the VM scripts in place
miniclaw update-vm [PATH] rebuild the VM's setup disk; --computer-use adds the helper
miniclaw where            print the remembered installation
```

Setup records the installation in
`~/Library/Application Support/MiniClaw/installation.json`
([default-installation.mjs](../../scripts/setup/default-installation.mjs)), so a path is
needed only for another installation. The older `--destination` and `--installation`
flags still work.

`--preview` prints the system check and the default plan and changes nothing.
`--mac-only` skips the VM question. `miniclaw resume` finishes an interrupted VM setup.
The default destination is `~/Library/Application Support/MiniClaw Install`; an
existing path is refused. `miniclaw start` refuses an installation whose
`settings.json` says `backend: "linux"`, which the retired Linux controller wrote.

## Releases: installing without the checkout

`miniclaw package [FOLDER]` ([package-release.mjs](../../scripts/package-release.mjs)), run
from a checkout on a Mac that has a VM installation, builds a self-contained release: a folder
and a `.tar.gz` of it. The audited 2026-09-25 build was 63 MB.

| In the release | What |
|---|---|
| `miniclaw` | A launcher that runs the scripts with the release's own Node |
| `node/` | The official Node 24.21.0 for Apple silicon, downloaded from nodejs.org and checked against its published SHA-256, which is pinned in the packager |
| `scripts/`, `node_modules/` | The CLI, setup and VM scripts, and only the packages they need at run time |
| `prebuilt/` | The backend and page, the VM worker, the network helper, the VM supervisor, the guest's capture helper and socket probe, and `manifest.json` with every file's SHA-256 |

VM restore images and disk media stay outside the source checkout and release. When the
optional VM is selected, [`wizard.mjs`](../../scripts/setup/wizard.mjs) uses
`~/Library/Caches/MiniClaw-setup` for verified downloads. The packager rejects VM media
names in copied trees and archive entries. Its check is in
[`package-release.mjs` `vmMedia`](../../scripts/package-release.mjs):

```js
function vmMedia(name) {
  return /\.(?:ipsw|dmg|img|iso|qcow2|vmdk|vdi|vhdx?|sparseimage|sparsebundle|ova)$/i.test(name);
}
```

On the target Mac nothing else is needed: no Rust, Swift, Go, Node or Homebrew.
[`prebuilt.mjs`](../../scripts/setup/prebuilt.mjs) decides: a checkout has no manifest and
compiles as before; a release uses its prebuilt files, each checked against the manifest
first. A release with a missing or changed file stops with that reason and never falls back to
compiling. A release also skips the 65 MB Go toolchain download, and an installation made from
one keeps the two guest helpers, so a project VM's setup disk can be made later.

The release is **not signed** with an Apple Developer ID. macOS asks once before the first
run; `READ ME FIRST.txt` in the release explains "Open Anyway" and the `xattr` alternative.
The VM supervisor is signed ad hoc with its entitlement on the target Mac, as before.

Before compilation, the packager checks the scripts, web source and backend source for
credential files and recognizable secret literals. It refuses `.env` files, credential
configuration, private keys, database files, links, populated credential templates and
literal key assignments. Finder metadata is omitted when copying scripts. The pinned Node
archive contributes only `bin/node` and `LICENSE`. The packager then checks the completed
release folder and the raw tar headers before reporting success. A failure removes the new
folder and archive. See the [release packaging sequence](sequence.md#release-packaging).
The content check recognizes common formats; it cannot certify that an arbitrary string in
source code is not a secret. The backend source scan skips `project_memory.rs` because its
test module contains synthetic detector examples; its production section was checked
separately for those marker formats.

The final two checks in [`package-release.mjs`](../../scripts/package-release.mjs) run after
the release files have been assembled:

```js
    await auditReleaseTree(output);
```

After tar creation, `auditReleaseArchive` inspects raw headers because macOS `tar -t`
hides AppleDouble entries from its listing. The two empty
`credentials.env.example` templates are retained for local setup.

Observed on 2026-09-21: the release was built for real, its Mac part was installed into a
scratch folder with `PATH=/usr/bin:/bin` and the release's own Node, and the installed backend
answered `/api/health` and served the page within 2 s. Not observed: a VM installation from a
release (it takes a macOS install), and a Mac that truly has no developer tools.

## Sep's Roblox Development Bundle

Setup offers an unchecked **Install Sep's Roblox Development Bundle?** checkbox in both
Mac-only and VM setups. Its indented, italic contents come from the bundled manifest:
four MCPs (`blender`, `roblox_studio`, `blender-roblox`, `roblox-devproducts`), five skills,
Development Skills and Universal groups, and Blender Agent. The main agent's skill
assignments and saved agent permissions are included. The VM test agent is excluded.

[`questions.mjs`](../../scripts/setup/questions.mjs) preserves the selection when reviewing
choices again. [`install.mjs`](../../scripts/setup/install.mjs) adds one stage after building
the backend:

```js
    let robloxBundle = null;
    if (plan.robloxBundle) {
      onStage('roblox-bundle');
      robloxBundle = await installBundle({ destination, providers: plan.providers, signal, onOutput });
    }
```

[`roblox-bundle.mjs`](../../scripts/setup/roblox-bundle.mjs) verifies packaged asset checksums,
creates an isolated Python environment, and installs the included official Blender Lab
wheel plus its compatible MCP SDK. The wheel comes from revision
`ff54e4d8f6b09502f2f466189cca0e52b4a91643`, with entry point `blmcp:main`; the same-named
PyPI project is unrelated. Setup checks `import blmcp` before accepting the installation.
It requires Python3.10+ or uv. Python dependencies still need a network connection, so this
optional bundle has prerequisites beyond the base MiniClaw release.

The backend's `--initialize-data` mode creates and seals a fresh profile database using
normal migrations, then exits without starting a provider or listener. The installer writes
profiles and rules in one SQLite transaction. Fresh IDs keep skill, group and MCP references
together without inheriting another installation's Keychain entries. Paths are resolved
inside `<installation>/bundles/sep-roblox`. Existing databases are refused. A failed stage
rolls back the new installation through the existing installer cleanup.

Saved activation and permissions remain separate. Blender Agent's saved Full access default
is included and disclosed in the prompt/review. Credential values, sessions, conversations
and projects are excluded. MCP credential names become required settings. The bundle includes
the Blender MCP add-on ZIP and Studio bridge plugin source, but installing/connecting the
apps, starting the bridge and entering local credentials remain explicit follow-up steps in
its [SETUP.md](../../scripts/setup/bundles/sep-roblox/SETUP.md). Normal chat works meanwhile.

The existing release packager copies the entire `scripts/` tree, including this versioned
bundle. A normal runtime update does not reapply it over edited profiles. The optional
[sequence](sequence.md#optional-roblox-bundle) maps installation boundaries. Fixture tests
cover both branches, reference preservation, rollback and backend reopen. A real disposable
install initialized the official Blender MCP and listed26 tools on2026-09-25.

## Updating an installation

An installation runs its own copies: the backend in `runtime/bin`, the page in
`runtime/web`, the launcher, and the VM session scripts in `runtime/vm`. A change in the
checkout reaches none of them until `miniclaw update`
([update-runtime.mjs](../../scripts/setup/update-runtime.mjs)) runs. It builds a fresh
bundle in a temporary folder inside the installation, then replaces the backend, the page,
the launcher, the VM session with everything it imports, and the project VM setup files.
Each file is written beside its target and renamed over it. No backup is kept, because
every replaced file can be rebuilt from the checkout, and the build folder is removed
afterwards. The manifest is replaced last. The worker and the setup disk are not touched;
they belong to `miniclaw update-vm`. Restart `miniclaw` afterwards to load the new backend.
An installation without a VM gets no VM scripts.

## Updating a VM

The VM's worker and capture helper are installed from the read-only setup disk.
`miniclaw update-vm` ([update-media.mjs](../../scripts/vm/update-media.mjs)) rebuilds that
disk with the current worker and helper while the VM is off. It reuses the pairing key from
`worker.json`, so the Mac still trusts the VM, replaces the previous disk without keeping
a copy (the new one is complete before the swap, so an update takes no extra space and
never creates a VM), and refreshes `runtime/miniclaw-worker`, from which project
VMs are provisioned. It refuses while a VM of the installation is running:

```js
// scripts/vm/update-media.mjs, updateMasterMedia
if (await vmRunning(destination)) throw new Error('Shut the VM down first, then run this again.');
```

`--computer-use [APP]` also puts the Codex Computer Use helper on the disk, by default from
`~/.codex/computer-use/Codex Computer Use.app`, and records the choice in `settings.json`
(`computer.path`, `masterSetup.computerUse`, `pair.provisioning.computerApp`). Later updates
keep the helper, and project VMs created afterwards receive it.

The user then runs "Install Codex.command" from the disk inside the guest again. The
installer replaces an older capture helper. Files on the setup disk carry Finder metadata
that a strict signature check refuses, so the installer copies without metadata and
verifies the copy ([install_desktop_helper](../../scripts/vm/install-master.command)). A new
worker has a new file name, so macOS asks again for its Screen Recording permission.

## What the wizard does

1. Checks the Mac: hardware, Node, Cargo, `/usr/bin/sandbox-exec`, and both providers
   with their sign-in state. Setup stops if Cargo or every provider is missing.
2. Asks which installed providers to enable, the port, and whether to add the VM.
   Choosing the VM adds its resource limits, size and the Computer Use helper.
3. Shows a review and installs only after approval.
4. Runs numbered steps: a progress bar per download, and a build log that clears on
   success and stays on failure.
5. Without a VM, prints the start command and any provider still to sign in. With a
   VM, runs first-time VM setup.

Provider discovery keeps a yes or no, never an account name:

```js
// scripts/setup/providers.mjs, detectProviders
const result = await probe(path, provider.status).catch(() => ({ code: 1, stdout: '' }));
found.push({ id: provider.id, name: provider.name, path, signedIn: provider.signedIn(result) });
```

Codex is asked with `codex login status` (exit code) and Claude with
`claude auth status --json` (`loggedIn`). Codex is looked for in `CODEX_BIN`,
`/Applications/Codex.app`, `/Applications/ChatGPT.app` and `PATH`; Claude in
`CLAUDE_BIN`, `~/.local/bin/claude`, `~/.claude/local/claude` and `PATH`.

## Plan rules

[`planSetup`](../../scripts/setup/plan.mjs) validates every choice before anything is
written. The backend must be the Mac, at least one provider must be enabled, and the VM
choice must agree with its allocation:

```js
// scripts/setup/plan.mjs, planSetup
if ((input.backend ?? 'host') !== 'host' || input.linux) {
  throw new Error('The backend always runs on the Mac');
}
```

A VM plan downloads the macOS restore image, the Codex package and the pinned Go
toolchain for the network helper, each checksum verified. CPU, RAM and disk are checked
against the global VM budget and free space. `recommend` sizes the VM for the Mac but
never turns it on.

## What gets installed

```text
<destination>/
  settings.json            private receipt: port, providers, binaries, pair
  data/providers.json      the wizard's provider choice, read by the backend
  runtime/                 bin/miniclaw-server, web/, start-host.mjs, manifest.json
  runtime/vm/, runtime/setup/   VM installs only: the session and what it imports
  worker.json              VM installs only: the guest worker key and workspace
  master/, master-setup/   VM installs only: the VM and its setup disk
```

[`install`](../../scripts/setup/install.mjs) creates the destination itself and removes
it again on any failure, so a failed run leaves no half installation. Verified downloads
stay in `~/Library/Caches/MiniClaw-setup`. Only enabled providers are wired in:

```js
// scripts/setup/install.mjs, install
const binaries = { codex: plan.providers.codex ? codexBin : null,
  claude: plan.providers.claude ? claudeBin : null };
```

`start-host.mjs` passes an empty `CODEX_BIN` or `CLAUDE_BIN` for a provider that was
not wired in, which disables it instead of letting the backend rediscover it.

`copyVmRuntime` copies `vm/session.mjs`, `vm/readiness.mjs` and every local module they
import, keeping the relative layout. A test imports the copied session in a clean
process.

## The VM layout

[`prepareMacVm`](../../scripts/vm/prepare-mac.mjs) builds the guest worker, writes
`worker.json`, prepares the setup disk, builds and signs the VM binary, installs macOS
in the guest and builds the network helper. It returns a pair with `mac`:

```js
// scripts/vm/prepare-mac.mjs, prepareMacVm
return { root: destination, port: plan.port, mac: { worker: workerSettings },
  master: { binary, guest, media }, network,
```

There is no Linux controller and no separate Mac worker. The
[VM connection](../vm-connection/README.md) page describes the session this pair starts.

## First-time VM setup

[`connectSetup`](../../scripts/setup/connect.mjs) starts the session, starts the
installed backend with the session's link file, boots the VM once, waits for the guest
worker, then signs in to Codex inside the guest. VM routes need one provider signed in on
the Mac, so the wizard asks for that first when needed. The backend stops before the VMs:

```js
// scripts/setup/connect.mjs, connectSetup
const close = async () => { await backend?.close(); return session.close(); };
```

Guest sign-in uses the VM's own worker through `POST /api/vms/master/auth/start` and
polls `GET /api/vms/master/status` for `signedIn`. The Mac's `/api/auth/start` is never
used. After success the receipt is rewritten with `vmReady: true`.

## Verification

`npm run test:setup` covers the planner, questions, panels, progress reporter, provider
discovery, installer transaction, guest sign-in and setup ordering with injected
collaborators. On 2026-09-20 a Mac-only install was run through the real wizard in a
pseudo-terminal: it built, installed, and the installed copy started with both
providers connected and served the UI. The VM path has not been run against a real VM.

### Roblox connection guide

When the optional Roblox bundle is selected, the wizard offers a group-owned or
personal connection guide after installation. It covers separate asset and
developer-product keys, owner and universe IDs, private storage, Studio/Blender
connections and health checks. It never asks for key values. Skip or cancel to
finish later using the bundle's SETUP.md. Showing the guide does not verify setup.

The bundle is installed at `<destination>-roblox-tools`, outside MiniClaw's protected
installation directory. Its exact path is recorded in the installation receipt.
`miniclaw bundle status` and `bundle add` use the folder an installation already has
(`findBundleFolder` in `scripts/setup/roblox-bundle.mjs`): the recorded one; for a September
Mac installation, which recorded `<destination>/bundles/sep-roblox`, first its copy in
`~/Library/Application Support/MiniClaw Tools/sep-roblox` that its MCP servers use; the
default only when nothing is recorded. A complete bundle is kept as it is.
Failed installation removes the external bundle it created; existing folders are refused.

Provider discovery prefers the packaged `codex-cli/bin/codex` launcher in either
Codex.app or ChatGPT.app, then tries legacy app paths. Explicit CODEX_BIN overrides
still take precedence.
