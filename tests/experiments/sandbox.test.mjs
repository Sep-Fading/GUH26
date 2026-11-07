// Checks actual canary files and replies.
import assert from 'node:assert/strict';
// Runs sandbox hypotheses as separate cases.
import { test } from 'node:test';
// Starts only the installed Codex runtime.
import { spawn } from 'node:child_process';
// Waits for local inference server readiness.
import { once } from 'node:events';
// Supplies deterministic responses without paid inference.
import { createServer } from 'node:http';
// Reads canaries inside disposable experiment directories.
import { mkdtemp, readFile, rm } from 'node:fs/promises';
// Locates temporary directories without user data.
import { tmpdir } from 'node:os';
// Builds paths within the disposable directory.
import { join } from 'node:path';
// Parses newline-delimited responses from Codex stdout.
import { createInterface } from 'node:readline';
// Converts fixture URLs into absolute paths.
import { fileURLToPath } from 'node:url';
// Reuses bounded waits and subprocess shutdown.
import { eventually, stopChild } from '../support/harness.mjs';

const codex = process.env.CODEX_BIN ?? '/Applications/Codex.app/Contents/Resources/codex';
const helper = fileURLToPath(new URL('./support/canary.mjs', import.meta.url));

// Starts a private stdio client and declines every server-initiated request.
async function connect(home, config) {
  const args = ['app-server', '--listen', 'stdio://'];
  for (const [key, value] of Object.entries(config)) {
    args.push('-c', `${key}=${JSON.stringify(value)}`);
  }
  const child = spawn(codex, args, { cwd: home,
    env: { HOME: home, CODEX_HOME: home, PATH: process.env.PATH },
    stdio: ['pipe', 'pipe', 'pipe'] });
  const pending = new Map();
  const notifications = [];
  const approvals = [];
  let nextId = 0;
  let failure;
  let stderr = '';
  // Sends one bounded experiment frame to the owned child.
  const send = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  // Rejects outstanding calls when the transport becomes unavailable.
  const fail = error => {
    failure = error;
    for (const entry of pending.values()) entry.reject(error);
  };
  child.on('error', fail);
  child.on('exit', () => fail(new Error(`Probe child exited: ${stderr}`)));
  child.stdin.on('error', fail);
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-8192); });
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    try {
      const frame = JSON.parse(line);
      if (frame.method && frame.id !== undefined) {
        approvals.push(frame.method);
        send({ id: frame.id, error: { code: -32601, message: 'Experiment declines requests' } });
      } else if (frame.id !== undefined) pending.get(frame.id)?.resolve(frame);
      else notifications.push(frame);
    } catch (error) { fail(error); }
  });
  // Correlates an RPC reply and rejects errors or expired calls.
  async function call(method, params) {
    if (failure) throw failure;
    const id = ++nextId;
    let timer;
    try {
      const response = new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        timer = setTimeout(() => reject(new Error(`${method} timed out`)), 15_000);
      });
      send({ id, method, params });
      const frame = await response;
      assert.equal(frame.error, undefined, JSON.stringify(frame.error));
      return frame.result;
    } finally {
      clearTimeout(timer);
      pending.delete(id);
    }
  }
  try {
    await call('initialize', { clientInfo: { name: 'miniclaw_sandbox_probe', version: '0.1.0' },
      capabilities: { experimentalApi: true } });
    send({ method: 'initialized' });
    return { child, call, notifications, approvals };
  } catch (error) {
    await stopChild(child);
    throw error;
  }
}

