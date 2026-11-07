// A real model, a real picture. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-attachments.test.mjs
// It checks what no fake can: that the provider really shows the model an uploaded picture and
// the text of an uploaded file. Add MINICLAW_PROBE_CLAUDE=1 to run the same through Claude Code.
import assert from 'node:assert/strict';
// Registers the opt-in experiment.
import { test } from 'node:test';
// Builds a picture with an unmistakable colour.
import { deflateSync, crc32 } from 'node:zlib';
// Starts MiniClaw against the installed providers and their existing sign-ins.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

/** Makes a 64 by 64 PNG filled with pure red. */
function redSquare() {
  const chunk = (type, data) => { const body = Buffer.concat([Buffer.from(type), data]); const size = Buffer.alloc(4); size.writeUInt32BE(data.length);
    const sum = Buffer.alloc(4); sum.writeUInt32BE(crc32(body)); return Buffer.concat([size, body, sum]); };
  const header = Buffer.alloc(13); header.writeUInt32BE(64, 0); header.writeUInt32BE(64, 4); header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(64 * 3, Buffer.from([255, 0, 0]))]);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(Array(64).fill(row)))), chunk('IEND', Buffer.alloc(0))]);
}

/** Uploads a picture and a text file, asks about both, and checks the answer. */
async function seesBoth(app, choice) {
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...choice }), 201);
  const upload = async (name, bytes) => (await json(await app.request(`/api/threads/${thread.id}/attachments?name=${name}`, { method: 'POST', body: bytes,
    headers: { 'Content-Type': 'application/octet-stream', 'X-MiniClaw-Request': '1' }, signal: AbortSignal.timeout(30000) }), 201)).attachment.id;
  const attachments = [await upload('square.png', redSquare()), await upload('secret.txt', Buffer.from('The password of the day is marmalade-42.\n'))];
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Two short lines: the single colour that fills the picture, and the password in the text file.',
    clientMessageId: crypto.randomUUID(), attachments, ...choice }), 202);
  const done = await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'the reply', 120000);
  const answer = done.messages.at(-1).text;
  console.log(`${choice.model}:`, answer.replaceAll('\n', ' | '));
  assert.match(answer, /red/i);
  assert.match(answer, /marmalade-42/);
}

test('a real Codex model sees an uploaded picture and reads an uploaded file', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  await seesBoth(app, { model: 'gpt-5.6-luna', effort: 'low' });
});

test('a real Claude model sees them too', { skip: !(enabled && process.env.MINICLAW_PROBE_CLAUDE === '1') && 'set MINICLAW_PROBE_CLAUDE=1 as well', timeout: 300000 }, async t => {
  // The harness keeps the real Claude CLI off unless a test names it.
  const { execFileSync } = await import('node:child_process');
  const app = await launchApp({}, { real: true, env: { CLAUDE_BIN: execFileSync('/bin/zsh', ['-lc', 'command -v claude'], { encoding: 'utf8' }).trim() } });
  t.after(() => app.close());
  await eventually(async () => (await app.request('/api/models')).json(), value => JSON.stringify(value).includes('claude-haiku'), 'the Claude models', 60000);
  await seesBoth(app, { model: 'claude-haiku-4-5-20251001', effort: 'low' });
});
