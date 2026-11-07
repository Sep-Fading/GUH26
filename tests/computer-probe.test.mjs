import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { eventually, stopChild } from './support/harness.mjs';

// Real Codex protocol, fake inference and image. No desktop access or real credentials.
test('Astra tools can return screen images', { timeout: 30000 }, async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-computer-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const image = 'data:image/png;base64,' +
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=';
  const requests = [];
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    requests.push(JSON.parse(body));
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const emit = event => response.write(`data: ${JSON.stringify(event)}\n\n`);
    emit({ type: 'response.created', response: { id: `probe-${requests.length}` } });
    if (requests.length === 1) {
      const item = { type: 'function_call', id: 'item-screen',
        call_id: 'call-screen', name: 'miniclaw_screen', arguments: '{}' };
      emit({ type: 'response.output_item.added', output_index: 0, item });
      emit({ type: 'response.output_item.done', output_index: 0, item });
    }
    emit({ type: 'response.completed', response: {
      id: `probe-${requests.length}`, output: [],
      usage: { input_tokens: 0, output_tokens: 0 },
    } });
    response.end();
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));

  const policy = JSON.parse(await readFile(
    new URL('../server/chat-policy.json', import.meta.url), 'utf8',
  ));
  const config = {
    ...policy, model: 'gpt-6-astra', model_provider: 'probe',
    'model_providers.probe.name': 'Local test',
    'model_providers.probe.base_url':
      `http://127.0.0.1:${server.address().port}/v1`,
    'model_providers.probe.wire_api': 'responses',
    'model_providers.probe.requires_openai_auth': false,
  };
  const args = ['app-server', '--listen', 'stdio://'];
  for (const [key, value] of Object.entries(config)) {
    args.push('-c', `${key}=${JSON.stringify(value)}`);
  }
  const child = spawn(process.env.CODEX_BIN ??
    '/Applications/Codex.app/Contents/Resources/codex', args, {
    cwd: home,
    env: { PATH: process.env.PATH, HOME: home, CODEX_HOME: home },
    stdio: ['pipe', 'pipe', 'ignore'],
  });
  t.after(() => stopChild(child));
  const pending = new Map();
  const incoming = [];
  let id = 0;
  const failed = new Promise((_, reject) => {
    child.on('error', reject);
    child.on('exit', code => reject(new Error(`Codex exited: ${code}`)));
    t.signal.addEventListener('abort', () => reject(new Error('Probe timed out')),
      { once: true });
  });
  failed.catch(() => {});
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    const frame = JSON.parse(line);
    if (frame.method) incoming.push(frame);
    else if (pending.has(frame.id)) {
      pending.get(frame.id)(frame);
      pending.delete(frame.id);
    }
  });
  const write = value => child.stdin.write(`${JSON.stringify(value)}\n`);
  async function call(method, params) {
    const key = ++id;
    const response = new Promise(resolve => pending.set(key, resolve));
    write({ id: key, method, params });
    const reply = await Promise.race([response, failed]);
    assert.equal(reply.error, undefined, JSON.stringify(reply.error));
    return reply.result;
  }

  await call('initialize', {
    clientInfo: { name: 'miniclaw_computer_probe', version: '0.1.0' },
    capabilities: { experimentalApi: true },
  });
  write({ method: 'initialized' });
  const { thread } = await call('thread/start', {
    cwd: home, ephemeral: true, environments: [],
    sandbox: 'read-only', approvalPolicy: 'never',
    dynamicTools: [{ type: 'function', name: 'miniclaw_screen',
      description: 'Return a synthetic test screen. No computer access.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    }],
  });
  await call('turn/start', {
    threadId: thread.id, environments: [], model: 'gpt-6-astra', effort: 'low',
    input: [{ type: 'text', text: 'Inspect the test screen.', text_elements: [] }],
  });
  await eventually(() => incoming, frames => frames.some(frame =>
    frame.method === 'item/tool/call'), 'dynamic tool request', 10000);
  const tool = incoming.find(frame => frame.method === 'item/tool/call');
  assert.equal(tool.params.threadId, thread.id);
  assert.equal(tool.params.tool, 'miniclaw_screen');
  assert.equal(tool.params.callId, 'call-screen');
  assert.deepEqual(tool.params.arguments, {});
  write({ id: tool.id, result: { success: true, contentItems: [
    { type: 'inputText', text: 'Synthetic screen, 1 by 1 pixels.' },
    { type: 'inputImage', imageUrl: image },
  ] } });

  await eventually(() => requests, values => values.length >= 2,
    'continuation with tool image', 10000);
  assert.equal(requests[0].model, 'gpt-6-astra');
  assert.equal(requests[0].reasoning.effort, 'low');
  const tools = [...(requests[0].tools ?? []), ...requests[0].input
    .filter(item => item.type === 'additional_tools').flatMap(item => item.tools)];
  const definitions = tools.flatMap(tool =>
    tool.type === 'namespace' ? tool.tools : [tool]);
  await t.test('tool definition and image survive the round trip', () => {
    assert.ok(definitions.some(tool => tool.name === 'miniclaw_screen' ||
      (tool.name === 'exec' && tool.description.includes('miniclaw_screen(args:'))));
    const output = requests[1].input.find(item =>
      item.type === 'function_call_output' && item.call_id === 'call-screen');
    assert.ok(output, 'Tool result must keep its original call ID');
    assert.ok(output.output.some(item =>
      item.type === 'input_image' && item.image_url === image), JSON.stringify(output));
  });
  await t.test('screen access does not enable unrelated tools', () => {
    const allowed = ['miniclaw_screen', 'update_plan', 'get_current_time', 'exec', 'wait'];
    assert.deepEqual(definitions.map(tool => tool.name ?? tool.type)
      .filter(name => !allowed.includes(name)), []);
  });
});
