# Runtime and startup

MiniClaw serves the Svelte UI, HTTP API, and SSE streams through one Axum listener.
In normal mode, Rust owns a Codex App Server connected over private stdio. Normal chat does not
require the Codex desktop UI. See the [startup sequence](sequence.md).

## Components

| Component | Responsibility |
| --- | --- |
| [scripts/start.mjs](../../scripts/start.mjs) | Builds assets and Rust, tracks the backend, forwards shutdown. |
| [main.rs](../../server/src/main.rs) | Opens private storage, acquires the instance lock, binds loopback, starts Codex. |
| [runtime.rs](../../server/src/runtime.rs) | Applies process-local restrictions and checks inherited provider/MCP settings. |
| [rpc.rs](../../server/src/rpc.rs) | Initializes JSON-RPC, correlates replies, routes notifications and approval requests. |
| [chat.rs](../../server/src/chat.rs) | Owns cached account/chat state and notifies SSE subscribers. |
| [api.rs](../../server/src/api.rs) | Validates requests, maps Codex methods, serves built assets. |
| [App.svelte](../../web/src/App.svelte) | Renders the conversation and controls using [Chat](../../web/src/chat.svelte.ts). |

The [Cargo workspace](../../Cargo.toml) contains `server`. Backend dependencies are
declared in [server/Cargo.toml](../../server/Cargo.toml); frontend/build dependencies
are in [package.json](../../package.json). The UI uses Svelte, TypeScript, Bits UI,
Tailwind CSS, and Lucide. Lockfiles record resolved versions.

## Launch modes

`npm start` builds `web/dist`, builds `miniclaw-server`, then starts that executable.
`npm run dev` also starts a Vite build watcher. It opens no Vite HTTP server or
hot-reload WebSocket; browser refresh loads rebuilt assets. Rust and configuration
changes require a restart. [vite.config.ts](../../web/vite.config.ts) rejects direct
Vite development and preview servers.

The launcher reads the executable path from Cargo's JSON output. Axum defaults to
assets relative to the compile-time Cargo manifest directory. `MINICLAW_WEB_DIR`
overrides that path for the installed package, whose launcher
([start-host.mjs](../../scripts/start-host.mjs)) points it at the packaged `web/` folder.
Normal `npm start` and `npm run dev` behavior is unchanged.

## Configuration

These defaults come from [main](../../server/src/main.rs).

| Variable | Default / use |
| --- | --- |
| `MINICLAW_PORT` | `8787`; accepts a `u16`. `0` asks the OS to allocate a port. |
| `CODEX_BIN` | `/Applications/Codex.app/Contents/Resources/codex` |
| `MINICLAW_DATA_DIR` | `$HOME/Library/Application Support/MiniClaw` |
| `MINICLAW_CHAT_DIR` | `chat-workspace` under the data directory; canonicalized before use. |
| `MINICLAW_WEB_DIR` | Optional absolute asset directory containing `index.html`; invalid values fail before storage or listener creation. |
| `MINICLAW_HELPER_BIN` | Optional native helper path; see [Computer Use](../computer-use/README.md). |

There is no configurable public bind address or Codex WebSocket URL. `main` binds
the listener with this exact statement:

```rust
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
```

The data and chat directories are set to mode `0700`. A held OS lock on
`instance.lock` prevents a second backend using the same data directory, even on
another port. Leave the file in place; process exit releases the lock. An occupied
port fails startup without selecting another port.

## State ownership

| Owner | State |
| --- | --- |
| Codex credential store | Existing ChatGPT OAuth credentials; MiniClaw does not read token files. |
| Codex history | Durable conversations. |
| `agents.sqlite3` | [Agent definitions and bindings](../agents/README.md), [skills and pinned copies](../skills/README.md). |
| `computer-recovery` | [ID-only recovery records](../computer-use/README.md) for interrupted switches. |
| Rust process | Account/usage cache, loaded chats, event replay, send receipts, pending approvals. |
| Browser memory | Drafts, selected settings, message view, pending controls. |
| Browser local storage | `miniclaw.activeTask`, the selected task ID. |

## Transport and shutdown

`Client::spawn` sends `initialize`, waits for success, then sends `initialized`.
Capabilities include `experimentalApi` for empty environments and
`mcpServerOpenaiFormElicitation` for native confirmation forms. Ordinary RPC calls
use numeric IDs; notifications and server-initiated requests are routed separately.

The transport limits a JSON line to 8 MiB, pending calls and outbound queue to 128,
and the notification broadcast to 1024 entries. Notifications also share a 32 MiB
raw JSON payload budget. RPC calls time out after 30 seconds; pipe writes after
5 seconds. Approval waits run outside the reader so cancellation can proceed.
Dropping a waiting call removes its pending reply entry.

`Events::publish` in [rpc.rs](../../server/src/rpc.rs) charges the raw UTF-8 JSON
length, including whitespace and escaping. It shares an immutable `Notification`
between the queue and receivers. An owned semaphore permit remains inside that
shared allocation until its last copy is dropped. Admission never waits for space:

```rust
        let permit = u32::try_from(json.len()).ok()
            .and_then(|bytes| self.budget.clone().try_acquire_many_owned(bytes).ok());
```

The backend listener keeps the envelope while decoding and applying a frame.
Generated approval-resolution notifications use the same budget. Payload exhaustion
terminates the connection; it does not discard a notification and continue as though
the transcript were complete. Dropped receivers and values release their permits.

The terminal decision is immutable. Once recorded, it blocks ordinary producers
and new RPC admission. The reader checks it before dispatching selected work while
keeping reads and writes fairly selectable. A write already issued can still have
an unknown outcome. Later orderly cleanup cannot relabel an observed failure.

Transport failure fails pending calls, emits one fixed-size disconnect notification,
and kills/reaps its owned child. That final notification alone bypasses the payload
budget. A separate completion watch retains the small terminal reason so a lagged
or late backend listener can still process failure. The listener clears approvals,
invalidates account state, and preserves uncertainty already set by a failed POST:

```rust
                            chat.uncertain |= chat.active.is_some() || chat.sending;
```

Startup/controller teardown waits for the listener's [failure notice](../notifications/README.md)
to finish before exiting. Receipts remain intact and no message is automatically
resent. The completion observer does not hold a transport-owning client reference.

The 32 MiB limit covers retained raw notification payload, not process memory.
Parsing trees, frame-decoder buffers, serialization temporaries, replies, outbound
requests, approval parameters and [transcript/snapshot storage](../history/README.md)
remain outside it. The reader parses frames for routing; the listener parses each
notification when applying it. Parsed trees are not retained in the broadcast queue.

The backend can still
serve the UI when initial Codex launch fails. It does not automatically replace a
failed runtime. The launcher's shutdown requests termination and allows five seconds
before forcing its child to exit.

`GET /api/health` reports `backend` and cached transport status `codex` without an RPC.
[Connection.svelte](../../web/src/Connection.svelte) checks only `backend` for its
badge, once on mount and on manual retry, with an eight-second timeout. A green
backend badge does not establish OAuth or model availability.

Coverage: [startup tests](../../tests/startup.test.mjs),
[single-listener tests](../../tests/local.test.mjs), and
[RPC tests](../../server/tests/rpc.rs). Notification bounds and lifecycle coverage:
[transport and HTTP tests](../../server/tests/rpc_budget.rs),
[byte accounting and reader fairness](../../server/src/rpc/tests.rs),
[backend lifetime tests](../../server/tests/backend_lifetime.rs), and
[worker transport tests](../../server/tests/worker.rs).
