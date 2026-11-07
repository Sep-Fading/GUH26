// Reads the VM desktop stream in the page. An <img> cannot do this job: the browser fires no
// event when a multipart stream ends, so a stream that died left its last frame on screen for
// good, and it draws each frame only when the next one starts arriving.

const HEADER_END = [13, 10, 13, 10];
const MAX_FRAME = 4 * 1024 * 1024;

// Finds a byte sequence in a buffer, or returns -1.
function indexOf(bytes: Uint8Array, needle: number[], from = 0) {
  outer: for (let i = from; i <= bytes.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (bytes[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}

// Splits a multipart JPEG stream into frames as its bytes arrive. Every part must state its
// Content-Length, which MiniClaw's backend does, so no frame is ever scanned for a boundary.
export class FrameParser {
  private buffer = new Uint8Array(0);

  // Adds received bytes and returns every frame that is now complete.
  push(chunk: Uint8Array): Uint8Array[] {
    const joined = new Uint8Array(this.buffer.length + chunk.length);
    joined.set(this.buffer);
    joined.set(chunk, this.buffer.length);
    this.buffer = joined;
    const frames: Uint8Array[] = [];
    while (true) {
      const end = indexOf(this.buffer, HEADER_END);
      if (end < 0) break;
      const header = new TextDecoder().decode(this.buffer.subarray(0, end));
      const length = Number(/content-length:\s*(\d+)/i.exec(header)?.[1]);
      if (!Number.isSafeInteger(length) || length <= 0 || length > MAX_FRAME) {
        throw new Error('The desktop stream sent a part without a valid length.');
      }
      const start = end + HEADER_END.length;
      if (this.buffer.length < start + length) break;
      frames.push(this.buffer.slice(start, start + length));
      this.buffer = this.buffer.slice(start + length);
    }
    if (this.buffer.length > MAX_FRAME + 1024) throw new Error('The desktop stream sent too much without a frame.');
    return frames;
  }
}

// Reads one stream until it ends, handing each frame to `show`. Resolves when the stream
// ends by itself, rejects when it cannot be opened, and stops quietly when aborted.
export async function readFrames(url: string, signal: AbortSignal,
  show: (frame: Uint8Array) => void): Promise<void> {
  const response = await fetch(url, { signal, cache: 'no-store' });
  if (!response.ok || !response.body) throw new Error(`Desktop stream unavailable (${response.status}).`);
  if (!response.headers.get('content-type')?.startsWith('multipart/')) {
    // A single picture instead of a stream is shown as one frame.
    show(new Uint8Array(await response.arrayBuffer()));
    return;
  }
  const parser = new FrameParser();
  const reader = response.body.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) return;
    // Only the newest complete frame matters; older ones in the same chunk are skipped.
    const frames = parser.push(value);
    if (frames.length) show(frames[frames.length - 1]);
  }
}
