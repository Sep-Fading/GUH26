//! Test-only stand-in for the installed Windows Computer Use helper. Not part of any package.
//!
//! The computer suites configure a helper but never drive it: fake Codex answers for Computer
//! Use, and the backend only checks that a helper is configured. The Mac suites name
//! `/usr/bin/true` for that. Windows accepts only an absolute path to a file named like the
//! installed helper (`codex-computer-use.exe` or `codex-computer-use-arm64.exe`), so the suites
//! copy this program under that name. Like `/usr/bin/true` it does nothing and exits at once:
//! if a suite ever started it, the backend would see a helper that ended without answering, and
//! no test could pass by talking to it.

/// Ends at once with success, reading and writing nothing.
fn main() {}
