# VM desktop stream

The chat page shows a live, view-only picture of the VM a chat runs in. Frames travel from
a capture helper in the guest, through that VM's worker and the Mac backend, to a canvas
in the page. The picture never accepts input; control happens in the VM's own window.

See the [stream sequence](sequence.md).

## Capture in the guest

[desktop-stream.swift](../../scripts/vm/desktop-stream.swift) captures the first display
with ScreenCaptureKit and writes length-prefixed JPEGs to its standard output. It captures
at up to 30 frames a second and scales the longer side down to 1920 pixels:

```swift
// scripts/vm/desktop-stream.swift, DesktopStream.main
let scale = min(1.0, 1920.0 / Double(max(display.width, display.height)))
```

```swift
// scripts/vm/desktop-stream.swift, DesktopStream.main
config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
```

A frame is encoded at quality 0.7, and again at 0.45 when the result exceeds the 768 KiB
frame limit. A frame that arrives while the previous one is still encoding is dropped:

```swift
// scripts/vm/desktop-stream.swift, DesktopOutput.stream
guard !encoding else { return }
```

## Screen Recording permission

macOS charges the capture to the worker's launch job, not to the helper. This was observed
in a real guest: adding the worker to Screen Recording made frames appear; adding the
helper did not. The worker's file name contains its digest and it is ad hoc signed, so
each worker update needs the grant again.

The worker asks for the permission itself. When a helper ends without delivering one
frame, [capture_desktop](../../server/src/worker.rs) runs the helper's `--authorize` mode
once per worker process, which makes macOS show its request for the worker:

```rust
// server/src/worker.rs, capture_desktop
if !delivered { request_capture_access(&binary).await; }
```

The guest installer no longer asks from Terminal, because that request names Terminal.

## Worker: one frame per request

The backend asks the VM's worker for `miniclaw/desktop/frame`. The worker starts the helper
on the first request and stops it after 30 seconds without requests. Each reply reads from
the requesting viewer's own place in the frame sequence, so a frame is neither repeated nor
skipped. A reply waits up to one second for a
new frame, then repeats the newest one so a still screen still answers:

```rust
// server/src/worker.rs, next_frame
async fn next_frame(latest: &Frames, wait: Duration) -> Option<Arc<Vec<u8>>> {
    let mut latest = latest.lock().await;
    match tokio::time::timeout(wait, latest.changed()).await {
        Ok(Err(_)) => None,
        _ => latest.borrow_and_update().clone(),
    }
}
```

Each HTTP stream names itself with a `viewer` id, and the worker keeps one place per
viewer ([Viewers](../../server/src/worker.rs)), at most eight, dropping the longest-idle
one. Two tabs on one VM therefore both get every frame. A worker installed before viewers
existed accepts only empty parameters, so the backend asks it again the plain way:

```rust
// server/src/api.rs, desktop_stream
let (first, viewer) = match desktop_frame(&client, Some(&named)).await {
    Ok(frame) => (frame, Some(named)),
    Err(_) => (desktop_frame(&client, None).await?, None),
};
```

## Backend: the HTTP stream

`GET /api/vms/{vm_id}/desktop` ([desktop_stream](../../server/src/api.rs)) requires the VM
to be running with its worker ready, and never boots a VM. It answers with
`multipart/x-mixed-replace; boundary=frame`. Each part states its length
([frames::part](../../server/src/frames.rs)), because the page parses the stream itself.

The backend drops a frame identical to the last one it sent
([frames::Fresh](../../server/src/frames.rs)) and asks again after 33 ms. Before this
filter existed, a guest that answered at once produced about 230 identical frames a second
(measured: 1,405 frames and 203 MB in six seconds, nine of them distinct).

A failed frame read does not end the stream. The guest's capture restarts around login and
display changes, so the backend retries every 500 ms and ends the stream only after 60
consecutive failures or when the worker is gone:

```rust
// server/src/api.rs, desktop_stream
Err(_) if failures < 60 && client.available() => {
    failures += 1;
    tokio::time::sleep(Duration::from_millis(500)).await;
    continue;
}
```

## Page: reading and painting

[DesktopView.svelte](../../web/src/DesktopView.svelte) does not use an `<img>` for the
stream. Two browser behaviours rule it out, both measured in Chromium:

- An `<img>` receives one `load` event for a multipart stream and no event when the stream
  ends. A stream that ended left its last frame on screen for good.
- A multipart frame is drawn only once the next part's headers arrive
  ([mjpeg-draw.test.mjs](../../tests/experiments/mjpeg-draw.test.mjs)), so a screen that
  went still showed the frame before last.

[readFrames](../../web/src/desktop-stream.ts) fetches the stream and
[FrameParser](../../web/src/desktop-stream.ts) cuts it into frames by `Content-Length`. A
part without a usable length is refused rather than scanned for a boundary. While the VM is
ready the page keeps a stream open, and reopens one that ended or failed after two seconds.

Frames are decoded with `createImageBitmap` and painted on a canvas inside an element with
`role="img"`. A canvas needs no image URL, so the page's Content-Security-Policy keeps
`img-src 'self' data: https:`; showing frames as `blob:` URLs was refused by that policy.

The frame takes the picture's own shape, so a portrait guest display shows no bars. Beside
the chat it fills the column; stacked above the chat, on windows narrower than 1024 px, it
is limited to 28% of the window height, 22% on phones, and can be folded away.

## Window and shutdown actions

`POST /api/vms/{vm_id}/desktop/show` and `/desktop/shutdown` reach the VM session through
the host worker ([desktop_action](../../server/src/api.rs)). Both need
`X-MiniClaw-Request: 1` and a JSON content type, which the mutation guard requires of every
POST.

## Verification

- [desktop-capture.test.mjs](../../tests/desktop-capture.test.mjs): capture settings, the
  worker's permission request, and the installer replacing an older helper without
  signature-checking the file on the setup disk.
- `server/src/worker.rs` tests: frames are neither repeated nor skipped, each of two
  viewers gets every frame, viewer places are bounded and viewer ids are checked.
- [environment_routing.rs](../../server/tests/environment_routing.rs): a stream names its
  viewer and still works with a worker that refuses the name.
- `server/src/frames.rs` tests: part format, duplicate filter, frame codec.
- [desktop-stream.test.ts](../../web/tests/desktop-stream.test.ts): frame parsing across
  arbitrary chunk sizes, boundary-like bytes inside a frame, refusal of a missing length.
- [environments.spec.ts](../../tests/browser/environments.spec.ts): the panel decodes a
  frame under the real Content-Security-Policy, reconnects when a stream ends, explains a
  missing picture, and sends window and shutdown actions with the required headers.

Observed with a real guest on 2026-09-20: the panel painted the current desktop at
1024x1096 with no console errors. Frame rate under load was not measured.
