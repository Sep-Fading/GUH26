# Worker transport

[worker.rs](../../server/src/worker.rs) connects the existing RPC client to an
authenticated byte stream. It opens no listener. The Linux controller and macOS
worker use it through [private VM channels](../vm-connection/README.md), started
by the installation wizard. See the [sequence](sequence.md).

## Ownership

`connect(stream, key)` authenticates the peer, then initializes App Server through
the existing request, notification and approval handler. From `connect`:

```rust
    authenticate(&mut stream, &key, false).await?;
    Client::attach(stream).await
```

`serve(stream, key, command)` authenticates the controller before invoking the
process factory. The caller supplies the executable and its policy locally; neither
comes from the stream. The worker owns that child and reaps it on disconnect.

The caller supplies a private stream and a per-worker 32-byte secret. This module
does not provision keys or enforce per-agent app and file permissions.
It is not suitable for an untrusted network: the handshake authenticates peers but
does not encrypt or authenticate subsequent traffic. The host relay is trusted.

## Handshake

The controller sends `MCWORK01` and a 32-byte challenge. The worker returns its own
32-byte challenge and an HMAC-SHA256 proof. After verifying that proof, the controller
returns its proof; the worker verifies it and acknowledges with byte `1`.

Each proof covers the protocol marker, sender role and both challenges. Roles are
`worker` and `controller`. Fresh challenges use two random UUIDs. The shared key is
never transmitted. RustCrypto verifies proofs without ordinary byte comparisons.
The complete exchange has a three-second deadline.

## Forwarding and shutdown

Each direction accepts JSON objects in lines of at most 8 MiB. Writes time out after
five seconds. Invalid or oversized frames close the connection. The worker logs
neither RPC payloads nor child stderr.

The installed guest uses `serve_with` with
[Policy](../../server/src/guest_computer.rs). It permits the RPC methods used for
initialization, read-only configuration, account sign-in, models, history, chats,
turns and Computer Use discovery. Unknown methods, command execution, configuration
writes, thread forks and plugin installation return a protocol error before
reaching App Server. Approval replies still use the existing response path.

Chat creation and resume accept only the current MiniClaw parameter fields.
Nonempty environment or capability roots, alternate sandboxes, and configuration
values that relax `chat-policy.json` are rejected. Turn input is text-only.
Only the guest-local Computer Use marker can configure the installed helper.
These checks do not restrict files opened inside an allowed native app.

## Tools the Mac answers

A guest chat's `dynamicTools` may hold the file and command tool, first and byte for byte, since it
is the one that acts in the VM, and then any of `miniclaw_project`, `miniclaw_ask` and
`miniclaw_mcp`, each once (`guest_computer.rs` `owned_tools`, `MAC_TOOLS`). Those three are checked
by name and shape: only `type`, `name`, `description` and `inputSchema`, `type` is `function`,
`inputSchema` is an object, 8 KB at most. Their words are not compared. They are advice that gets
reworded, and a guest that compared them would refuse every conversation until it was updated.
They grant the guest nothing: the worker relays the guest Codex's `item/tool/call` to the Mac
untouched, and the Mac's own handlers answer it under the Mac's checks. `miniclaw_delegate` and
every other name are refused, and the Mac offers delegation to Mac conversations only. A Computer
Use chat may carry the three but never the file tool.

## Protocol number and compatibility

`worker::PROTOCOL` (6) only drives the notice in Settings that a VM needs `miniclaw update-vm`
(`workerCurrent`). What the backend relies on is asked for by name: `miniclaw/worker/info` answers
`{ protocol, tools, features }`, and `Backend::machine_carries` offers a tool, or uses a feature
such as the terminal, for a VM conversation only when that VM's worker named it. The answer is kept per VM until its worker connects again. A worker
installed earlier names none, so its conversations keep the file tool alone and work as before;
a current worker with an older backend is simply offered less. The same rule as `frames.rs`: an
installed guest is never broken by updating the Mac.

[Client::transport](../../server/src/rpc.rs) retains the existing 128-pending-request
limit, approval handling and disconnect events. Local startup still owns its Codex
process; worker attachment starts no local child. Stop closes the stream. The worker
then kills and waits for its owned child. If the child exits first, the worker drains
its remaining output for at most five seconds.

## Verification

[Worker tests](../../server/tests/worker.rs) cover wrong keys, malformed handshakes,
reflected and replayed proofs, deadlines, oversized requests, reply ordering,
streaming, initialization cancellation, pending approvals and process cleanup.

[Guest policy tests](../../server/tests/guest_computer.rs) cover method and parameter
denial, retained text turns and denial through the authenticated stream. They do
not establish per-agent native app or file confinement.

[The opt-in runtime test](../../server/tests/worker_runtime.rs) uses the installed
Codex binary over a Unix socket pair with a disposable file-based credential home.
It verifies initialization, signed-out account/config reads and shutdown. It makes
no inference request and does not test a connection between running VMs.

The separate opt-in `tests/vm-chat.test.mjs` passed against the retained Linux and
macOS guests with real guest-owned OAuth and a streamed Astra reply. This verifies
the connected chat path, not guest-native computer control or cold-start recovery.
