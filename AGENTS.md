# MiniClaw Mini

A minimal MiniClaw for Linux, macOS and Windows. The full MiniClaw is in `../miniclaw-windows`;
code is ported from there, not reinvented, unless `PLAN.md` says otherwise.

Keep local research notes in ignored `codex-docs/`. Use `docs/` for developer documentation.

When creating or updating developer documentation, read and apply `/Users/sep/.codex/skills/developer-docs/SKILL.md` by default.

For every user-facing response in this project, including progress updates and code walkthroughs, apply `/Users/sep/.codex/skills/unslop/SKILL.md` (on Linux, the `unslop` skill). Read it at task start or after context loss if its contents are not already available.

Before choosing tools, dependencies, versions, or implementation approaches, research current official documentation on the web. Cite the source when explaining the choice and distinguish verified facts from recommendations.

At task start and after context compaction, read `MEMORY.md` to recover progress, user decisions, verification results, and the next unfinished work. Recheck mutable facts against the workspace.

Before implementing or reviewing, read `PLAN.md` for scope, decisions and acceptance criteria. Its status words distinguish planned behavior from verified implementation.

Before adding application code, read `TESTING.md` and the relevant acceptance tests. Write the failing test first. Update expectations only for an evidenced test defect or a requirement change, not to accommodate the implementation.

Before writing or reviewing implementation blocks, read `CODING_STYLE.md`. Explain code in small reviewable blocks and capture explicit user feedback there; distinguish confirmed preferences from provisional conventions.

Build the frontend with Svelte + TypeScript, Bits UI (`bits-ui`), Tailwind CSS, and Lucide (`@lucide/svelte`).

Reuse the user's existing Codex and Claude Code sign-ins. There is no MiniClaw account, credential home, or logout. Temporary provider homes are for tests, not the production default. Normal chat must work with the Codex desktop app closed: the backend launches and owns its App Server.

Every platform-specific piece sits behind one interface with a backend per OS. A feature is not done on an OS until it has been checked on that OS; say which OS a result came from.

After meaningful progress or changed decisions, and before handing work back, update `MEMORY.md` with completed work, actual check results, unresolved issues, and the next step. Keep detailed contracts in `PLAN.md` and `TESTING.md`; omit credentials and private chat content from memory.
