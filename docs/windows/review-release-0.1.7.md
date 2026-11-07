# Independent review: release 0.1.7 candidate

Scope: `git diff 39d193e HEAD`. That is commit 79bbc2e, the chat ordering fix (`web/src/chat.svelte.ts`
`placeTurns`/`nameTurn`, `web/src/App.svelte`, `server/src/chat.rs` `name_turn`, `server/src/api.rs`), plus
62c6fdd (status docs). The release 0.1.6 follow-ups the caller listed were committed before the 39d193e merge,
so they are not in that diff. I reviewed them from their own commits: ca7eb95 (`downloadReleaseAsset`, Git
ZIP and source from the release), e0fe0aa (`package-release.mjs` uses `gitCorrespondingSource`), e7df45a
(uninstall removes `<installation>-roblox-tools`) and 34d9198 (README/doc moves). The review was read-only,
done by reading code, using the standards of `TESTING.md` (2026-10-10 section) and
`review-release-0.1.3.md`. No guest runs.

**Verdict: 0.1.7 may ship. Nothing has to be fixed first.** I found no case where an Activity list or run
card is duplicated or misplaced under the new rule. The backend naming is free of races in every path I
traced. The release-asset download and the uninstall folder removal both fail closed. There are two LOW
behaviour findings (F-1, F-2) and some test gaps (F-3, F-4). They are worth a follow-up but do not block.

Run by me: `npx vitest run --config web/vitest.config.ts web/tests/conversation-order.test.ts`: 5/5 passed.
I also ran two throw-away probe tests from the scratchpad (not added to the repo) against the same
`App.svelte`. They confirmed F-1 and F-2 (both passed, so the behaviour is as described below).

---

## Verdict per area

| Area | Verdict |
| --- | --- |
| Ordering rule (page) | Sound. One LOW visibility regression (F-1) and one LOW fallback weakness (F-2) |
| Backend naming (`name_turn`, `turn.started.messageId`) | Sound. No race; old records degrade safely |
| Codex / Claude adapters | Sound. Both store the user item under `clientUserMessageId` and page history by turn |
| `downloadReleaseAsset` / packager | Sound. Exact host and path allowlist, token only to the exact API asset URL, size and SHA-256 gate before use |
| Uninstall bundle-folder removal | Sound. The path comes from the destination, not from the record; links are refused; a forged record can only delete its own folder |
| Tests | Adequate for the reported bugs. Gaps in F-3/F-4 |
| Mac regressions in shared code | None found. The ordering code is shared and behaves the same; uninstall is Windows-only |

---

## Findings (most severe first)

### F-1 (LOW, confirmed by probe): a still-running background run is hidden when its parent turn is on an older page

`web/src/chat.svelte.ts:1714`: a group with no loaded message is `pending` only when
`turn === this.turn || !this.historyCursor`. A **background** delegation keeps running after its parent turn
has ended (`server/src/api.rs` delivers background results "with this message"). So `this.turn` is not that
run's turn.

Failure scenario: a background run is started in turn k. Later the conversation is reopened, after the cache
was evicted or the backend restarted, and turn k is now older than the first 50-turn history page
(`history.rs:158`, `"limit": 50`). The run card, with its live line and status, does not appear until the
user presses "Load older messages" enough times to reach turn k. In 0.1.6 it showed at the bottom. Approvals
from the child still surface through `delegatedRequests`, so control is not lost, only visibility. Probe:
`historyCursor: 'older'`, messages of turn-5 only, `delegations: [run('r1','turn-1',…,'running')]`. The
rendered order is `['user Fifth','assistant Fifth answer']`, with no card. It needs 50 or more later turns
while the run is still going, so it is unlikely.

Severity LOW, confidence high (behaviour) / low (likelihood). Fix: let `runPlacement` treat a group with any
`status === 'running'` run as pending (for example, pass a `live(group)` predicate into `placeTurns`), and
add one vitest case.

### F-2 (LOW, confirmed by probe): an unnamed turn with no reply disappears when it ends

