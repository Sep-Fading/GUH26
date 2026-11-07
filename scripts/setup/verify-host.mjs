#!/usr/bin/env node
// Checks the native Windows installation without VM dependencies or implicit model calls.
import { readFile } from 'node:fs/promises';
import { join, resolve, win32 } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { defaultInstallation } from './default-installation.mjs';

// Reports each independent check; service liveness alone never certifies isolation.
export async function verifyHost({ api, service, chat = false, wait = ms => new Promise(resolve => setTimeout(resolve, ms)) }) {
  const results = [];
  const check = async (name, run) => {
    try { await run(); results.push({ state: 'pass', name }); }
    catch (error) { results.push({ state: 'FAIL', name, detail: error.message }); }
  };
  await check('backend responds', async () => {
    if ((await api('/api/health')).backend !== 'ok') throw new Error('Backend is not ready');
  });
  await check('provider is signed in', async () => {
    if ((await api('/api/account')).status !== 'signed_in') throw new Error('Sign in to an enabled provider');
  });
  await check('network service is running without cleanup errors', async () => {
    const state = await service();
    if (state.state !== 'Running' || state.exitCode !== 0 || state.serviceSpecificExitCode !== 0) throw new Error('Service is stopped or reports incomplete cleanup');
  });
  results.push({ state: 'skip', name: 'native sandbox and desktop interaction acceptance',
    detail: 'Requires native boundary tests and interactive app approvals; service liveness is not proof.' });
  if (chat) {
    let thread;
    await check('one real host model reply', async () => {
      const models = (await api('/api/models')).models;
      const model = models.find(entry => /luna|haiku/i.test(entry.model)) ?? models.find(entry => !/astra|opus/i.test(entry.model));
      if (!model) throw new Error('No small model is offered');
      thread = (await api('/api/threads', {})).thread;
      await api(`/api/threads/${thread.id}/messages`, { text: 'Reply with exactly: HOST OK',
        clientMessageId: crypto.randomUUID(), model: model.model, effort: 'low' });
      for (let tries = 0; tries < 120; tries++) {
        await wait(1000);
        const snapshot = await api(`/api/threads/${thread.id}`);
        if (!snapshot.activeTurn && snapshot.messages.at(-1)?.role === 'assistant') {
          if (!/HOST OK/i.test(snapshot.messages.at(-1)?.text ?? '')) throw new Error('Unexpected model reply');
          return;
        }
      }
      throw new Error('No model reply in two minutes');
    });
    if (thread) await check('verification chat removed', () => api(`/api/threads/${thread.id}/delete`, {}));
  }
  return results;
}

// Reads only the service identity for an SCM status query; never prints its client credential.
async function serviceStatus(data) {
  const config = JSON.parse(await readFile(join(data, 'windows-network.json'), 'utf8'));
  if (!/^MiniClawNetwork-[a-f0-9]{32}$/.test(config.service_name)) throw new Error('Invalid service identity');
  const command = `$s=Get-CimInstance Win32_Service -Filter "Name='${config.service_name}'"; if (!$s) { throw 'Service missing' }; @{state=$s.State;exitCode=$s.ExitCode;serviceSpecificExitCode=$s.ServiceSpecificExitCode} | ConvertTo-Json -Compress`;
  const { stdout } = await promisify(execFile)(win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe'),
    ['-NoProfile', '-NonInteractive', '-Command', command], { timeout: 15000, windowsHide: true });
  return JSON.parse(stdout);
}

async function main() {
  const args = process.argv.slice(2);
  if (args.some(arg => arg.startsWith('-') && arg !== '--chat') || args.filter(arg => !arg.startsWith('-')).length > 1) throw new Error('Usage: miniclaw verify [PATH] [--chat]');
  const destination = args.find(arg => !arg.startsWith('-')) ?? await defaultInstallation();
  const settings = JSON.parse(await readFile(join(destination, 'settings.json'), 'utf8'));
  if (!Number.isInteger(settings.port) || settings.port < 1 || settings.port > 65535) throw new Error('Invalid installation port');
  const origin = `http://127.0.0.1:${settings.port}`;
  const api = async (path, body) => {
    const response = await fetch(origin + path, { method: body === undefined ? 'GET' : 'POST',
      headers: { Origin: origin, 'Sec-Fetch-Site': 'same-origin', ...(body === undefined ? {} : { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }) },
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30000) });
    if (!response.ok) throw new Error(`Local API ${path}: HTTP ${response.status}`);
    return response.json();
  };
  const results = await verifyHost({ api, service: () => serviceStatus(join(destination, 'data')), chat: args.includes('--chat') });
  for (const result of results) console.log(`${result.state}  ${result.name}${result.detail ? ': ' + result.detail : ''}`);
  process.exitCode = results.some(result => result.state === 'FAIL') ? 1 : 0;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
