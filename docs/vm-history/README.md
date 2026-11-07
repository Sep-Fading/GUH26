# VM chat history on the Mac

A VM chat's history lives in that VM's Codex. The backend keeps a private display copy on
the Mac, so the chat can be read while its VM is off. The VM stays the source of truth: the
copy is only shown while the VM is unavailable and is replaced by the real task once the VM
is ready.

See the [history sequence](sequence.md).

## Storage

[transcripts.rs](../../server/src/transcripts.rs) writes one file per chat to
`<data>/vm-transcripts/<thread id>.json`, holding the chat's summary and its newest 400
messages. The folder is created with mode `0700` and each file with `0600`. A name that is
not a canonical UUID is refused, so a chat id cannot address another path.

Saves run on background threads and can overlap, so each save writes its own temporary file
and the lock is held until the rename:

```rust
// server/src/transcripts.rs, save
let staged = dir.join(format!(".{id}.{}", uuid::Uuid::new_v4()));
```

A save queued before a delete may only start after it. Deleted ids are remembered for the
life of the process and refused, so a deleted chat's copy does not come back:

```rust
// server/src/transcripts.rs, save
if removed.contains(id) { return Ok(()); }
```

## When a copy is written

[Backend::keep_transcript](../../server/src/chat.rs) copies only chats whose summary says
`environment.kind == "vm"`. It is called from two places:

- the worker listener, when a VM chat's turn completes:

```rust
// server/src/chat.rs, listen_client
if method == "turn/completed" { backend.keep_transcript(id, chat); }
```

- [snapshot](../../server/src/api.rs), when an idle VM chat is read. This also covers chats
  that existed before the copy did.

A failed write is ignored; the chat then shows no history while its VM is off.

## When a copy is shown

`GET /api/threads/{id}` for a VM chat whose worker is not attached returns the copy with
`vmOffline: true`, or an empty placeholder when there is none:

```rust
// server/src/api.rs, snapshot
let (mut summary, messages) = match kept {
    Some((mut thread, messages)) => { thread["vmOffline"] = json!(true); (thread, messages) }
    None => (offline_summary(&id, &vm_id), json!([])),
```

An offline snapshot is never loaded into the backend's chat cache, so it cannot accept a
message. The page therefore reloads the task as soon as the VM reports ready
([refreshVmStatus](../../web/src/chat.svelte.ts)). Before this reload existed, a send from
such a chat failed with `Task or route not found.`

```ts
// web/src/chat.svelte.ts, refreshVmStatus
if (this.vmReady && this.active?.vmOffline && !this.vmReloading && !this.busy) {
```

Deleting a chat removes its copy ([delete_chat](../../server/src/api.rs)).

## Verification

- `server/src/transcripts.rs` tests: private file mode, the 400-message limit, refusal of a
  name that is not a chat id, eight threads saving one chat at once, and a save that lands
  after the delete.
- [host-vm.test.mjs](../../tests/host-vm.test.mjs): a VM chat gets a `0600` copy, and the
  copy is served with `vmOffline: true` after the VM's worker stops. The fixture guest runs
  no turns, so the test writes the messages into the copy itself.
- [environments.spec.ts](../../tests/browser/environments.spec.ts): an offline task shows
  the kept messages, then reloads once its VM is ready.