The same line. If a turn's user message never gets a `turnId`, its group shows while the turn runs
(`turn === this.turn`). Once `turn.completed` clears `this.turn`, it is hidden if older history exists.
Before, it fell to the bottom. Probe: `turn.started` with `messageId: null`, then an activity, then
`turn.completed`, with no later naming snapshot. The order goes from `['user First','activity npm test']` to
`['user First']`.

In practice the backend names the message twice: on accept (`api.rs:4217`) and on `turn/started`
(`chat.rs:556`), and the post-send snapshot carries the name. So this needs the message to have left the
10,000-message bound, or an uncertain send where neither event was processed. Severity LOW, confidence high
(behaviour) / very low (likelihood). An optional fix is to keep a finished, unanchored group at the end when
it is newer than the oldest loaded message (for example by activity `startedAt`). It is probably not worth
the complexity. Document it as accepted instead.

### F-3 (LOW, test robustness): packaging tests silently read the developer's env

`gitBundleFiles({ archive = process.env.MINICLAW_GIT_ARCHIVE })` (`scripts/setup/windows-git-bundle.mjs:57`)
and `gitCorrespondingSource({ folder = process.env.MINICLAW_GIT_SOURCE })` (`:87`) use JS default parameters.
A test that passes `archive: undefined` / `folder: undefined` therefore gets the env value. This affects
`tests/setup-windows-git.test.mjs:142,146,149,182,185,189`, `tests/setup-release.test.mjs:219,335`. On a
packaging machine with those variables set, which is the documented release setup, these tests read the real
artifact instead of the fake fetcher and fail. The failures are false failures, not false passes. Fix: pass
`null`, treated as unset, or have the tests clear the two variables. Severity LOW.

### F-4 (LOW, test gaps)

- The `turn === this.turn` branch of `placeTurns` (a running turn whose message is not yet named, with older
  history) has no test. All five `conversation-order` tests name the message through `turn.started`. Probe 2
  exercised it, and it behaves as intended while running.
- There is no test that `name_turn(None, …)` leaves the newest message alone when it is an assistant message
  or a user message naming another turn (`chat.rs:462-470`). The filter is correct by reading; there is just
  no Rust unit test for it.
- No test covers stale `startedTurns` across a conversation switch. It is cleared at
  `chat.svelte.ts:1225,1349,1415`, and `send()` checks `selection !== this.selected` before `nameTurn`, so it
  is correct by reading.
- The "after reload" Playwright checks reload the page against the same backend cache. The provider-history
  path (`history.rs:245`, `turnId` on every reloaded message) is not exercised by these new tests. It is
  pre-existing and unchanged.
- Uninstall: no test that a `<installation>-roblox-tools` **link or junction** carrying a valid
  `installed.json` is kept (`uninstall-windows.mjs:68`). The code refuses it.

### F-5 (INFO): an externally started turn can name a failed send's message

`chat.rs:556` names the newest message when it is the user's and has no `turnId`. The only such message after
this change is one whose `turn/start` failed definitely (BAD_GATEWAY; the message stays in the cache). If a
turn is then started outside MiniClaw on the same thread (for example the Codex app; `tests/chat-cache.test.mjs`
"observed-turn"), that message gets the observed turn's id. The effect is cosmetic: that turn's cards sit
under the failed message. No action needed.

---

## Reviewed and sound

**Ordering rule.** A turn whose user message is missing but whose reply is loaded anchors `before` the first
reply. Two turns in flight cannot happen: `send` refuses while `active`/`sending` is set (`api.rs` busy
checks), and queue drains go through the same `send` (`api/queue.rs:100`). For a "Send now" turn, the
interrupted turn's message is named on accept and on `turn/started`; its late activity and run updates key
by `turnId` and stay put (tests 1-3). For reconnect merges, `merge()` keeps the newer copy. The server copy
carries `turnId` once named, and `nameTurn(null,'')` in `apply` re-applies what the stream said to a snapshot
written before the naming. It only fills a missing `turnId` and never overrides one. Old conversations: after
a restart the cache is rebuilt from provider history, where every message carries its turn
(`history.rs:189,245`). VM-offline transcripts saved by 0.1.6 may lack `turnId` on live user messages, but
those snapshots have no `historyCursor`, so no-reply turns fall to the end exactly as in 0.1.6. Delegation
cards after switching away and back come from the snapshot and anchor by the parent's messages. Message
ids: live user ids are random UUIDs or the queued id, and Claude stores the user item under
`clientUserMessageId` (`claude.rs:752`), so live and history ids agree. No duplicates: each turn group
lands in exactly one of before/after/pending. `runs.pending.flat()` keeps run ids unique for the keyed
`{#each}`. `nameTurn` per-conversation memory is cleared on every conversation change and is bounded by
the number of turns.

