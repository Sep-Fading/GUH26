# Coding style notebook

Record the user's preferences here as we review MiniClaw. This is evidence for a future skill, not a finished skill or a description of the assistant's default style.

## Confirmed preferences

| Preference | Evidence | How to apply it |
| --- | --- | --- |
| Keep documentation concise and indexes minimal. | User, 2026-09-12: remove redundant commentary such as "Includes sequence diagrams" from the docs index. | Use capability links in the index. Omit self-description, repeated headings, and phrases that add no technical information. |
| Keep test names short and descriptive. | User flagged `get_health_reports_the_backend_is_running` as too long; shortened to `health_returns_ok`. | Name the behavior concisely. The replacement wording is the assistant's proposal, not a user-approved naming convention. |
| Wrap long expressions for readability. | User flagged the JSON-parsing assignment on line 26 of `server/tests/health.rs` as too long. | Split long assignments or call chains across lines. A numeric line-length limit has not been agreed. |
| Research implementation choices on the web. | User, 2026-09-11: "always research your choices on web". | Consult current official documentation before choosing tools, versions, dependencies, or implementation approaches. Explain the evidence and cite it during review. |
| Use Unslop for all project responses. | User, 2026-09-11: "use this skill in all of your responses in this project". | Apply the Unslop skill to explanations, progress updates, and final responses. The project-wide instruction is in `AGENTS.md`. |
| Write tests before implementation. | User, 2026-09-11: "write the test without implementation" to avoid tests tailored to the code. | Derive assertions from intended behavior. Show the test before the implementation and report the actual failure. |
| Explain implementation code block by code block. | User, 2026-09-11: "actively talk me through implementation codeblock by code block so its reviewable by me". | Show each small, coherent block with its purpose, decisions, and relevant test result. Leave room for feedback before moving ahead. |
| Explain the architecture as features are added. | User, 2026-09-11, on restoring sessions: "I want to understand our application's architecture along the way". | Explain which layer owns each responsibility and where state lives before reviewing its implementation. |
| Summarize every function and method. | User, 2026-09-11, citing api.rs: "what they handle and do and return" with a "one or two liner very minimal explanation on top of every method/function". | Put a one- or two-line, high-level summary above every function and method, including private helpers and getters. Explain its purpose and return value or state change in plain language. Apply Unslop. |
| Explain package imports in 5-7 words. | User, 2026-09-11, citing axum: "a 5-7 word comment explaining why it was imported/what its used for". | Put a short purpose comment above the import or same-package import group. Explain its use in that file, not a generic package description. Apply Unslop and check the word count. |
| Keep comments inside functions sparse. | User's earlier request for "very minimal useful comments" remains compatible with the explicit summaries above every function. | Comment a block's purpose, state ownership, or non-obvious constraint when useful; avoid repeating the function summary or narrating individual statements. |
| Preserve coding preferences for a future skill. | User, 2026-09-11: "note down my coding style into a separate md to turn into a skill later on". | Update this notebook from explicit feedback. Keep observations separate until confirmed. |
| Start with the application foundation before internal mechanisms. | User questioned starting with event replay, then accepted the sequence of project structure, health test, minimal Rust server, and Svelte connection state. | Begin with the smallest working browser-to-backend connection. Introduce streaming internals when that behavior is being built. |
| Ship the capability with focused tests and little scaffolding. | User, 2026-09-14, on the guest desktop: "Do not over engineer useless scaffolding and a billion tests, ship the feature and write the code that delivers the capability." | Add the smallest useful test for the behavior, then build the actual route and UI. Avoid broad fixture frameworks for a single feature. |

## Working review cadence

Proposed by the assistant, awaiting feedback: explain the test, review a small implementation block, run its checks, then pause before the next block. Do not build the rest of the application silently while a block is under review.

## Not yet established

Naming rules beyond concise test names, function size, abstraction preferences, error-handling style, and formatting rules beyond wrapping long expressions have not been expressed by the user. Existing assistant-written code is not evidence of the user's taste. Use language tooling and project conventions provisionally.

## Project constraints, not universal style

