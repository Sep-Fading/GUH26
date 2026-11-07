# Testing

Tests come first: write the test, see it fail for the expected reason, then implement. A failing
test is fixed or explained, never deleted. Record each result with the OS it ran on.

- Rust unit tests sit beside the code; route tests in `server/tests/`.
- Provider tests use fake Codex App Server and Claude CLI peers, with temporary provider homes.
- Sandbox tests run real commands under the real backend for that OS. On an OS without the
  backend's requirements they are reported as skipped, never as passed.

## Acceptance, milestone 0

- `GET /api/health` returns 200 with `{"ok":true}`.
- The backend binds `127.0.0.1` only.
- The page shows "Connected" when the backend answers, and "Not connected" when it does not.

## Acceptance, milestone 2 (Linux sandbox)

Each of these runs a real command through the Linux backend:

- A command cannot read `~/.codex/auth.json`, `~/.claude`, or `~/.claude.json`, including when the
  user granted `$HOME` itself.
- A command cannot read a file outside its grants, and cannot write to a read grant.
- A command can write inside a write grant, and its output comes back.
- A command has no network: connecting to a public address fails.
- With hosts approved, `CONNECT host:443` to an approved host works through the proxy; any other
  host, any other port and any private address are refused and recorded.
- A command still running when its parent ends is killed.
- With `bwrap` missing or user namespaces unavailable, the tool refuses with the reason and runs
  nothing.

## Results on Linux (2026-11-07, Arch Linux, kernel 7.2.14, x86_64)

- `cargo test --test seatbelt`: 13/13, run against the real bubblewrap. Five cases were added
  for Linux: the session bus and keyrings, installs and administrator changes, a timed-out command
  leaving nothing running, plus the unavailable-bubblewrap and mask-order unit tests.
  `MINICLAW_REAL_NETWORK=1` real HTTPS case: passed.
- `cargo test --no-fail-fast`: 420+ pass. The only failures are 4 VM transfer and backup tests
  (`backups`, `transfer_jobs`), VM features that are out of scope.
- Web: vitest 143/143, svelte-check 0 errors.
- Node suites: host-tools, mcp-*, skills, agents, permissions, delegation, claude, providers,
  conversation, history, briefing, main-agent, capability-toggles, questions, local, startup and
  contracts all pass. host-tools also passes 39/39 with bash forced as the shell.
- `MINICLAW_NATIVE_MODEL_PROBE=1 tests/experiments/real-e2e.test.mjs`, in-scope cases, with real
  models: 13/13.

Test changes, with the evidence for each:

- **Linux added as a platform (requirement change):** health and contracts accept `linux`.
  `platform.test.ts` no longer expects `linux` to be ignored.
- **Linux refusal text:** a path outside the grants is absent from the sandbox's view, so the
  expected error is "No such file or directory", ENOENT or ECONNREFUSED instead of macOS's EPERM.
- **Listening ports on Linux:** an undeclared listener may bind inside the server's own network,
  so the test asserts instead that the machine cannot reach it.
- **Web test setup:** jsdom puts the host OS in its user agent, so `web/tests/setup.ts` pins
  `macos` before each test.
- **`real-e2e` cases older than the current behaviour:**
  - `wc` is on the harmless list (`harmless.rs`), so it runs unasked; the case now uses `sed`.
  - Delegated requests arrive in `delegatedRequests` (since 2026-09-27).
  - The idle lead carries on with a background result by itself (`delegation::carry_on`).
- **Mac-only details:** the `/private/tmp` cwd became the temp folder; `lsof` has an `ss`
  fallback.
- **Removed:** `web/tests/setup-guides.test.ts`, which tested only the removed Roblox guides.
