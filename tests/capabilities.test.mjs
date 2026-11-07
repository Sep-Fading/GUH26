import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { stopChild } from './support/harness.mjs';

// Real Codex, disposable credentials, local fake inference. No model service call.
const cases = JSON.parse(process.env.MINICLAW_CAPABILITY_CASES ??
  '[{"model":"gpt-5.5","effort":"medium"}]');
// The probe runs the user's installed Codex; only macOS has a fixed default location.
const skip = process.platform === 'win32' && !process.env.CODEX_BIN
  && 'needs a real installed Codex: the default is /Applications/Codex.app on macOS; set CODEX_BIN to run it on Windows';
for (const { model, effort } of cases) {
test(`chat has no environment tools: ${model}/${effort}`, { timeout: 45000, skip }, async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-cap-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  let received;
  const request = new Promise(resolve => { received = resolve; });
  const server = createServer(async (req, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    received(JSON.parse(body));
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.end('data: {"type":"response.completed","response":{"id":"probe","output":[],"usage":{"input_tokens":0,"output_tokens":0}}}\n\n');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));
  const policy = JSON.parse(await readFile(
    new URL('../server/chat-policy.json', import.meta.url), 'utf8',
  ));
  const config = {
    ...policy,
    model,
    model_provider: 'probe',
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
    // Windows sockets need SystemRoot; the profile folder is the disposable home there too.
    env: { PATH: process.env.PATH, HOME: home, CODEX_HOME: home,
      ...(process.platform === 'win32' && { SystemRoot: process.env.SystemRoot, USERPROFILE: home }) },
    stdio: ['pipe', 'pipe', 'ignore'],
  });
  t.after(() => stopChild(child));
  const pending = new Map();
  const failed = new Promise((_, reject) => {
    child.on('error', reject);
    child.on('exit', code => reject(new Error(`Codex exited: ${code}`)));
  });
  failed.catch(() => {});
  let id = 0;
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    const reply = JSON.parse(line);
    if (reply.id != null) pending.get(reply.id)?.(reply);
  });
  async function call(method, params) {
    const key = ++id;
    const result = new Promise(resolve => pending.set(key, resolve));
    child.stdin.write(`${JSON.stringify({ id: key, method, params })}\n`);
    const reply = await Promise.race([result, failed]);
    assert.equal(reply.error, undefined, JSON.stringify(reply.error));
    return reply.result;
  }
  await call('initialize', {
    clientInfo: { name: 'miniclaw_probe', version: '0.1.0' },
    capabilities: { experimentalApi: true },
  });
  child.stdin.write('{"method":"initialized"}\n');
  const started = await call('thread/start', {
    cwd: home, ephemeral: true, environments: [],
    sandbox: 'read-only', approvalPolicy: 'never',
  });
  await call('turn/start', {
    threadId: started.thread.id, environments: [], model, effort,
    input: [{ type: 'text', text: 'Hello', text_elements: [] }],
  });
  const body = await Promise.race([request, failed]);
  // Astra can carry tool definitions in input items rather than the top-level field.
  const tools = [...(body.tools ?? []), ...(body.input ?? [])
    .filter(item => item.type === 'additional_tools').flatMap(item => item.tools)];
  assert.ok(tools.length, 'Probe must inspect actual tool definitions');
  if (process.env.MINICLAW_TOOL_DIAGNOSTICS === '1') {
    t.diagnostic(JSON.stringify(tools));
  }
  const names = tools.flatMap(tool =>
    tool.type === 'namespace' ? tool.tools.map(item => item.name) :
      [tool.name ?? tool.type],
  );
  assert.deepEqual(names.filter(name => !['update_plan', 'get_current_time']
    .includes(name)), [], `Unexpected tools: ${JSON.stringify(names)}`);
});
}
