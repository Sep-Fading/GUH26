# Files and pictures in messages

A message can carry up to 10 uploads. A picture reaches the model as a picture, a small text file
as text, and any other file is named to the model and, in a project conversation on this Mac,
copied into the project's working folder. The store is [`attachments.rs`](../../server/src/attachments.rs),
the routes are [`api/attachments.rs`](../../server/src/api/attachments.rs), and the send route in
[`api.rs`](../../server/src/api.rs) ties uploads to their message.

Sequence: [sequence.md](sequence.md).

## Routes

| Route | Does |
|---|---|
| `POST /api/threads/{id}/attachments?name=` | Receives the file as the request body and answers `201 {attachment}` |
| `GET /api/threads/{id}/attachments/{attachment}` | Returns the bytes |
| `DELETE /api/threads/{id}/attachments/{attachment}` | Removes an upload no message carries; `409 attachment_sent` otherwise |
| `POST /api/threads/{id}/messages` and `/queue` | Accept `attachments: [id]` |

The upload body is written to disk as it arrives and stops at the limit, so the route has no body
limit of its own. It is the one mutation the request guard accepts as `application/octet-stream`;
it still needs `X-MiniClaw-Request: 1`, which a page of another site cannot send without a
preflight this backend never answers.

Only a picture is served as what it is. Every other upload is served as
`application/octet-stream` with `Content-Disposition: attachment`, so the page never renders or
runs a file the user uploaded.

## What an upload is

The kind comes from the content, never from the name:

```rust
if let Some(media) = picture(&head[..read]) { return Ok((Kind::Image, media.into())); }
if size <= TEXT_BYTES {
    let bytes = std::fs::read(path)?;
    if !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok() { return Ok((Kind::Text, "text/plain".into())); }
}
Ok((Kind::File, "application/octet-stream".into()))
```

`picture` knows PNG, JPEG, GIF and WebP by their first bytes. SVG is markup that can carry script,
so it is text.

| Limit | Value |
|---|---|
| Uploads per message | 10 |
| Picture | 20 MB |
| Text read into the turn | 256 KB; a larger text file is kind `file` |
| Any upload | 500 MB (`MINICLAW_TEST_UPLOAD_BYTES` lowers it in debug builds) |

A name is a plain file name: no folder, no leading dot, no control character, 255 bytes at most.

## Storage

`<data>/attachments/<thread>/<id>/content` holds the bytes and `record.json` the record, both
readable by their owner only. A record gains `message` when it is sent, `turn` when the turn
starts, `saved` when a copy is placed, and `position` among the uploads of its message. A
reloaded conversation finds its uploads by `turn` (`attach` in `api.rs`), because the providers'
history carries their own message ids.

Deleting the conversation moves the folder to the Trash as `MiniClaw uploads <id> <time>`.
Copies in a project's `uploads/` folder belong to the project and stay.

## What each provider receives

The message stays as typed. What is added travels beside it:

| Upload | Codex | Claude |
|---|---|---|
| Picture | a `localImage` input item with the path, before the text | one stream-json line on stdin with `image` content blocks (base64) before the text, with `--input-format stream-json` |
| Text file | `additionalContext.miniclaw_attachments`, which holds the file's text | the same note, inside `<miniclaw-context>` before the message |
| Other file | the same note: name, size, and the copy's path or that it cannot be opened | the same |

`miniclaw_attachments` is a one-turn key like `miniclaw_slash`: the real Codex appends a changed
key in full and keeps an omitted one, so the text is sent once
(`tests/experiments/skill-context-cost.test.mjs`). A Claude turn without pictures still writes
plain text to stdin.

A picture larger than 5 MB, or longer than 2576 px on an edge, gets a JPEG copy for the model made
with macOS's `sips`; the upload itself is kept as it was. 2576 px is the longest edge Anthropic's
vision guide gives for its newest models.

A message of uploads alone sends the words `(attached: <names>)`.

In a project conversation on this Mac every upload is also copied to `<workspace>/uploads/`. The
copy never replaces a file (`scene.blend`, then `scene 2.blend`) and is recorded at once, so a
repeated send does not copy twice. A plain conversation keeps the file and tells the model it
cannot open it.

## VM conversations

The VM's worker accepts text input only, so a picture is not sent there. The note tells the model
a picture was attached that it cannot see. Text files are read into the turn as on the Mac. No
copy is placed in the VM.

## The page

`Chat.files` holds the draft's files. They are uploaded when the message is sent, because a new
conversation does not exist before that; `uploaded` on each keeps a repeated send from uploading
twice. [`AttachButton.svelte`](../../web/src/AttachButton.svelte) opens the picker, the composer
takes dropped files and pasted pictures, [`DraftFiles.svelte`](../../web/src/DraftFiles.svelte)
lists them with a remove button, and [`MessageFiles.svelte`](../../web/src/MessageFiles.svelte)
shows a sent message's pictures and download links. The page's content policy allows `blob:`
pictures for the draft's thumbnails.

## Checked by

`tests/attachments.test.mjs` with the fake Codex and the fake Claude CLI,
`tests/browser/attachments.spec.ts`, and `tests/experiments/real-attachments.test.mjs`, where
`gpt-5.6-luna` and Claude Haiku 4.5 each named a red picture's colour and quoted a text file.