// Emits one canary tool request, then a final reply to avoid empty-response retries.
function inference(bodies, nonce) {
  let issued = false;
  return createServer(async (request, response) => {
    try {
      let text = '';
      for await (const chunk of request) {
        text += chunk;
        if (text.length > 4 * 1024 * 1024) throw new Error('Probe request too large');
      }
      bodies.push(JSON.parse(text));
      const item = !issued ? {
        type: 'custom_tool_call', id: nonce, call_id: nonce,
        name: 'exec', namespace: 'functions',
        input: `text(await tools.mcp__canary__write_marker(${JSON.stringify({ nonce })}));`,
      } : {
        type: 'message', id: `${nonce}-answer`, role: 'assistant',
        phase: 'final_answer', status: 'completed',
        content: [{ type: 'output_text', text: 'Probe complete.', annotations: [] }],
      };
      issued = true;
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      for (const event of [
        { type: 'response.created', response: { id: `response-${bodies.length}` } },
        { type: 'response.output_item.added', output_index: 0, item },
        { type: 'response.output_item.done', output_index: 0, item },
        { type: 'response.completed', response: { id: `response-${bodies.length}`,
          output: [item], usage: { input_tokens: 0, output_tokens: 0, total_tokens: 0 } } },
      ]) response.write(`data: ${JSON.stringify(event)}\n\n`);
      response.end();
    } catch (error) {
      response.writeHead(500);
      response.end(String(error));
    }
  });
}

