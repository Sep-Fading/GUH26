# Building MiniClaw for Windows

The step-by-step build and install guide is [README.md](../README.md) at the top of this repository, Part 2. The native
tests are described in [windows/README.md](windows/README.md), section "Native build and verification", and the Git
runtime pins in [windows/git-runtime.md](windows/git-runtime.md).

Release packages are made with `node scripts/package-release.mjs <output>` after `npm ci`, on Windows, from a clean
checkout of the release commit (`git archive <commit>`). The version comes from `server/Cargo.toml`. Each Windows
release publishes the Git corresponding-source archive beside its package (GPLv2); the GitHub release lists both with
their SHA-256 sums.
