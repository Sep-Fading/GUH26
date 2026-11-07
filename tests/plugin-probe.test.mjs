import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, mkdir, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { stopChild } from './support/harness.mjs';

// Installs only into a disposable home. No shared config writes or model calls.
const adapted = process.env.MINICLAW_PROBE_ADAPTER === '1';
test(`native list_apps: adapter=${adapted}`, { timeout: 45_000 }, async t => {
  const serverName = adapted ? 'miniclaw_computer' : 'computer-use';
  const cwd = await mkdtemp(join(tmpdir(), 'mc-plugin-'));
  let child;
  t.after(async () => {
    if (child) await stopChild(child);
    await rm(cwd, { recursive: true, force: true });
  });
  const policy = JSON.parse(await readFile(
    new URL('../server/chat-policy.json', import.meta.url), 'utf8',
  ));
  const args = ['app-server', '--listen', 'stdio://'];
  policy['features.plugins'] = true;
  policy['features.computer_use'] = true;
  policy['orchestrator.mcp.enabled'] = true;
  for (const [key, value] of Object.entries(policy)) {
    args.push('-c', `${key}=${JSON.stringify(value)}`);
  }
  child = spawn(process.env.CODEX_BIN ??
    '/Applications/Codex.app/Contents/Resources/codex', args, {
    cwd, env: { HOME: cwd, CODEX_HOME: cwd,
      PATH: process.env.PATH }, stdio: ['pipe', 'pipe', 'ignore'],
  });
  const pending = new Map();
  let id = 0;
  const send = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  const fail = error => {
    for (const entry of pending.values()) entry.reject(error);
  };
  child.on('error', fail);
  child.on('exit', () => fail(new Error('Probe child exited')));
  createInterface({ input: child.stdout }).on('line', line => {
    try {
      const frame = JSON.parse(line);
      if (frame.method && frame.id !== undefined) {
        t.diagnostic(`Declined server request: ${frame.method}`);
        send({ id: frame.id, error: { code: -32601, message: 'Probe declines requests' } });
      } else if (frame.id !== undefined) {
        pending.get(frame.id)?.resolve(frame);
      }
    } catch (error) { fail(error); }
  });
  async function call(method, params) {
    const key = ++id;
    let timer;
    try {
      const reply = new Promise((resolve, reject) => {
        pending.set(key, { resolve, reject });
        timer = setTimeout(() => reject(new Error(`${method} timed out`)), 15_000);
      });
      send({ id: key, method, params });
      const frame = await reply;
      assert.equal(frame.error, undefined, JSON.stringify(frame.error));
      return frame.result;
    } finally {
      clearTimeout(timer);
      pending.delete(key);
    }
  }
  await call('initialize', {
    clientInfo: { name: 'miniclaw_plugin_probe', version: '0.1.0' },
    capabilities: { experimentalApi: true },
  });
  send({ method: 'initialized' });
  const bundled = '/Applications/Codex.app/Contents/Resources/plugins/openai-bundled';
  const marketplace = join(cwd, '.tmp/bundled-marketplaces/openai-bundled');
  await mkdir(join(marketplace, '.agents/plugins'), { recursive: true });
  await mkdir(join(marketplace, 'plugins'), { recursive: true });
  await symlink(join(bundled, '.agents/plugins/marketplace.json'),
    join(marketplace, '.agents/plugins/marketplace.json'));
  await symlink(join(bundled, 'plugins/computer-use'),
    join(marketplace, 'plugins/computer-use'));
  await mkdir(join(cwd, 'computer-use'));
  await symlink(join(process.env.CODEX_HOME ?? join(process.env.HOME, '.codex'),
    'computer-use/Codex Computer Use.app'),
  join(cwd, 'computer-use/Codex Computer Use.app'));
  await call('marketplace/add', { source: marketplace });
  await call('plugin/install', {
    marketplacePath: join(marketplace, '.agents/plugins/marketplace.json'),
    pluginName: 'computer-use',
  });
  await call('config/mcpServer/reload', {});
  const { thread } = await call('thread/start', {
    cwd, ephemeral: true, environments: [],
    approvalPolicy: 'never', sandbox: 'read-only',
    ...(adapted ? { config: {
      'features.plugins': false,
      'mcp_servers.miniclaw_computer': {
        command: new URL('../target/debug/miniclaw-server', import.meta.url).pathname,
        args: ['--computer-helper', join(cwd, 'computer-use/Codex Computer Use.app',
          'Contents/SharedSupport/SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient')],
        enabled: true, required: true,
      },
    } } : {}),
  });
  const catalog = await call('mcpServerStatus/list', {
    threadId: thread.id, detail: 'toolsAndAuthOnly', limit: 100,
  });
  t.diagnostic(JSON.stringify(catalog.data.map(server => ({
    name: server.name, tools: Object.keys(server.tools), error: server.toolsError,
    status: server.runtimeStatus, plugin: server.pluginId,
  }))));
  const active = catalog.data.filter(server => Object.keys(server.tools).length > 0);
  assert.deepEqual(active.map(server => server.name), [serverName]);
  const result = await call('mcpServer/tool/call', {
    threadId: thread.id, server: serverName, tool: 'list_apps', arguments: {},
  });
  assert.notEqual(result.isError, true, JSON.stringify(result));
  assert.ok(result.content?.length, 'Expected native response content');
  t.diagnostic('Native list_apps returned content; app names omitted.');
});
