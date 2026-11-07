import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { launchApp, json, eventually, stopChild } from './support/harness.mjs';
import { THREAD } from './support/fixtures.mjs';

// Executes only a V8 inventory query. Native MCP tools are discovered, never called.
for (const [computerUse, resume, live] of [[false, false, false], [true, false, false],
  [true, true, false], [true, true, true]]) {
test(`Astra runtime inventory: computer=${computerUse}, resume=${resume}, live=${live}`, {
  timeout: 45_000,
}, async t => {
  if (computerUse) assert.ok(process.env.MINICLAW_HELPER_BIN, 'Set the installed helper path explicitly');
  const app = await launchApp();
  let start, restore, listing, enable, children;
  try {
    if (resume) {
      await json(await app.request('/api/threads'));
      listing = (await app.inspect()).requests.find(r => r.method === 'thread/list').params;
      await json(await app.request(`/api/threads/${THREAD}`));
      restore = (await app.inspect()).requests.find(r => r.method === 'thread/resume').params;
    }
    await json(await app.request('/api/threads', { method: 'POST', body: { computerUse } }), 201);
    start = (await app.inspect()).requests.find(r => r.method === 'thread/start').params;
    if (live) {
      await app.set('thread/archive', { result: {} });
      await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
      for (const enabled of [false, true]) {
        await json(await app.request(`/api/threads/${THREAD}/computer`, {
          method: 'POST', body: { enabled },
        }));
        const request = (await app.inspect()).requests.filter(r => r.method === 'thread/resume').at(-1);
        if (enabled) enable = request.params;
        else restore = request.params;
      }
      children = (await app.inspect()).requests.find(r => r.params?.ancestorThreadId).params;
    }
  } finally { await app.close(); }

  const home = await mkdtemp(join(tmpdir(), 'mc-runtime-'));
  const journal = join(home, 'fake-calls.txt');
  t.after(() => rm(home, { recursive: true, force: true }));
  const bodies = [];
  let phase = 'initial';
  const issued = new Set();
  const code = 'text("MINICLAW_INVENTORY:" + JSON.stringify({' +
    'tools: ALL_TOOLS.map(t => t.name), process: typeof process, require: typeof require}));';
  const attempt = nonce => 'try { text(await tools.mcp__miniclaw_computer__get_app_state(' +
    JSON.stringify({ nonce: String(nonce) }) + ')); }' +
    'catch (error) { text("DENIED:" + String(error)); }';
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    bodies.push(JSON.parse(body));
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const emit = value => response.write(`data: ${JSON.stringify(value)}\n\n`);
    emit({ type: 'response.created', response: { id: `probe-${bodies.length}` } });
    const output = [];
    if (!issued.has(phase)) {
      issued.add(phase);
      const item = { type: 'custom_tool_call', id: `inventory-${phase}`,
        call_id: `inventory-call-${phase}`,
        name: 'exec', namespace: 'functions',
        input: `text("PROBE_RUN_${phase}");` + code +
          (resume ? attempt(phase) : '') };
      emit({ type: 'response.output_item.added', output_index: 0, item });
      emit({ type: 'response.output_item.done', output_index: 0, item });
      output.push(item);
    } else {
      // A final answer prevents empty-response retries from shifting request indices.
      const item = { type: 'message', id: `answer-${bodies.length}`,
        role: 'assistant', phase: 'final_answer', status: 'completed',
        content: [{ type: 'output_text', text: 'Probe finished.', annotations: [] }] };
      emit({ type: 'response.output_item.added', output_index: 0, item });
      emit({ type: 'response.output_text.delta', item_id: item.id,
        output_index: 0, content_index: 0, delta: 'Probe finished.' });
      emit({ type: 'response.output_item.done', output_index: 0, item });
      output.push(item);
    }
    emit({ type: 'response.completed', response: { id: `probe-${bodies.length}`,
      output, usage: { input_tokens: 0, output_tokens: 0 } } });
    response.end();
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));
  const policy = JSON.parse(await readFile(new URL('../server/chat-policy.json', import.meta.url)));
  const config = { ...policy, model: 'gpt-6-astra', model_provider: 'probe',
    'model_providers.probe.name': 'Local test',
    'model_providers.probe.base_url': `http://127.0.0.1:${server.address().port}/v1`,
    'model_providers.probe.wire_api': 'responses',
    'model_providers.probe.requires_openai_auth': false };
  const args = ['app-server', '--listen', 'stdio://'];
  for (const [key, value] of Object.entries(config)) args.push('-c', `${key}=${JSON.stringify(value)}`);
  let child;
  t.after(() => stopChild(child));
  const pending = new Map();
  const notifications = [];
  let id = 0;
  const write = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  async function boot() {
    child = spawn('/Applications/Codex.app/Contents/Resources/codex', args, {
      cwd: home, env: { HOME: home, CODEX_HOME: home, PATH: process.env.PATH },
      stdio: ['pipe', 'pipe', 'ignore'],
    });
    const lines = createInterface({ input: child.stdout });
    lines.on('line', line => {
      const frame = JSON.parse(line);
      if (frame.method && frame.id !== undefined) {
        write({ id: frame.id, error: { code: -32601, message: 'No requests allowed' } });
      } else if (frame.id !== undefined) pending.get(frame.id)?.(frame);
      else notifications.push(frame);
    });
    await call('initialize', { clientInfo: { name: 'miniclaw_runtime_probe', version: '0.1.0' },
      capabilities: { experimentalApi: true } });
    write({ method: 'initialized' });
  }
  async function call(method, params) {
    const key = ++id;
    const result = new Promise(resolve => pending.set(key, resolve));
    write({ id: key, method, params });
    const reply = await result;
    pending.delete(key);
    assert.equal(reply.error, undefined, JSON.stringify(reply.error));
    return reply.result;
  }
  await boot();
  if (resume) {
    start.config['mcp_servers.miniclaw_computer'] = {
      command: process.execPath,
      args: [fileURLToPath(new URL('./support/fake-computer.mjs', import.meta.url)), journal],
      enabled: true, required: true,
    };
    if (live) enable.config['mcp_servers.miniclaw_computer'] = start.config['mcp_servers.miniclaw_computer'];
  }
  const { thread } = await call('thread/start', {
    ...start, ...(live ? restore : {}), cwd: home, ephemeral: !resume,
  });
  if (live) {
    // Empty tasks have no rollout yet; persist an ordinary turn before reloading.
    phase = 'bootstrap';
    await call('turn/start', { threadId: thread.id, model: 'gpt-6-astra', effort: 'low',
      environments: [], input: [{ type: 'text', text: 'Ordinary question.', text_elements: [] }] });
    await eventually(() => notifications, list => list.some(n => n.method === 'turn/completed'),
      'ordinary turn completion', 15000);
    const descendants = await call('thread/list', { ...children, ancestorThreadId: thread.id });
    assert.deepEqual(descendants.data, [], 'production descendant check accepts a childless task');
    await call('thread/archive', { threadId: thread.id });
    await call('thread/unarchive', { threadId: thread.id });
    const enabled = await call('thread/resume', {
      ...enable, threadId: thread.id, cwd: home, model: 'gpt-6-astra',
    });
    assert.equal(enabled.thread.id, thread.id, 'enable keeps the task identity');
    phase = 'initial';
    notifications.length = 0;
  }
  t.diagnostic(JSON.stringify({ source: thread.source, provider: thread.modelProvider,
    ephemeral: thread.ephemeral }));
  if (resume) {
    await call('turn/start', { threadId: thread.id, model: 'gpt-6-astra', effort: 'low',
      environments: [], input: [{ type: 'text', text: 'Saved question.', text_elements: [] }] });
    await eventually(() => notifications, list => list.some(n => n.method === 'turn/completed'),
      'saved turn completion', 15000);
    assert.match(JSON.stringify(bodies.flatMap(body => body.input).filter(item =>
      item.type.endsWith('_output') && item.call_id === 'inventory-call-initial')),
    /FAKE_COMPUTER_EXECUTED/, 'positive control must execute the fake tool');
    assert.equal(await readFile(journal, 'utf8'), 'initial\n');
    if (!live) {
      await stopChild(child);
      await boot();
    } else {
      await call('thread/archive', { threadId: thread.id });
      await call('thread/unarchive', { threadId: thread.id });
    }
    const saved = await call('thread/list', { ...listing, cwd: home,
      modelProviders: ['probe'] });
    assert.ok(saved.data.some(task => task.id === thread.id), 'task survives process restart');
    const reopened = await call('thread/resume', { ...restore, threadId: thread.id, cwd: home,
      ...(live ? { model: 'gpt-6-astra' } : {}) });
    assert.equal(reopened.thread.id, thread.id);
    const page = await call('thread/turns/list', { threadId: thread.id, limit: 50,
      sortDirection: 'desc', itemsView: 'full' });
    assert.ok(page.data.flatMap(turn => turn.items).some(item => item.type === 'userMessage' &&
      item.content.some(part => part.text === 'Saved question.')), 'saved text is readable');
  }
  const catalog = await call('mcpServerStatus/list', {
    threadId: thread.id, detail: 'toolsAndAuthOnly', limit: 100,
  });
  t.diagnostic(JSON.stringify(catalog.data.map(server => ({
    name: server.name, tools: Object.keys(server.tools), error: server.toolsError,
  }))));
  if (resume) phase = 'resumed';
  const offset = bodies.length;
  await call('turn/start', { threadId: thread.id, model: 'gpt-6-astra', effort: 'low',
    environments: [], input: [{ type: 'text', text: 'Inspect the tool inventory.', text_elements: [] }] });
  const isOutput = item => item.type.endsWith('_output') &&
    item.call_id === `inventory-call-${phase}`;
  const captured = await eventually(() => bodies.slice(offset),
    values => values.some(body => body.input.some(isOutput)), 'inventory continuation', 15000);
  const text = captured.find(body => body.input.some(isOutput)).input.filter(isOutput)
    .map(item => typeof item.output === 'string' ? item.output :
      (item.output ?? []).map(part => part.text ?? '').join('\n')).join('\n');
  if (resume) {
    t.diagnostic(text);
    const calls = await readFile(journal, 'utf8');
    t.diagnostic(`Fake helper journal: ${JSON.stringify(calls)}`);
    assert.equal(calls, 'initial\n', 'disabled tool must not execute after resume');
    assert.doesNotMatch(text, /FAKE_COMPUTER_EXECUTED/);
    assert.match(text, /DENIED:/);
  }
  const match = [...text.matchAll(/MINICLAW_INVENTORY:(\{[^\n]*\})/g)].at(-1);
  assert.match(text, new RegExp(`PROBE_RUN_${phase}`));
  assert.ok(match, text);
  const inventory = JSON.parse(match[1]);
  t.diagnostic(JSON.stringify(inventory));
  assert.equal(inventory.process, 'undefined');
  assert.equal(inventory.require, 'undefined');
  const operations = ['list_apps', 'get_app_state', 'click', 'perform_secondary_action',
    'set_value', 'select_text', 'scroll', 'drag', 'press_key', 'type_text'];
  // Codex adds resource lookup wrappers when MCP is enabled; only our server is loaded.
  const enabled = computerUse && !resume;
  if (resume) {
    assert.ok(catalog.data.every(server => Object.keys(server.tools).length === 0));
  } else {
    assert.deepEqual(catalog.data.map(server => server.name), enabled ? ['miniclaw_computer'] : []);
  }
  const allowed = ['clock__curr_time', ...enabled ? [
    ...operations.map(name => `mcp__miniclaw_computer__${name}`),
    'list_mcp_resources', 'list_mcp_resource_templates', 'read_mcp_resource',
  ] : []];
  assert.deepEqual(inventory.tools.filter(name => !allowed.includes(name)), []);
  if (enabled) assert.ok(inventory.tools.includes('mcp__miniclaw_computer__get_app_state'));
});
}
