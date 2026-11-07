// Checks how the page splits the desktop stream into frames.
import { describe, expect, it } from 'vitest';
// Parses the stream under test.
import { FrameParser } from '../src/desktop-stream';

const encode = (text: string) => new TextEncoder().encode(text);
// Formats one part the way MiniClaw's backend does.
function part(body: string) {
  return encode(`--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${body.length}\r\n\r\n${body}\r\n`);
}

describe('FrameParser', () => {
  it('returns each frame once its last byte has arrived, however the bytes are cut', () => {
    const bytes = new Uint8Array([...part('first'), ...part('second frame')]);
    for (const size of [1, 7, 64, bytes.length]) {
      const parser = new FrameParser();
      const frames: string[] = [];
      for (let at = 0; at < bytes.length; at += size) {
        for (const frame of parser.push(bytes.subarray(at, at + size))) frames.push(new TextDecoder().decode(frame));
      }
      expect(frames).toEqual(['first', 'second frame']);
    }
  });

  it('keeps a frame whose bytes look like a boundary', () => {
    const parser = new FrameParser();
    expect(parser.push(part('a\r\n--frame\r\nb')).map(frame => new TextDecoder().decode(frame)))
      .toEqual(['a\r\n--frame\r\nb']);
  });

  it('refuses a part without a usable length instead of guessing', () => {
    expect(() => new FrameParser().push(encode('--frame\r\nContent-Type: image/jpeg\r\n\r\nxx'))).toThrow(/valid length/);
    expect(() => new FrameParser().push(encode('--frame\r\nContent-Length: 99999999999\r\n\r\n'))).toThrow(/valid length/);
  });
});
