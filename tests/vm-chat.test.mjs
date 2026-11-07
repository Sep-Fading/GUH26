// Verifies the real signed-in guest connection.
import assert from 'node:assert/strict';
// Runs only explicitly selected live VM tests.
import { test } from 'node:test';
// Identifies one harmless live test message.
import { randomUUID } from 'node:crypto';

const base = process.env.MINICLAW_PAIRED_URL;

test('paired VMs stream a real guest Codex reply', { skip: !base, timeout: 100_000 }, async () => {
  assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
  const signal = AbortSignal.timeout(90_000);
  // Uses the local API with the same mutation headers as the browser.
  async function request(path, body) {
    const response = await fetch(`${base}${path}`, { signal,
      ...(body === undefined ? {} : { method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-MiniClaw-Request': '1', Origin: base,
      }, body: JSON.stringify(body) }),
    });
    assert.ok(response.ok, `HTTP ${response.status} for ${path}`);
    return response.json();
  }
  assert.deepEqual(await request('/api/health'), { backend: 'ok', codex: 'connected' });
  assert.equal((await request('/api/account')).status, 'signed_in');
  const { thread } = await request('/api/threads', { model: 'gpt-6-astra', effort: 'low' });
  console.log(`Retained guest test conversation: ${thread.id}`);
  const stream = await fetch(`${base}/api/threads/${thread.id}/events`, { signal });
  assert.equal(stream.status, 200);
  const reader = stream.body.getReader();
  // Collects event framing independently of the message acknowledgement.
  const received = (async () => {
    let text = '';
    const decoder = new TextDecoder();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      text += decoder.decode(value, { stream: true });
      if (text.includes('event: turn.completed')) break;
    }
    return text;
  })();
  try {
    await request(`/api/threads/${thread.id}/messages`, {
      text: 'Reply exactly VM_CHAT_OK. Do not use tools or access files.', clientMessageId: randomUUID(),
    });
    const transcript = await received;
    assert.match(transcript, /event: message.delta/);
    assert.match(transcript, /event: turn.completed/);
    const snapshot = await request(`/api/threads/${thread.id}`);
    assert.equal(snapshot.activeTurn, null);
    assert.equal(snapshot.messages.at(-1).text.trim(), 'VM_CHAT_OK');
  } finally {
    await reader.cancel();
    await received.catch(() => {});
  }
});
