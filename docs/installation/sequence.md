# Installation sequence

Back to [installation](README.md).

## Release packaging

```mermaid
sequenceDiagram
    actor User
    participant Package as package-release.mjs
    participant Build as Backend and native compilers
    participant Node as Verified Node archive
    participant Tar as macOS tar
    User->>Package: miniclaw package FOLDER
    Package->>Package: Audit scripts, web and backend inputs for secrets and VM media
    Package->>Build: Compile backend, page and Mac helpers
    Build-->>Package: Prebuilt files
    Package->>Node: Verify checksum and extract to scratch
    Node-->>Package: bin/node and LICENSE
    Package->>Package: Copy runtime files and audit release tree
    Package->>Tar: Create archive without macOS metadata
    Tar-->>Package: .tar.gz
    Package->>Package: Audit raw tar headers for secrets and VM media
    Package-->>User: Release folder and archive, or error with cleanup
```

Package: [`package-release.mjs`](../../scripts/package-release.mjs) `main`,
`auditReleaseTree`, `extractNodeRuntime` and `auditReleaseArchive`.
The sequence was checked against the source; no Mermaid renderer is installed locally.

## Mac-only install

```mermaid
sequenceDiagram
    actor User
    participant Wizard as wizard.mjs
    participant Check as providers.mjs / system.mjs
    participant Plan as plan.mjs
    participant Install as install.mjs
    participant Build as package-backend.mjs

    User->>Wizard: miniclaw setup
    Wizard->>Check: inspect hardware, Cargo, Seatbelt, Codex, Claude
    Check-->>Wizard: facts (sign-in as yes or no)
    Wizard->>User: "This Mac" panel
    User->>Wizard: providers, port, VM = no
    Wizard->>Plan: planSetup(choice)
    Plan-->>Wizard: validated plan
    Wizard->>User: review
    User->>Wizard: Install
    Wizard->>Install: install(plan, approved)
    Install->>Build: cargo build, vite build, assemble
    Build-->>Install: runtime/
    Install->>Install: write data/providers.json and settings.json
    Install-->>Wizard: receipt
    Wizard->>User: next steps
```

## Install with the optional VM

```mermaid
sequenceDiagram
    actor User
    participant Wizard as wizard.mjs
    participant Install as install.mjs
    participant Vm as prepare-mac.mjs
    participant Connect as connect.mjs
    participant Session as vm/session.mjs
    participant Backend as miniclaw-server
    participant Guest as VM worker

    Wizard->>Install: install(plan with master)
    Install->>Install: download and verify image, Codex, Go
    Install->>Install: build the Mac runtime
    Install->>Vm: prepareMacVm
    Vm-->>Install: pair { mac, master, network }
    Install-->>Wizard: receipt, status awaiting-guest-setup
    Wizard->>Connect: connectSetup(pair)
    Connect->>Session: startPair(pair)
    Session-->>Connect: linksPath (nothing booted)
    Connect->>Backend: start with MINICLAW_VM_LINKS
    Connect->>Session: startMaster()
    User->>Guest: finish macOS setup, run Install Codex.command
    Guest->>Backend: worker attaches over its private socket
    Connect->>Backend: POST /api/vms/master/auth/start
    Backend->>Guest: account/login/start (device code)
    Backend-->>Connect: verification URL and code
    User->>Guest: signs in to Codex
    Connect->>Backend: GET /api/vms/master/status until signedIn
    Wizard->>Wizard: rewrite receipt with vmReady
```

## Optional Roblox bundle

```mermaid
sequenceDiagram
    actor User
    participant W as Setup wizard
    participant I as Installer
    participant B as Roblox bundle installer
    participant P as Python installer
    participant S as Backend profile store
    User->>W: Check Install Sep's Roblox Development Bundle?
    W->>User: Review contents, permissions and prerequisites
    User->>I: Approve installation
    I->>I: Build or copy runtime
    I->>B: installRobloxBundle
    B->>B: Verify and copy packaged assets
    B->>P: Create venv, install official wheel and SDK
    P-->>B: Import check passes
    B->>S: --initialize-data
    S-->>B: New migrated database, no listener
    B->>S: Transaction with remapped profiles and permissions
    B-->>I: Bundle receipt and remaining connection steps
    I-->>W: Installation complete
    W-->>User: Local credential and plugin setup instructions
```

Wizard: [`questions.mjs`](../../scripts/setup/questions.mjs) `collectChoices` and
[`wizard.mjs`](../../scripts/setup/wizard.mjs); installer:
[`install.mjs`](../../scripts/setup/install.mjs) `install`; bundle:
[`roblox-bundle.mjs`](../../scripts/setup/roblox-bundle.mjs) `installRobloxBundle`,
`prepareDependencies`, `seedDatabase`; backend:
[`main.rs`](../../server/src/main.rs) initialization mode and `Store::open` migrations.
This added diagram was checked against the call sequence but has not been rendered.
