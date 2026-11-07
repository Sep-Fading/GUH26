// Checks the terminal stream helper without a browser: chunks decode to bytes, each event kind
// reaches its handler, and a closed feed is reported.
import { describe, expect, test, vi } from 'vitest';
import { decodeChunk, openTerminalStream } from '../src/terminal-stream';

class FakeSource extends EventTarget {
  static CLOSED = 2;
  readyState = 1;
  url: string;
  constructor(url: string) { super(); this.url = url; FakeSource.last = this; }
  close() { this.readyState = 2; }
  static last: FakeSource;
}

describe('terminal stream', () => {
  test('base64 output becomes the bytes the shell printed', () => {
    expect(new TextDecoder().decode(decodeChunk(btoa('hi\r\n')))).toBe('hi\r\n');
  });

  test('every event kind reaches its handler, and the offset is asked for', () => {
    vi.stubGlobal('EventSource', FakeSource);
    const seen: string[] = [];
    const stream = openTerminalStream('t1', 42, {
      output: bytes => seen.push(`out:${new TextDecoder().decode(bytes)}`), reset: () => seen.push('reset'),
      exit: code => seen.push(`exit:${code}`), notice: text => seen.push(`notice:${text}`), error: () => seen.push('error') });
    const source = FakeSource.last;
    expect(source.url).toBe('/api/threads/t1/terminal/stream?after=42');
    source.dispatchEvent(new MessageEvent('output', { data: JSON.stringify({ bytes: btoa('ok') }) }));
    source.dispatchEvent(new MessageEvent('reset', { data: '{}' }));
    source.dispatchEvent(new MessageEvent('exit', { data: JSON.stringify({ code: 3 }) }));
    source.dispatchEvent(new MessageEvent('notice', { data: JSON.stringify({ text: 'restarted' }) }));
    source.dispatchEvent(new Event('error'));
    source.close();
    source.dispatchEvent(new Event('error'));
    stream.close();
    expect(seen).toEqual(['out:ok', 'reset', 'exit:3', 'notice:restarted', 'error']);
    vi.unstubAllGlobals();
  });
});
