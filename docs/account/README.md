# Account and usage

Codex owns ChatGPT OAuth and credential persistence. MiniClaw reuses that authorization
and offers Codex-managed sign-in when needed. It has no separate account, token store,
or logout route. See the [sign-in sequence](sequence.md).

## API

Routes and normalization live in [api.rs](../../server/src/api.rs).

| Route | Input | Result |
| --- | --- | --- |
| `GET /api/account` | None | `status`, `email`, `planType`, `requiresOpenaiAuth`. |
| `POST /api/auth/start` | `method`: `browser` or `device`; `provider: "claude"` for Claude Code | Attempt status and available auth URL/device fields. Claude adds `authUrl`, `codeEntry`, `notice`, or `error`. |
| `GET /api/auth/status` | None | Cached attempt or `status: "idle"`. |
| `POST /api/auth/cancel` | No fields consumed | Cancels the stored login ID, or ends the Claude CLI; returns `{}`. |
| `POST /api/auth/code` | `code`: the `code#state` value Claude's page shows | Hands it to the waiting `claude auth login`; 400 for another shape, 409 with no Claude sign-in pending. |
| `GET /api/usage` | `provider=codex` (default) or `claude`; optional `refresh=true`; that provider's sign-in | Normalized limits, freshness, and fetch time. |

`read_account` maps null account to `signed_out`, `type: "chatgpt"` to `signed_in`,
and other account types to `unsupported_auth`. It exposes a field allowlist rather
than returning the upstream account object. Transport errors are separate HTTP errors;
the frontend represents an unsuccessful refresh as `unavailable`.

## Claude sign-in

[`Bridge::login`](../../server/src/claude.rs) runs `claude auth login --claudeai` with piped
output and reads the page from its line `If the browser didn't open, visit: <url>`
([`claude_login.rs`](../../server/src/claude_login.rs) accepts only HTTPS pages on Claude's
sign-in hosts). `POST /api/auth/start` waits up to ten seconds for that line. The browser opens
Claude's page with the CLI's `localhost` callback and the CLI finishes by itself. On macOS the
CLI opens it with `open`, as before. On Windows the CLI's `BROWSER` is MiniClaw's own executable:
`miniclaw-server.exe <url>` (only with `MINICLAW_SIGN_IN_OPENER=1`) validates the address and
starts `miniclaw-server.exe --open-sign-in-url <url>` outside the sign-in job
(`CREATE_BREAKAWAY_FROM_JOB`; only that job allows it), which opens it with `ShellExecuteExW`, so
the browser is not ended with the CLI. The printed address is Claude's code page; it is the
page's fallback link, and the code it ends with is pasted into MiniClaw, which writes it to the
CLI's input. Exactly one browser opener runs on each platform. The URL, its query and the code
are never logged. A CLI that exits with an error fails the attempt with its last error line,
URLs shortened to scheme, host and path. While a sign-in waits, `claude auth status` runs at most
once per three seconds.

## OAuth lifecycle

`login` maps browser sign-in to `chatgpt` and device sign-in to `chatgptDeviceCode`.
The login mutex coalesces pending attempts. Starting sign-in while already signed in
returns 409. MiniClaw keeps Codex's `loginId` private and publishes its own `attemptId`.

Returned URLs must use HTTPS and one of `auth.openai.com`, `chatgpt.com`, or
`auth0.openai.com`. The browser reserves a popup during the click handler, removes
its opener, then navigates after receiving the URL. Device flow displays the returned
verification link and code. MiniClaw does not implement the OAuth callback or token
exchange itself.

[Backend::new](../../server/src/chat.rs) handles `account/login/completed`. It stores
completion results by login ID so an event arriving before the start reply is not
lost. Status becomes `succeeded` or `failed`. Cancellation calls
`account/login/cancel`; it does not sign out the shared Codex account.

## Account changes

`read_account` serializes reads and compares account email/status. `account/updated`
invalidates the account immediately. Both paths discard cached chats and usage when
identity changes, increment the generation, and request cancellation of active work.
Generation checks reject replies started under an earlier identity.

