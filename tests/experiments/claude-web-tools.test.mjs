// Checks independent web search through the real Claude CLI.
import assert from 'node:assert/strict';
// Runs the check as one test case.
import { test } from 'node:test';
// Locates the signed-in CLI.
import { homedir } from 'node:os';
import { join } from 'node:path';
// Starts a disposable backend and follows a chat's events.
import { launchApp, json, events, eventually } from '../support/harness.mjs';

// One real model call on the signed-in Claude account, so it only runs when asked for.
const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });

test('a Claude chat answers from its independent web search', { skip: !enabled, timeout: 240_000 }, async t => {
  const app = await launchApp({}, { env: { CLAUDE_BIN: process.env.CLAUDE_BIN ?? join(homedir(), '.local/bin/claude') } });
  t.after(() => app.close());
  const models = (await json(await app.request('/api/models'))).models.filter(model => model.provider === 'claude');
  const model = (models.find(entry => /haiku/i.test(entry.model)) ?? models[0]).model;
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model }), 201);
  const stream = await events(app, `/api/threads/${thread.id}/events`);
  t.after(() => stream.close());
  await json(await post(app, `/api/threads/${thread.id}/messages`, { model, clientMessageId: crypto.randomUUID(),
    text: 'Use your web search tool to find the year the Eiffel Tower was completed. Then say which tools you have, by exact name. Be brief.' }), 202);
  // A real search takes longer than the helper's default wait.
  const done = await eventually(() => stream.seen.find(event => event.event === 'turn.completed'),
    Boolean, 'the real turn to finish', 200_000);
  assert.equal(done.data.status, 'completed');
  const reply = stream.seen.find(event => event.event === 'message.completed').data.text;
  console.log(`reply: ${reply.slice(0, 400)}`);
  assert.match(reply, /1889/);
  assert.doesNotMatch(reply, /\bBash\b|\bRead\b|\bEdit\b|\bWrite\b/);
});