**Backend.** `name_turn` runs under the data lock. The user message is pushed (`api.rs:4112`) before
`turn/start` is called, so `turn/started` cannot arrive first. Claude acks before `turn/started`
(`claude.rs:330`); both orders converge. The filter only accepts `role == "user"` with a null or equal
`turnId`, so a message of another turn is never renamed. Chats are per thread, so a message id from another
thread cannot be matched. Notifications buffered while loading replay through `notify` against history
messages that already carry `turnId`, so `name_turn` is a no-op there. `turn.started.messageId` is
`null` when nothing was named, and the page checks `typeof === 'string'`. No other consumer of
`turn.started` exists (grep of `tests`, `server/src`, `web/src`, `scripts`). `attach()` (`api.rs:3932`) now
also sees live user messages with `turnId`, but it runs only on reload paths. Neither Codex nor Claude reads
the message `turnId`; it is MiniClaw-side state only.

**`downloadReleaseAsset` (`scripts/setup/download.mjs:84-139`).** Hostnames are compared exactly after URL
parsing, so case is normalised and a trailing-dot host fails. HTTPS only, no userinfo, port 443. Allowed
are the own-release download path and the exact API asset path (both without a query), plus the two GitHub
storage hosts under `/github-production-release-asset`. The repository allowlist has one entry. The token
is attached only when the current URL is `api.github.com`, and that URL can only be this asset's exact
endpoint. After `redirect: 'manual'` (Node/undici returns the 3xx with `Location`), every hop is
re-validated before any request, and the hop count is capped at 6. A response larger than the pin is
refused during streaming; size and SHA-256 must both match before the bytes are returned, written or
unpacked. Error messages carry repository, tag and HTTP status, never the token. This matches GitHub's
documented "Get a release asset" flow (`Accept: application/octet-stream` → 302 to storage). Setup's own
`source()` still approves no GitHub host (test at `setup-download.test.mjs:252`).
`gitCorrespondingSource` writes the verified archive with `wx` into the packager's fresh `mkdtemp` folder,
which is removed in `finally`. `gitSourceArtifact` then checks the manifest and files before publishing.

**Uninstall (`scripts/setup/uninstall-windows.mjs:64-72,93-128`).** The folder is derived from the resolved
destination (`cli.mjs:55-58` resolves and strips trailing separators), never from the record. `lstat` refuses
links; on Windows libuv reports junctions as symbolic links. The record must say `id: 'sep-roblox'`,
`createdFolder === true` and the same destination (case-insensitive on Windows). Otherwise the folder is
kept and named in the plan and the output. Can a sandboxed process forge `installed.json`? Sandboxed bundle
servers may write only their own server folder under the bundle (`roblox-bundle.mjs:90`), not the bundle
root. Even a forged record could only make `--delete-data`, which the user confirms twice and whose plan
names the folder, delete that same folder. `fs.rm` does not follow links met inside the tree. Removal
happens only after the destination itself was removed, so a failure keeps the bundle folder.

**Docs.** No broken relative links in `README.md`, `docs/PROJECT-README.md`, `docs/windows/README.md`,
`docs/BUILD.md` (checked by script). No test or script depends on the old README content.

## Not reached

No Windows guest runs. Not rerun: `cargo test --locked`, the full `test:web`, `npm run check`,
`test:conversation`, Playwright, the Windows cross-check. I relied on the counts in `TESTING.md`
(2026-10-10). I did not check the real GitHub release assets (ids 616891706/616891738/616891793) against
their pins. I did not test whether `tar.exe` on Windows refuses `..` members; this is covered only by the
pinned digest of a MiniClaw-built archive.
