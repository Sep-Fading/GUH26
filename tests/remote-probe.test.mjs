import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { stopChild } from './support/harness.mjs';

// Protocol inspection only. Never enable Remote, create a pairing, or start a turn.
test('remote host API is readable while disabled', { timeout: 20000 }, async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-remote-'));
  const binary = process.env.CODEX_BIN ?? '/Applications/Codex.app/Contents/Resources/codex';
  const env = { HOME: home, CODEX_HOME: home, PATH: process.env.PATH };
  let child;
  t.after(async () => {
    if (child) await stopChild(child);
    await rm(home, { recursive: true, force: true });
  });
  const schemaDir = join(home, 'schema');
  await promisify(execFile)(binary, ['app-server', 'generate-json-schema',
    '--experimental', '--out', schemaDir], { env, cwd: home, timeout: 10000 });
  const schema = JSON.parse(await readFile(join(schemaDir, 'ClientRequest.json'), 'utf8'));
  const methods = schema.oneOf.flatMap(item => item.properties?.method?.enum ?? []);
  const remote = methods.filter(method => method.startsWith('remoteControl/')).sort();
  t.diagnostic(JSON.stringify({ methods: remote }));
  assert.ok(remote.includes('remoteControl/status/read'));
  assert.ok(remote.includes('remoteControl/pairing/start'));
  assert.ok(remote.includes('remoteControl/pairing/status'));

  const policy = JSON.parse(await readFile(new URL('../server/chat-policy.json', import.meta.url)));
  const args = ['app-server', '--listen', 'stdio://'];
  for (const [key, value] of Object.entries({ ...policy, 'features.remote_control': false })) {
    args.push('-c', `${key}=${JSON.stringify(value)}`);
  }
  child = spawn(binary, args, { env, cwd: home, stdio: ['pipe', 'pipe', 'ignore'] });
  const pending = new Map();
  let id = 0;
  const send = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  const fail = error => { for (const entry of pending.values()) entry.reject(error); };
  child.on('error', fail);
  child.on('exit', () => fail(new Error('Probe child exited')));
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    try {
      const frame = JSON.parse(line);
      if (frame.method && frame.id !== undefined) {
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
      const result = new Promise((resolve, reject) => {
        pending.set(key, { resolve, reject });
        timer = setTimeout(() => reject(new Error(`${method} timed out`)), 5000);
      });
      send({ id: key, method, params });
      const reply = await result;
      assert.equal(reply.error, undefined, JSON.stringify(reply.error));
      return reply.result;
    } finally {
      clearTimeout(timer);
      pending.delete(key);
    }
  }
  await call('initialize', { clientInfo: { name: 'miniclaw_remote_probe', version: '0.1.0' },
    capabilities: { experimentalApi: true } });
  send({ method: 'initialized' });
  const status = await call('remoteControl/status/read', {});
  assert.equal(status.status, 'disabled');
  assert.equal(status.environmentId, null);
  t.diagnostic('Remote remains disabled; no environment registered or pairing requested.');
});
