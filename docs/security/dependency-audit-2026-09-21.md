# Dependency audit, 2026-09-21

Run by hand; nothing here runs automatically.

| Tool | Version | Scope | Result |
|---|---|---|---|
| `npm audit` | npm bundled with the project's Node | `package-lock.json` | 0 vulnerabilities |
| `cargo audit` | 0.22.2 (RustSec), installed with `cargo install cargo-audit --locked` | `Cargo.lock`, 137 crates | 0 vulnerabilities |
| `govulncheck` | `golang.org/x/vuln` 1.8.0, run with the pinned Go 1.27.1 from the installation | `scripts/vm/network-helper` | 0 vulnerabilities in code the helper calls |

## The two Go advisories

`govulncheck` reports two advisories in modules the helper requires but whose affected code
it never calls (symbol-level analysis):

| Advisory | Module | Found | Fixed |
|---|---|---|---|
| GO-2026-6237 | `github.com/insomniacslk/dhcp` (indirect, through gvisor-tap-vsock) | `v0.0.0-20240710054256-ddd8a41251c9` | `v0.0.0-20260719225207-c76316d4aa82` |
| GO-2026-5932 | `golang.org/x/crypto` | `v0.56.0` | none yet |

Neither was changed. The helper's module set is pinned and built during setup, the first has
no release that gvisor-tap-vsock 0.8.9 asks for, and the second has no fix. **Recommendation:**
raise `insomniacslk/dhcp` to the fixed pseudo-version when gvisor-tap-vsock is next updated,
and rerun `govulncheck` then. This is a decision for the user, not done here.

## Updates applied

Patch-level only, inside the ranges `package.json` already allowed: `@codemirror/commands`
6.11.1, `@codemirror/state` 6.7.5, `@codemirror/view` 6.43.12, `@types/node` 22.20.4, `svelte`
5.57.1, `vitest` 5.0.1. After them: type check clean, unit 78, browser 225, build succeeds.

## Left for the user to decide

Major versions are available and were not taken: `typescript` 7, `jsdom` 29,
`@testing-library/jest-dom` 7, `@types/node` 26. No Rust dependency was changed.

## Tool inventory for this audit

Node and npm (project), Rust with cargo and `cargo-audit` 0.22.2, Go 1.27.1 at
`<installation>/network-tools/go`, `govulncheck` run with `go run` and not installed.
`git` is the system one; MiniClaw itself only ever runs it inside the sandbox
([workspace panel](../workspace-panel/README.md)).