[Chat.mount](../../web/src/chat.svelte.ts) refreshes once, every 15 seconds while the
document is visible, and on focus. Confirmed sign-out or identity change clears UI
drafts, cached choices, selected ID, and its event subscription. Temporary failures
preserve drafts. Credentials and durable Codex history are not deleted.

## Usage normalization

[`usage::read`](../../server/src/api/usage.rs) authorizes the selected provider and calls
its `account/rateLimits/read`. Codex normalization prefers `rateLimitsByLimitId`, including
an explicitly empty map; only an absent/non-object map falls back to `rateLimits`.
Each bucket exposes `limitId`, `limitName`, and nullable `primary` and `secondary` windows. A window
contains `usedPercent`, `windowDurationMins`, and `resetsAt` in Unix seconds.

Successful reads return `status: "fresh"`, `fetchedAt`, and nullable
`ordinaryUsageAllowed`. Failure returns cached data as `stale`, or `unavailable`
with empty buckets when no prior reading exists. Missing values remain null.

Claude's bridge starts a separate, bounded CLI process in safe mode. It sends
`initialize` and `get_usage` control requests, closes stdin, and reads the matching
response. There is no model prompt, persisted session, credential extraction, or
transcript scan. The request in [`Bridge::usage`](../../server/src/claude.rs) is:

```rust
            for (id, request) in [("init", json!({ "subtype": "initialize" })),
                ("usage", json!({ "subtype": "get_usage", "skip_behaviors": true }))] {
                let frame = json!({ "type": "control_request", "request_id": id, "request": request });
                input.write_all(format!("{frame}\n").as_bytes()).await.ok()?;
            }
```

The CLI owns sign-in and network
access. The read has a ten-second timeout, a 256 KiB line bound and a 512 KiB total
output bound. It runs separately from the bridge's command loop so a slow limit read
does not delay Stop or a new turn.

Claude readings are cached for thirty seconds unless `refresh=true`. Its structured
`limits` list takes precedence over legacy window fields; model/surface limits and
enabled usage-credit limits get their own buckets. ISO reset times become Unix seconds.
The backend allowlists these display fields and keeps caches by provider. A changed
Claude account clears the Claude reading; an account generation change rejects an
in-flight read. The experimental `get_usage` shape was observed with installed CLI
2.1.280; unsupported or absent data follows the unavailable/stale path.

The exact send guard in [Chat.send](../../web/src/chat.svelte.ts) is:

```typescript
    if (this.usage.ordinaryUsageAllowed === false) return;
```

The frontend does not infer renewed access from a reset clock or a percentage. It
shows reset times in the browser's locale and stale data as `Out of date`.
[`Chat.usage`](../../web/src/chat.svelte.ts) selects the current conversation's provider,
so Codex's restriction cannot disable Claude Send. Claude's percentages do not imply
whether another request is permitted; its `ordinaryUsageAllowed` stays null. Each
provider still enforces its service-side limits.

[`UsageLimits.svelte`](../../web/src/UsageLimits.svelte) shows installed providers near
the top of Settings > Analytics, above historical token counts and even for empty ranges.
It includes disabled/sign-in/unavailable states, every returned window, a manual refresh,
and clamped visual bars without changing the reported percentages. The footer shares
the labels and reset formatting from [`usage.ts`](../../web/src/usage.ts).
Quota requests run independently of chat refresh so slow reads cannot delay recovery.

Coverage: [OAuth/conversation tests](../../tests/conversation.test.mjs),
[account/usage contracts](../../tests/contracts.test.mjs),
[frontend state tests](../../web/tests/chat.test.ts), and
[browser sign-in/usage tests](../../tests/browser/chat.spec.ts). Provider-specific quota
cases are in [usage.test.mjs](../../tests/usage.test.mjs) and
[usage.spec.ts](../../tests/browser/usage.spec.ts). The opt-in
[real usage test](../../tests/experiments/real-usage.test.mjs) observed fresh percentages
and reset times through both real providers without a model prompt.
