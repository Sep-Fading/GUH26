import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import test from 'node:test';

const helper = process.env.MINICLAW_HELPER_BIN;
const viaCodex = process.env.MINICLAW_HELPER_VIA_CODEX === '1';
const mode = viaCodex ? 'App Server' : 'direct MCP';

test(`helper discovery through ${mode}`, { timeout: 15_000 }, async (t) => {
  assert.ok(helper, 'Set MINICLAW_HELPER_BIN to the installed native client');
  const home = await mkdtemp(join(tmpdir(), 'miniclaw-helper-'));
  const executable = viaCodex
    ? process.env.CODEX_BIN ?? '/Applications/Codex.app/Contents/Resources/codex'
    : helper;
  const args = viaCodex ? [
    'app-server', '--listen', 'stdio://',
    '-c', `mcp_servers.helper.command=${JSON.stringify(helper)}`,
    '-c', 'mcp_servers.helper.args=["mcp"]',
    '-c', 'mcp_servers.helper.startup_timeout_sec=5',
    '-c', 'features.plugins=false',
    '-c', 'agents.enabled=false',
    '-c', 'analytics.enabled=false',
  ] : ['mcp'];
  const child = spawn(executable, args, {
    env: { HOME: home, CODEX_HOME: home, PATH: '/usr/bin:/bin' },
    cwd: home,
    stdio: ['pipe', 'pipe', 'pipe'],
    detached: true,
  });
  const closed = once(child, 'close');
  const pending = new Map();
  const lines = createInterface({ input: child.stdout });
  let nextId = 0;
  let bytes = 0;
  let stderr = '';
  let failure;

  function fail(error) {
    failure = error;
    for (const waiter of pending.values()) waiter.reject(error);
    pending.clear();
  }

  function send(message) {
    child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  function request(method, params) {
    if (failure) return Promise.reject(failure);
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      send({ jsonrpc: '2.0', id, method, params });
    });
  }

  child.on('error', fail);
  child.stdin.on('error', fail);
  child.stderr.on('data', (chunk) => {
    stderr = (stderr + chunk.toString()).slice(-4000);
  });
  child.stdout.on('data', (chunk) => {
    bytes += chunk.length;
    if (bytes > 1_048_576) fail(new Error('MCP output exceeded 1 MiB'));
  });
  child.on('close', (code, signal) => {
    fail(new Error(`Helper closed: ${code ?? signal}; ${stderr}`));
  });
  lines.on('line', (line) => {
    try {
      const message = JSON.parse(line);
      // Discovery must never grant permissions or execute server requests.
      if (message.method && message.id !== undefined) {
        send({
          jsonrpc: '2.0',
          id: message.id,
          error: { code: -32601, message: 'Probe does not handle requests' },
        });
        return;
      }
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
      else waiter.resolve(message.result);
    } catch (error) {
      fail(error);
    }
  });
  const deadline = setTimeout(() => fail(new Error('MCP discovery timed out')), 8000);

  try {
    const params = {
      protocolVersion: process.env.MINICLAW_MCP_VERSION ?? '2024-11-05',
      capabilities: JSON.parse(process.env.MINICLAW_MCP_CAPABILITIES ?? '{}'),
      clientInfo: { name: 'miniclaw-helper-probe', version: '0.1.0' },
    };
    if (viaCodex) delete params.protocolVersion;
    const initialized = await request('initialize', params);
    let tools;
    if (viaCodex) {
      send({ method: 'initialized' });
      const catalog = await request('mcpServerStatus/list', {
        detail: 'toolsAndAuthOnly', limit: 100,
      });
      const server = catalog.data.find((entry) => entry.name === 'helper');
      assert.ok(server, 'Expected the configured helper');
      assert.equal(server.toolsError ?? null, null, server.toolsError);
      t.diagnostic(JSON.stringify({ server: server.serverInfo }));
      tools = Object.values(server.tools);
    } else {
      assert.ok(initialized.serverInfo?.name);
      t.diagnostic(JSON.stringify({ server: initialized.serverInfo }));
      send({ jsonrpc: '2.0', method: 'notifications/initialized' });
      tools = (await request('tools/list', {})).tools;
    }
    assert.ok(Array.isArray(tools));
    assert.ok(tools.length > 0, 'Expected discoverable tools');
    for (const name of ['get_app_state', 'click', 'type_text']) {
      assert.ok(tools.some((tool) => tool.name === name), `Missing ${name}`);
    }
    t.diagnostic(JSON.stringify({ tools: tools.map((tool) => tool.name) }));
  } finally {
    clearTimeout(deadline);
    lines.close();
    child.stdin.end();
    if (child.pid && child.exitCode === null && child.signalCode === null) {
      process.kill(-child.pid, 'SIGTERM');
    }
    const kill = setTimeout(() => {
      try { process.kill(-child.pid, 'SIGKILL'); } catch {}
    }, 1000);
    await closed.catch(() => {});
    clearTimeout(kill);
    await rm(home, { recursive: true, force: true });
  }
});
