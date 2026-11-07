# Test architecture

API and browser tests run the real Rust backend against a scripted Codex peer.
Separate compatibility probes run the installed Codex binary against fake inference.
See the [fixture sequence](sequence.md).

## Test layers

Commands are defined in [package.json](../../package.json); Rust tests are under
[server/tests](../../server/tests).

| Command | Coverage |
| --- | --- |
| `cargo test` | Health, RPC lifecycle/correlation, native-helper process arrangement, agent storage. |
| `npm test` | Harness, startup, local listener, frontend state, models, agents, conversations/history, computer, contracts, browser suites. |
| `npm run test:mcp` | MCP server list and import, the client against fake stdio and HTTP servers, the gateway tool with both providers, slash commands. Included in `npm test`. |
| `npm run test:agents` | Agents, permissions, skills, projects, skill groups, notifications, creation recovery, the main agent and the two capability levels. |
| `npm run test:conversation` | Conversations, history, the conversation cache, replay bounds and the message queue. |
| `npm run test:web` | Vitest with jsdom and Svelte Testing Library. |
| `npm run test:ui` | Playwright desktop and mobile Chromium. |
| `npm run test:vm` | VM routes, native relay, readiness deadlines and unexpected exits. |
| `npm run test:setup` | Wizard choices, installation, downloads, guest authentication and cleanup. |
| `npm run test:vm-runtime` | Explicit headless Linux guest: two boots, 126-second deadline, assets and listener. |
| `npm run test:vm-e2e` | Explicit installed pair: real browser send, SSE delivery and restart persistence. |
| `npm run check` | Svelte and application TypeScript diagnostics. |
| `npm run test:check` | Test TypeScript diagnostics. |
| `npm run build` | Production frontend build. |
| `npm run test:agent-runtime` | Real Codex against fake inference: what the model is given, and that it is offered exactly MiniClaw's tools. Opt-in. On a Mac with only ChatGPT.app set `CODEX_BIN=/Applications/ChatGPT.app/Contents/Resources/codex`. |
| `MINICLAW_RUNTIME_PROBE=1 node --test tests/experiments/skill-context-cost.test.mjs` | Real Codex against fake inference: how many copies of repeated skill text the model receives. Opt-in, same `CODEX_BIN` note. |
| `npm run test:computer-runtime` | Real Codex inventory, fake-helper execution/revocation, restart and toggle behavior. |
| `npm run test:capabilities` | Older raw-tool-name diagnostic, with wrapper-name assumptions described in [TESTING.md](../../TESTING.md). |

`npm test` does not include Cargo tests, static checking, or the installed-runtime
probes. Build the backend with `cargo build` and assets with `npm run build` before
running an individual API/browser suite. The harness checks that both exist; it does
not compile them automatically. Full-suite local-launcher tests also exercise builds.

## Scripted peer

[launchApp](../../tests/support/harness.mjs) creates a temporary data directory,
chat workspace, Codex home, and fixture executable. It selects a temporary loopback
port and overrides the child environment. This exact test-harness fragment directs
the fixture to disposable storage:

```javascript
        MINICLAW_DATA_DIR: peer.dir,
        CODEX_HOME: peer.codexHome, MINICLAW_CHAT_DIR: peer.cwd },
```

[fake-codex.mjs](../../tests/support/fake-codex.mjs) speaks JSON-RPC over stdio and
accepts test controls through a local Unix socket. Tests can inspect requests, replace
method handlers, emit notifications, and hold/release replies. Unscripted methods
fail rather than returning success. [fixtures.mjs](../../tests/support/fixtures.mjs)
provides synthetic accounts, model choices, and task/turn payloads.

`app.restart()` reuses the same fixture home and workspace. `app.close()` stops the
owned processes and removes only disposable fixture data. The SSE reader handles
chunk boundaries and UTF-8 while collecting parsed events. Temporary test listeners
are distinct from the application's one persistent listener.

## Browser checks

[playwright.config.ts](../../playwright.config.ts) uses one worker, no retries,
retained failure traces, and failure screenshots. Desktop is 1440 by 900; mobile
is 390 by 844 with touch/mobile emulation. Individual layout tests also check 320px.
This is Chromium coverage, not Safari, a physical phone, or a screen-reader audit.

[web/vitest.config.ts](../../web/vitest.config.ts) discovers `web/tests/**/*.test.ts`
under jsdom. Unit tests control fetch and event responses to exercise state races;
browser tests verify actual controls, focus behavior, streaming text, and layout.

## Installed-runtime probes

[agent-runtime.test.mjs](../../tests/agent-runtime.test.mjs) captures the backend's
actual agent start/resume parameters, uses disposable Codex homes, and serves fake
Responses events over loopback. It checks executable tools before and after process
restart. The dynamic-tool case returns a synthetic receipt; no worker is executed.

[computer-runtime.test.mjs](../../tests/computer-runtime.test.mjs) requires an
explicit `MINICLAW_HELPER_BIN`. Its revocation checks use a fake helper and independent
call journal, not native clicks. Phase-specific call IDs distinguish pre-restart
and resumed inference. Neither probe requires paid inference or changes shared OAuth.

The latest recorded results and test-first failures are in [TESTING.md](../../TESTING.md).
Native grant lifetime and device-wide isolation are not established by fake peers
or the runtime compatibility probes.

## Paired VM acceptance

[vm-e2e.test.mjs](../../tests/vm-e2e.test.mjs) requires `MINICLAW_E2E_INSTALL` to name
a completed test installation. Missing configuration fails; it is not a skipped
pass. Port `8787` is refused to protect the current development app.

By default it runs the actual `miniclaw start --installation` path twice. Each boot
must connect within 126 seconds, including any manual Master unlock time. It checks
the owned process tree has exactly one loopback TCP listener and rejects a hostile
Origin. Playwright selects Astra with low effort, sends a fixed no-tools prompt,
checks delivered SSE events and verifies the stored reply. The second boot reopens
that conversation without sending again.

`MINICLAW_E2E_RUNNING=1` checks an already-running test installation without owning
its lifecycle. This mode does not verify boot time, process-tree listeners or restart
persistence. Neither mode installs macOS or completes native account setup.
Real replies consume the guest account's Codex allowance.

The test keeps screenshots and `report.json` under a private `e2e-*` directory in
the installation. Reports identify stages, durations and failure types without
storing RPC or OAuth payloads. Browser exceptions, console errors, failed requests,
HTTP errors and SSE errors fail the check. Shutdown requests retain VM disks and
wait for native exit. Supervisors force power-off after 90 seconds if needed;
exit code `2` is accepted only with the forced-stop warning. Reports record each
shutdown's code, elapsed time and `forced` flag. Other nonzero exits fail. Forced
shutdown is never reported as clean, and restart must still restore the conversation.

On 2026-09-13 both attached and installed two-boot modes passed against the fresh
wizard installation. Installed readiness took 40,045 ms and 82,404 ms including
manual unlock. The second boot reused guest OAuth and restored the conversation.
These results do not cover runtime crash recovery or guest-native computer control.

`vm-backend.test.mjs`, which produced those reports, was removed with the Linux
controller. It accepted `MINICLAW_VM_REPORT` as a new report-path prefix and recorded boot
timings and private console logs after shutdown. Use a fresh guest to measure provisioning; a retained guest measures
restart. Existing report files are not overwritten.