// Distinguishes an absent canary from unrelated filesystem errors.
async function canary(path) {
  try { return await readFile(path, 'utf8'); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}

for (const [name, sandbox, access, denied, advertised] of [
  ['MCP canary positive control', 'workspace-write', 'write', false, 'read-only'],
  ['read-only sandbox constrains read-only-labelled MCP', 'read-only', 'write', true, 'read-only'],
  ['executor denial blocks MCP writes', 'read-only', 'read-only', true, 'read-only'],
  ['write-labelled MCP requires approval', 'read-only', 'write', true, 'write'],
]) {
  test(name, { timeout: 45_000 }, async t => {
    const dir = await mkdtemp(join(tmpdir(), 'mc-mcp-sandbox-'));
    const bodies = [];
    const nonce = 'canary-request';
    const server = inference(bodies, nonce);
    let client;
    t.after(async () => {
      if (client) await stopChild(client.child);
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
      await rm(dir, { recursive: true, force: true });
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const policy = JSON.parse(await readFile(
      new URL('../../server/chat-policy.json', import.meta.url), 'utf8',
    ));
    delete policy.mcp_servers;
    client = await connect(dir, { ...policy, model: 'gpt-6-astra',
      model_provider: 'probe', 'model_providers.probe.name': 'Disposable canary',
      'model_providers.probe.base_url': `http://127.0.0.1:${server.address().port}/v1`,
      'model_providers.probe.wire_api': 'responses',
      'model_providers.probe.requires_openai_auth': false,
      'orchestrator.mcp.enabled': true,
      'mcp_servers.canary.command': process.execPath,
      'mcp_servers.canary.args': [helper, dir, access, advertised],
      'mcp_servers.canary.enabled': true,
      'mcp_servers.canary.required': true,
    });
    const { thread, sandbox: effective } = await client.call('thread/start', {
      cwd: dir, ephemeral: true, environments: [], approvalPolicy: 'never', sandbox,
    });
    assert.equal(effective.type, sandbox === 'read-only' ? 'readOnly' : 'workspaceWrite',
      'runtime must acknowledge requested sandbox');
    const catalog = await client.call('mcpServerStatus/list', {
      threadId: thread.id, detail: 'toolsAndAuthOnly', limit: 100,
    });
    assert.deepEqual(catalog.data.map(s => s.name), ['canary']);
    assert.deepEqual(Object.keys(catalog.data[0].tools), ['write_marker']);
    await client.call('turn/start', { threadId: thread.id, model: 'gpt-6-astra',
      effort: 'low', environments: [], input: [{ type: 'text',
        text: 'Run the disposable canary probe.', text_elements: [] }] });
    await eventually(() => client.notifications,
      values => values.some(n => n.method === 'turn/completed'), 'probe completion', 25_000);
    const completed = client.notifications.find(n => n.method === 'turn/completed');
    assert.equal(completed.params.turn.status, 'completed',
      JSON.stringify({ turn: completed.params.turn, requests: bodies.length,
        errors: client.notifications.filter(n => n.method === 'error') }));
    const outputs = bodies.flatMap(b => b.input ?? []).filter(item =>
      item.call_id === nonce && item.type.endsWith('_output'));
    assert.ok(outputs.length, 'model continuation must include this tool result');
    if (advertised === 'write') {
      assert.match(JSON.stringify(outputs), /requires approval/);
      assert.equal(await canary(join(dir, 'canary.txt')), null);
      assert.equal(await canary(join(dir, 'journal.jsonl')), null,
        'approval denial must occur before invoking the helper');
      t.diagnostic('Write-labelled tool denied before helper invocation.');
      return;
    }
    assert.match(JSON.stringify(outputs), /CANARY_(WRITTEN|DENIED)/,
      'helper must return a canary result, not a transport or tool lookup failure');
    const journal = JSON.parse((await readFile(join(dir, 'journal.jsonl'), 'utf8')).trim());
    assert.equal(journal.nonce, nonce, 'independent helper journal must observe the call');
    const value = await canary(join(dir, 'canary.txt'));
    t.diagnostic(JSON.stringify({ sandbox, access, advertised, approvals: client.approvals,
      helperOutcome: journal.outcome, canaryWritten: value !== null }));
    assert.deepEqual(client.approvals, [], 'no approval is granted by this experiment');
    if (denied) {
      assert.equal(value, null, 'read-only access must leave the canary absent');
      assert.equal(journal.outcome, 'denied');
      assert.match(JSON.stringify(outputs), /CANARY_DENIED/);
    } else {
      assert.equal(value, nonce);
      assert.equal(journal.outcome, 'written');
      assert.match(JSON.stringify(outputs), /CANARY_WRITTEN/);
    }
  });
}

// Native discovery is opt-in and never calls an app or starts a model turn.
if (process.env.MINICLAW_PROBE_NATIVE === '1') {
  test('native descriptors expose target fields', { timeout: 30_000 }, async t => {
    const native = process.env.MINICLAW_HELPER_BIN;
    assert.ok(native, 'Set MINICLAW_HELPER_BIN explicitly for native discovery');
    const dir = await mkdtemp(join(tmpdir(), 'mc-native-descriptors-'));
    let client;
    t.after(async () => {
      if (client) await stopChild(client.child);
      await rm(dir, { recursive: true, force: true });
    });
    const policy = JSON.parse(await readFile(
      new URL('../../server/chat-policy.json', import.meta.url), 'utf8',
    ));
    delete policy.mcp_servers;
    client = await connect(dir, { ...policy, 'features.computer_use': true,
      'orchestrator.mcp.enabled': true,
      'mcp_servers.miniclaw_computer.command': fileURLToPath(new URL(
        '../../target/debug/miniclaw-server', import.meta.url,
      )),
      'mcp_servers.miniclaw_computer.args': ['--computer-helper', native],
      'mcp_servers.miniclaw_computer.enabled': true,
      'mcp_servers.miniclaw_computer.required': true,
    });
    const { thread } = await client.call('thread/start', {
      cwd: dir, ephemeral: true, environments: [], sandbox: 'read-only',
      approvalPolicy: 'never',
    });
    const catalog = await client.call('mcpServerStatus/list', {
      threadId: thread.id, detail: 'toolsAndAuthOnly', limit: 100,
    });
    assert.deepEqual(catalog.data.map(s => s.name), ['miniclaw_computer']);
    const tools = catalog.data[0].tools;
    assert.ok(tools.get_app_state && tools.click && tools.type_text);
    const descriptors = Object.entries(tools).map(([name, tool]) => ({
      name, fields: Object.keys(tool.inputSchema?.properties ?? {}),
      required: tool.inputSchema?.required ?? [], annotations: tool.annotations,
    }));
    t.diagnostic(JSON.stringify(descriptors));
    assert.ok(descriptors.every(d => d.name === 'list_apps' || d.fields.includes('app')),
      'each app operation must expose its requested app for a potential broker');
    assert.deepEqual(client.approvals, []);
  });
}
