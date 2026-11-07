# Windows computer use

MiniClaw now has a testable MCP adapter for the installed Windows Computer Use helper.
Selecting and verifying a helper in setup saves the user's consent together with the Codex
app's package family and the helper's file name, not the versioned package folder. The backend
finds the helper again at startup and before each use; a resolved, OpenAI-signed helper enables
local Computer Use availability; user capability permissions and the
adapter's per-action approvals still control execution. There is no separate native-test
switch that disables this configured capability. Native desktop acceptance has not yet
passed, so availability must not be described as verified GUI compatibility. The macOS
bridge remains unchanged.

## Integration evidence

[OpenAI's Computer Use guide](https://learn.chatgpt.com/docs/computer-use) describes
foreground Windows operation and app approvals. It does not publish a standalone
third-party helper protocol. The adapter protocol was instead checked against the
installed desktop runtime's `@oai/cua` Windows client on 2026-09-30. Its transport
starts `codex-computer-use.exe` or `codex-computer-use-arm64.exe` with the actual parent
PID, sends newline-delimited requests, and handles native `approvalRequest` replies.
This observed integration contract may change with desktop updates.

[The installer verifier](../../scripts/setup/windows-computer.mjs) requires an
OpenAI-signed Authenticode executable selected by the user. It never copies the helper.
A helper inside the Codex app package (`C:\Program Files\WindowsApps\OpenAI.Codex_<version>_<arch>__<publisher>\...`)
is saved as `{ enabled, packageFamily, file }` in `settings.json`; [start-host.mjs](../../scripts/start-host.mjs)
migrates a versioned path saved by 0.1.4 and earlier on load and passes `MINICLAW_HELPER_PACKAGE` and
`MINICLAW_HELPER_FILE` to the backend. A helper outside a package keeps its fixed path. PowerShell runs the fixed signature-check script with a
process-local execution policy; machine policy is not changed. Microsoft documents
that scope in [PowerShell's command-line parameters](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1).

[computer::target](../../server/src/computer.rs) uses an explicit `MINICLAW_HELPER_BIN` path
on Windows when one is set (tests and fixed paths); otherwise the package family and file name.
[windows_helper](../../server/src/computer/windows_helper.rs) lists the family's packages for the
current user with [GetPackagesByPackageFamily](https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getpackagesbypackagefamily)
and their folders with [GetPackagePathByFullName](https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getpackagepathbyfullname)
(both in kernel32), takes the newest version that holds the file in the `@oai\cua` or `@oai\sky`
folder (the rule of [setup's detection](../../scripts/setup/detect.mjs)), and checks its signature with the
same Authenticode test as setup (Windows PowerShell `Get-AuthenticodeSignature`, signer organization
OpenAI); the check is reused while the same file is unchanged. `GET /api/computer` reports the
reason when no helper resolves, and the Tools menu shows it. While a package helper is configured,
`%ProgramFiles%\WindowsApps` is a protected root for tool grants. Availability does not assert
native authentication or GUI compatibility. When `CODEX_BIN` names an
absolute executable, the adapter forwards it as `CODEX_CLI_PATH` so the helper uses
the configured CLI for its own authentication. It never passes an authentication token. [main](../../server/src/main.rs)
routes `--computer-helper` on Windows to [windows_bridge::run](../../server/src/computer/windows_bridge.rs).

The old `--computer-filter` subcommand belongs to the Unix transport and explicitly refuses Windows use. It is not the Windows adapter or its readiness check.

## Approval and execution boundary

The adapter exposes a fixed set of app/window methods, with no script or shell API.
Each action requires a fresh MiniClaw approval showing its method and exact bounded
inputs. Denial, expiry, disconnect, or a native failure prevents further execution of
that request. A native app approval is relayed through another real user prompt before
the adapter constructs approval metadata; model arguments cannot supply that metadata.

Before a window action, the adapter resolves its HWND through native `get_window`
and checks the returned app identity. Launching requires an app ID previously returned
by native inventory. Protected app results are omitted from inventory. Screenshot
results must contain image data; arbitrary returned file paths and URLs are not read.
The helper process uses explicit Windows handle inheritance and Job containment.

The request validator rejects oversized or undeclared fields before prompting. This
fragment comes from `request` in the adapter:

```rust
    if params.to_string().len() > 2048 { return Err(invalid()); }
    let object = params.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) { return Err(invalid()); }
```

Desktop control uses the user's normal app access, outside the shell sandbox. A
protected-app list restricts direct targets; it is not a claim that an approved app
cannot affect files, network, or other apps. See the [request sequence](computer-use-sequence.md).

## Verification

Host tests exercise actual MCP framing, denial, expiry, native approval replay, and
spoofed app/HWND rejection. Windows target compilation includes the adapter. The
installer test covers signer rejection; its native signature-check test is skipped
outside Windows.

Native acceptance still requires the installed helper, caller authentication,
foreground screenshot/input, cancellation and physical Escape. In particular, the
native helper must independently enforce HWND/app ownership at action time, including
saved app allow-list cases; a separate metadata preflight cannot prove that race safe.
No native screen action is reported as verified yet.

## Native results, 2026-10-04 (signed-in guest)

Run on the Windows 11 ARM64 guest as interactive user S after the Codex app sign-in, with
the debug ARM64 backend built in the guest at 16:18 UTC (SHA-256 `C23240B3…6196E26`) from
the working tree after checkpoint `ace5621`, and Codex package 26.930.3930.0. Logs are in
`work/windows-vm/` outside the repository.

| Check | Result | Log |
| --- | --- | --- |
| Helper signature, `account/read`, tool discovery, unattended denial | Passed: valid signature, signed in (type `chatgpt`), 14 tools, inventory refused without approval | `computer-readiness-signedin.log` |
| Helper caller authentication and session through `--computer-helper` | Passed: the installed helper accepted the adapter as its caller | `computer-inventory-green.log` |
| `list_apps` and `list_windows` after MiniClaw's own approval | Passed: 39 apps and 1 window returned, adapter exit 0. The test harness accepted only the exact inventory approval text | `computer-inventory-green.log` |
| Launch, screenshot, input, cancellation, physical Escape | Not run. They need a person at the console | `computer-use-manual-checklist.md` |
| Model-driven Computer Use turn through MiniClaw's approval API | Not run. The account has no Codex usage left | `computer-codex-direct-red.log` |

Inventory is not a screen action, so the statement above still holds: no native screen
action is verified. `computer-console.mjs` is a prepared console runner in which a person
approves each action; it has not been executed.

The live chat attempt reached the provider through MiniClaw's App Server integration
(provider `signed_in`, 10 models, thread and turn accepted) and the turn then failed. A
direct App Server turn without MiniClaw failed the same way with `usageLimitExceeded`, so
the failure is the account's usage limit. MiniClaw showed only "Codex could not complete
this response."; the provider's reason did not reach the chat.
