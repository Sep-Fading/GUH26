# VM desktop stream sequence

```mermaid
sequenceDiagram
    participant Page as DesktopView (page)
    participant API as desktop_stream (backend)
    participant Worker as VM worker
    participant Helper as miniclaw-desktop-stream

    Page->>API: GET /api/vms/{vm_id}/desktop
    API->>API: desktop_target: VM running and worker ready
    API->>Worker: miniclaw/desktop/frame { viewer }
    Worker->>Helper: start on first request
    Helper-->>Worker: length-prefixed JPEG frames
    alt helper ended without a frame
        Worker->>Helper: --authorize (once per worker process)
        Worker-->>API: error
        API->>API: wait 500 ms, retry (up to 60 times)
    end
    Worker-->>API: next unseen frame, or the newest after 1 s
    alt same bytes as the last frame sent
        API->>API: drop, wait 33 ms, ask again
    else new frame
        API-->>Page: multipart part with Content-Length
        Page->>Page: FrameParser cuts the frame
        Page->>Page: createImageBitmap, paint canvas
    end
    alt stream ends or fails
        Page->>Page: wait 2 s
        Page->>API: GET /api/vms/{vm_id}/desktop
    end
```

| Participant | Source |
| --- | --- |
| DesktopView | [DesktopView.svelte](../../web/src/DesktopView.svelte), [desktop-stream.ts](../../web/src/desktop-stream.ts) `readFrames`, `FrameParser` |
| desktop_stream | [api.rs](../../server/src/api.rs) `desktop_stream`, `desktop_target`, `desktop_frame`; [frames.rs](../../server/src/frames.rs) `part`, `Fresh` |
| VM worker | [worker.rs](../../server/src/worker.rs) `Viewers`, `next_frame`, `capture_desktop`, `request_capture_access` |
| Helper | [desktop-stream.swift](../../scripts/vm/desktop-stream.swift) |

Back to [VM desktop stream](README.md).
