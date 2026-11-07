// Compares settings responses without running tools.
import assert from 'node:assert/strict';
// Exercises disposable controller and approval storage.
import { test } from 'node:test';
// Starts a backend with synthetic Codex replies.
import { launchApp, json, eventually } from './support/harness.mjs';
// Uses one known synthetic chat identifier.
import { THREAD } from './support/fixtures.mjs';

// Sends a guarded JSON mutation through MiniClaw.
const post = (app, path, body) => app.request(path, { method: 'POST', body });
// Names one exact app action for settings.
const textedit = { target_type: 'application', id: 'com.apple.TextEdit', action: 'control' };

test('agent and conversation permission settings persist with sources', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = (await json(await post(app, '/api/agents', {
    name: 'Editor', instructions: 'Use approved tools only.', active: true,
  }), 201)).agent;
  await json(await post(app, '/api/threads', {
    agentId: agent.id, environment: { kind: 'host' },
  }), 201);

  const agentPath = `/api/permissions/agent/${agent.id}`;
  const chatPath = `/api/permissions/conversation/${THREAD}`;
  await json(await post(app, agentPath, { target: textedit, effect: 'allow' }));
  let settings = await json(await app.request(chatPath));
  assert.deepEqual(settings.sources.agent, [{ target: textedit, effect: 'allow' }]);
  assert.deepEqual(settings.sources.conversation, []);

  await json(await post(app, chatPath, { target: textedit, effect: 'block' }));
  settings = await json(await app.request(chatPath));
  assert.deepEqual(settings.sources.conversation, [{ target: textedit, effect: 'block' }]);
  assert.deepEqual(settings.effective, [{ target: textedit, decision: 'block', source: 'conversation' }]);
  await app.restart();
  assert.deepEqual(await json(await app.request(chatPath)), settings);

  await json(await post(app, chatPath, { target: textedit, effect: 'remove' }));
  settings = await json(await app.request(chatPath));
  assert.deepEqual(settings.effective, [{ target: textedit, decision: 'allow', source: 'agent' }]);
});

test('permission writes reject malformed targets and unknown scopes', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const missing = `/api/permissions/agent/${THREAD}`;
  assert.equal((await post(app, missing, { target: textedit, effect: 'allow' })).status, 404);
  assert.equal((await app.request(`/api/permissions/unknown/${THREAD}`)).status, 400);
  assert.equal((await post(app, `/api/permissions/conversation/${THREAD}`, {
    target: { ...textedit, action: 'execute' }, effect: 'allow',
  })).status, 400);
});

test('requesting another app records review work but never grants access', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const path = `/api/permissions/conversation/${THREAD}`;
  const unknown = { target_type: 'application', id: 'com.apple.Preview', action: 'control' };
  assert.equal((await post(app, path, { target: unknown, effect: 'allow' })).status, 400);
  await json(await post(app, `${path}/app-requests`, { name: 'Preview' }), 201);
  assert.equal((await app.request(`${path}/app-requests`, { method: 'POST',
    body: { name: 'Terminal' }, headers: { Origin: 'https://unrelated.example' },
  })).status, 403);
  await app.emit({ id: 'forged-app-review', method: 'item/tool/call', params: {
    tool: 'request_application', arguments: { name: 'Terminal' },
  } });
  const model = await eventually(() => app.inspect(), value =>
    value.responses.some(reply => reply.id === 'forged-app-review'));
  assert.equal(model.responses.find(reply => reply.id === 'forged-app-review').result.success, false);
  await app.emit({ id: 'scoped-native-before-activation', method: 'item/tool/call', params: {
    tool: 'miniclaw_scoped_native', threadId: THREAD, turnId: 'forged-turn',
    callId: 'forged-call', arguments: { app: 'com.apple.TextEdit', action: 'observe' },
  } });
  const scoped = await eventually(() => app.inspect(), value =>
    value.responses.some(reply => reply.id === 'scoped-native-before-activation'));
  assert.equal(scoped.responses.find(reply =>
    reply.id === 'scoped-native-before-activation').result.success, false);
  let settings = await json(await app.request(path));
  assert.equal(settings.appCatalog[0].id, 'com.apple.TextEdit');
  assert.deepEqual(settings.appRequests, [{ name: 'Preview', status: 'pending' }]);
  assert.deepEqual(settings.effective, []);
  await app.restart();
  settings = await json(await app.request(path));
  assert.deepEqual(settings.appRequests, [{ name: 'Preview', status: 'pending' }]);
  assert.equal((await post(app, `${path}/app-requests`, { name: '  ' })).status, 400);
});

test('model tool calls and cross-site writes cannot alter rules', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const agent = (await json(await post(app, '/api/agents', {
    name: 'Editor', instructions: 'Use approved tools only.', active: true,
  }), 201)).agent;
  const path = `/api/permissions/agent/${agent.id}`;
  assert.equal((await app.request(path, { method: 'POST',
    body: { target: textedit, effect: 'allow' },
    headers: { Origin: 'https://unrelated.example' },
  })).status, 403);
  await app.emit({ id: 'forged-permission', method: 'item/tool/call', params: {
    tool: 'set_permission', arguments: { path, target: textedit, effect: 'allow' },
  } });
  const state = await eventually(() => app.inspect(), value =>
    value.responses.some(reply => reply.id === 'forged-permission'));
  assert.equal(state.responses.find(reply => reply.id === 'forged-permission').result.success, false);
  assert.deepEqual((await json(await app.request(path))).rules, []);
});
