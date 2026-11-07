// Reads a conversation's shell output as it is printed, and types into it. The stream is its own
// server-sent event feed, apart from the chat's, so keystrokes and output never crowd out chat
// events. `EventSource` reconnects by itself and sends the last event id back, which is the ring
// offset the reader had reached, so a reconnect continues where it left off.

export type TerminalHandlers = {
  output: (bytes: Uint8Array) => void;
  reset: () => void;
  exit: (code: number) => void;
  notice: (text: string) => void;
  error: () => void;
};

// Turns base64 into bytes, for xterm.
export function decodeChunk(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

// Opens the stream from an offset and hands each event to its handler. Returns a way to close it.
export function openTerminalStream(thread: string, after: number, handlers: TerminalHandlers) {
  const feed = new EventSource(`/api/threads/${thread}/terminal/stream?after=${after}`);
  const read = (event: Event) => { try { return JSON.parse((event as MessageEvent).data); } catch { return null; } };
  feed.addEventListener('output', event => { const data = read(event); if (data?.bytes) handlers.output(decodeChunk(data.bytes)); });
  feed.addEventListener('reset', () => handlers.reset());
  feed.addEventListener('exit', event => { const data = read(event); handlers.exit(typeof data?.code === 'number' ? data.code : -1); });
  feed.addEventListener('notice', event => { const data = read(event); if (typeof data?.text === 'string') handlers.notice(data.text); });
  feed.addEventListener('error', () => { if (feed.readyState === EventSource.CLOSED) handlers.error(); });
  return { close: () => feed.close() };
}

// Posts one terminal request the way the rest of the page does.
export async function terminalCall<T>(thread: string, action: string, body: unknown): Promise<T> {
  const response = await fetch(`/api/threads/${thread}/terminal/${action}`, { method: 'POST', cache: 'no-store',
    headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }, body: JSON.stringify(body) });
  const value = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(value.error?.message ?? 'The terminal request failed.', { cause: value.error?.code });
  return value as T;
}
