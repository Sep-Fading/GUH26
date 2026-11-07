// Opt-in single-image smoke against the signed-in Codex runtime; deletes its own task.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launchApp, json, eventually } from '../support/harness.mjs';

test('subscription image generation reaches MiniClaw', {
  skip: process.env.MINICLAW_REAL_IMAGE !== '1', timeout: 240_000,
}, async () => {
  const app = await launchApp({}, { real: true });
  let thread;
  try {
    const account = await eventually(async () => {
      const response = await app.request('/api/account');
      return response.ok ? response.json() : { status: 'checking' };
    }, value => value.status === 'signed_in', 'Codex OAuth account', 30_000).catch(error => {
      console.log('STARTUP_DIAGNOSTIC', app.logs().split('\n').filter(line =>
        /unavailable|failure|error|invalid|rejected|runtime/i.test(line)).map(line => line.slice(0, 160)).slice(-6).join(' | '));
      throw error;
    });
    assert.equal(account.status, 'signed_in');
    const main = '00000000-0000-0000-0000-000000000000';
    await json(await app.request(`/api/permissions/agent/${main}`, { method: 'POST', body: {
      target: { target_type: 'feature', name: 'image_generation' }, effect: 'allow',
    } }));
    thread = (await json(await app.request('/api/threads', { method: 'POST', body: {
      environment: { kind: 'host' }, model: 'gpt-6-sol', effort: 'low',
      capabilities: { image_generation: true },
    } }), 201)).thread.id;
    await json(await app.request(`/api/threads/${thread}/messages`, { method: 'POST', body: {
      text: 'Use the built-in image generation tool once to make a small, plain red circle on a white background. Reply briefly when it is done.',
      clientMessageId: crypto.randomUUID(), model: 'gpt-6-sol', effort: 'low',
    } }), 202);
    const snapshot = await eventually(async () => json(await app.request(`/api/threads/${thread}`)),
      value => !value.activeTurn && !value.pending && value.messages.some(message => message.imageStatus),
      'one image turn', 180_000);
    const generated = snapshot.messages.find(message => message.imageStatus === 'completed');
    assert.ok(generated, JSON.stringify(snapshot.messages.map(message => ({ id: message.id, status: message.imageStatus, text: message.text.slice(0, 100) }))));
    const image = generated.attachments?.[0];
    assert.equal(image?.kind, 'image');
    const response = await app.request(`/api/threads/${thread}/attachments/${image.id}`);
    assert.equal(response.headers.get('content-type'), 'image/png');
    assert.ok((await response.arrayBuffer()).byteLength > 1000);
    console.log('IMAGE_SMOKE completed=true served=true');
  } finally {
    if (thread) await app.request(`/api/threads/${thread}/delete`, { method: 'POST', body: {} }).catch(() => {});
    await app.close();
  }
});
