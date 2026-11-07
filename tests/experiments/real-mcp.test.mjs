// A real model and an MCP server. Opt-in, because it makes paid model calls:
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-mcp.test.mjs
// It checks what failed for the user on 2026-09-21: asked to "use the <name> MCP", a real model said
// it had no access without ever calling the gateway.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { copyFile, mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const MODEL = { model: 'gpt-5.6-luna', effort: 'low' };
const MAIN = '00000000-0000-0000-0000-000000000000';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body, signal: AbortSignal.timeout(30000) });

test('asked to use a named MCP server, a real Codex model goes through the gateway', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true });
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-real-mcp-')));
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  await copyFile(new URL('../support/fake-mcp.mjs', import.meta.url), join(folder, 'server.mjs'));
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'parrot', kind: 'stdio', command: process.execPath,
    args: [join(folder, 'server.mjs')], folder }), 201);
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, ...MODEL }), 201);
  const snapshot = async () => json(await app.request(`/api/threads/${thread.id}`));
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Use the Parrot MCP to echo the word "kiwi", and tell me exactly what it returned.',
    clientMessageId: crypto.randomUUID(), ...MODEL }), 202);
  // Each tool asks first; the user approves.
  let approved = 0;
  const done = await eventually(async () => {
    const value = await snapshot();
    if (value.scopedApproval) { approved += 1; await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: 'approve_once' }); }
    return value;
  }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'the reply', 180000);
  console.log('ACTIVITY:', JSON.stringify(done.activities.map(entry => [entry.kind, entry.title, entry.status])));
  console.log('REPLY:', done.messages.at(-1).text.slice(0, 300).replaceAll('\n', ' | '));
  assert.ok(done.activities.some(entry => entry.kind === 'mcp' && entry.title === 'parrot: echo' && entry.status === 'ok'), 'it called the tool through the gateway');
  assert.ok(approved >= 1);
  assert.match(done.messages.at(-1).text, /echo: kiwi/);
});

test('the same request in a Computer Use conversation, which is where it failed for the user', { skip: !enabled && 'set MINICLAW_NATIVE_MODEL_PROBE=1', timeout: 300000 }, async t => {
  const app = await launchApp({}, { real: true });
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-real-mcp-')));
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  await copyFile(new URL('../support/fake-mcp.mjs', import.meta.url), join(folder, 'server.mjs'));
  await eventually(async () => (await app.request('/api/account')).json(), value => value.status === 'signed_in', 'the Codex sign-in', 60000);
  const computer = await json(await app.request('/api/computer'));
  console.log('COMPUTER:', JSON.stringify(computer));
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'parrot', kind: 'stdio', command: process.execPath,
    args: [join(folder, 'server.mjs')], folder }), 201);
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  const created = await post(app, '/api/threads', { environment: { kind: 'host' }, computerUse: true, ...MODEL });
  console.log('CREATE:', created.status, created.status === 201 ? '' : (await created.clone().text()).slice(0, 300));
  const { thread } = await json(created, 201);
  const snapshot = async () => json(await app.request(`/api/threads/${thread.id}`));
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Use the Parrot MCP to echo the word "kiwi", and tell me exactly what it returned. Do not use the computer.',
    clientMessageId: crypto.randomUUID(), ...MODEL }), 202);
  const done = await eventually(async () => {
    const value = await snapshot();
    if (value.scopedApproval) await post(app, `/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: 'approve_once' });
    return value;
  }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'the reply', 180000);
  console.log('ACTIVITY:', JSON.stringify(done.activities.map(entry => [entry.kind, entry.title, entry.status])));
  console.log('REPLY:', done.messages.at(-1).text.slice(0, 300).replaceAll('\n', ' | '));
  assert.match(done.messages.at(-1).text, /echo: kiwi/);
});