User, 2026-09-22: hovering over a truncated conversation or project name in the left
sidebar should scroll it so the full name can be read. This is a MiniClaw interaction
requirement, not a general rule for every truncated label.

User, 2026-09-14: this is first-version development. Do not add code solely for
backward compatibility; existing development chats are disposable when cleanup is
needed. This does not remove protections for active work or duplicate sends.

User, 2026-09-14: the current guest-desktop item is view-only. It needs live
frames, Shutdown, and an expand button to reveal the same VM in a native window.
Keyboard/mouse control, Pause, and two-agent desktop coordination are deferred.
User, 2026-09-14: a one-frame-per-second screenshot loop does not meet the live
desktop expectation. Stream continuously at a useful frame rate.

User, 2026-09-13: Master VM dependency installation and sign-in belong inside the
same `miniclaw setup` wizard. Do not require exiting into a separate setup command.

User, 2026-09-13: the installation wizard initially targets Apple silicon Macs.
Show recommended options at each step and size VM recommendations from actual host
resources. Small hosts need compact profiles, subject to image minimums and combined
resource limits. This is a MiniClaw setup requirement, not a universal sizing rule.

User, 2026-09-12: skills may belong to multiple groups. Group activation and member
switches are local to that group; changes apply from the next message without
interrupting the current response. Independent selections must remain intact.

User, 2026-09-12: distinguish prompts with right-aligned dark bubbles and white text,
following the supplied chat reference. Assistant replies stay unboxed on the left.

User, 2026-09-12: chat navigation should use thin marks that expand near the hovered
mark, with a bold prompt excerpt above a Markdown response preview. This is a
MiniClaw interaction requirement based on the supplied Codex screenshot.
Follow-up: shrink the rail and marks, but leave the preview size unchanged.

User, 2026-09-12: agent and skill form delete buttons should resemble Save in size,
with a leading trash icon, "Delete" label, white fill, red border, and red text/icon.
On hover, enabled buttons use red fill and white text/icon; the border stays red.
Keep chat-row deletion compact. This is a MiniClaw control style requirement.

User, 2026-09-12: instruction editing must be one live-formatted surface, without
Edit/Preview tabs. Markdown typed into the field should render in place. This is
a MiniClaw interaction requirement, not a universal editor preference.

MiniClaw uses Svelte, TypeScript, Rust, Bits UI, Tailwind CSS, and Lucide. It reuses local Codex OAuth with no separate MiniClaw account. Keep these in the project plan; do not turn them into blanket rules for unrelated projects.

User, 2026-09-11: one interface, without multiple local servers or publicly exposed
ports. Keep normal and development startup on one loopback UI/API address. This is
a MiniClaw deployment constraint, not a general preference about every application.

## Future skill

User, 2026-09-12: keep MiniClaw's model and reasoning selectors compact and left-aligned.
The composer starts at one line, grows/shrinks with input, and scrolls beyond four lines.
This is a confirmed MiniClaw layout requirement, not a universal component rule.

User, 2026-09-12: Computer Use belongs beside reasoning effort as a toggle in the
same chat, with a pressed appearance and bluish active icon. Do not offer a separate
computer-chat creation control. This is a MiniClaw interaction requirement.

User, 2026-09-12: permission switching must not visibly reload the frontend. Keep
the conversation, draft, and scroll position; report failed steps inline with retry.
The user approved archive/unarchive behind that control after runtime verification.

When the user asks to create the skill, review this notebook together, resolve provisional observations, and extract only confirmed reusable preferences. Keep task progress in `MEMORY.md`.

User, 2026-09-22: conversation search belongs directly below New chat in the left
sidebar, not in the top bar. Use the same placement in the mobile navigation drawer.

User, 2026-09-22: label the sidebar conversation finder "Search".

User, 2026-09-22: provider model lists must be dynamic for every onboarded provider. Reuse
the existing refresh control in Settings > Providers; do not add another refresh button.

User, 2026-09-22: the left sidebar should be resizable.

User, 2026-09-22: do not combine unrelated tool sets in one capability. Name Web fetch,
Subagents and Task tracking separately, with independent switches.
