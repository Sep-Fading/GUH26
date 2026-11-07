#!/usr/bin/env node
// Checks a running MiniClaw and its real VM from outside, feature by feature. It reads the
// same local API the page uses. It makes no model call unless asked with --chat, never starts
// or stops a VM, and removes the conversation it creates.
//
//   miniclaw verify            check what can be checked without a model
//   miniclaw verify --chat     also send one small message to the VM's Codex
// Reads the installation's port.
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
// Remembers where MiniClaw was installed.
import { defaultInstallation } from '../setup/default-installation.mjs';

const args = process.argv.slice(2);
const chat = args.includes('--chat');
const installation = args.find(arg => !arg.startsWith('-')) ?? await defaultInstallation();
const { port } = JSON.parse(await readFile(join(installation, 'settings.json'), 'utf8'));
const origin = `http://127.0.0.1:${port}`;
const results = [];

// Calls the local API the way the page does.
async function api(path, body) {
  const response = await fetch(`${origin}${path}`, { method: body === undefined ? 'GET' : 'POST',
    headers: { Origin: origin, 'Sec-Fetch-Site': 'same-origin',
      ...(body === undefined ? {} : { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }) },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30_000) });
  const text = await response.text();
  let value; try { value = JSON.parse(text); } catch { value = text; }
  if (!response.ok) throw new Error(`${response.status} ${value?.error?.message ?? text.slice(0, 120)}`);
  return value;
}

// Runs one check and records pass, fail or skip with a reason a person can act on.
async function check(name, run) {
  try { const detail = await run(); results.push(['pass', name, detail ?? '']); }
  catch (error) { results.push([error.skip ? 'skip' : 'FAIL', name, error.message]); }
}
const skip = reason => Object.assign(new Error(reason), { skip: true });
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));

let master, thread;
await check('MiniClaw is running and signed in', async () => {
  await api('/api/health');
  const account = await api('/api/account');
  if (account.status !== 'signed_in') throw new Error('not signed in');
  return (account.providers ?? []).map(provider => `${provider.id}: ${provider.status}`).join(', ');
});
await check('the Master VM is running, logged in and signed in to Codex', async () => {
  master = await api('/api/vms/master/status');
  if (master.state !== 'running') throw new Error(`state is "${master.state}". Start the VM (send a message to a VM chat, or Settings > Virtualisation > Start) and run this again.`);
  if (!master.ready) throw new Error('running, but its worker is not connected. Log in to the VM.');
  if (!master.signedIn) throw new Error('its Codex is not signed in. Run "Install Codex.command" in the VM and sign in.');
  return `computer use helper: ${master.computerReady ? 'installed' : 'not installed'}`;
});
const running = () => { if (master?.state !== 'running' || !master.ready) throw skip('needs the running VM'); };

await check('the VM worker is the current version', async () => {
  running();
  const view = await api('/api/virtualisation');
  if (view.workerCurrent !== true) throw new Error('the worker is older than this MiniClaw. Shut the VM down, run: miniclaw update-vm, start it, then run "Install Codex.command" in the VM.');
  return `${view.ledger.vms.length} VM(s) in the budget ledger, exceeded: ${view.ledger.exceeded.join(', ') || 'none'}`;
});
await check('the desktop stream delivers a picture', async () => {
  running();
  const response = await fetch(`${origin}/api/vms/master/desktop?viewer=${crypto.randomUUID()}`,
    { headers: { Origin: origin, 'Sec-Fetch-Site': 'same-origin' }, signal: AbortSignal.timeout(20_000) });
  const reader = response.body.getReader();
  let bytes = 0; const started = Date.now();
  while (bytes < 20_000 && Date.now() - started < 15_000) { const { value, done } = await reader.read(); if (done) break; bytes += value.length; }
  await reader.cancel();
  if (bytes < 20_000) throw new Error(`only ${bytes} bytes in 15 s. In the VM, allow Screen Recording for the MiniClaw worker.`);
  return `${Math.round(bytes / 1024)} KB in ${((Date.now() - started) / 1000).toFixed(1)} s`;
});
await check('file transfer: the VM lists its two offered folders', async () => {
  running();
  const workspace = await api('/api/vms/master/transfers/files?folder=workspace&path=');
  const transfers = await api('/api/vms/master/transfers/files?folder=transfers&path=');
  return `Workspace ${workspace.entries.length} entries, Transfers ${transfers.entries.length} entries`;
});
await check('idle shutdown setting is saved per VM and restored', async () => {
  const before = (await api('/api/vms/master/idle')).minutes;
  await api('/api/vms/master/idle', { minutes: 30 });
  const read = (await api('/api/vms/master/idle')).minutes;
  await api('/api/vms/master/idle', { minutes: before });
  if (read !== 30) throw new Error(`saved 30, read ${read}`);
  return `was ${before} min, restored`;
});
await check('a VM conversation can be created', async () => {
  running();
  thread = (await api('/api/threads', { environment: { kind: 'vm' } })).thread;
  if (thread.vmId !== 'master') throw new Error(`it runs on "${thread.vmId}"`);
  return thread.id;
});
const needsThread = () => { if (!thread) throw skip('needs the VM conversation'); };
await check('workspace panel: files of the VM workspace are listed through the sandbox', async () => {
  needsThread();
  const files = await api(`/api/threads/${thread.id}/workspace/files?path=`);
  return `${files.root}: ${files.entries.length} entries`;
});
await check('workspace panel: git runs sandboxed inside the VM', async () => {
  needsThread();
  const changes = await api(`/api/threads/${thread.id}/workspace/changes`);
  return `source: ${changes.source}, ${changes.files.length} changed file(s)`;
});
await check('experimental backup of the VM workspace, then a checked restore', async () => {
  running();
  const before = (await api('/api/vms/master/backups')).backups.length;
  await api('/api/vms/master/backups/run', {});
  let view;
  for (let tries = 0; tries < 60; tries++) { await wait(1000); view = await api('/api/vms/master/backups'); if (view.backups.length > before || view.settings.lastError) break; }
  if (view.settings.lastError && view.backups.length === before) throw new Error(view.settings.lastError);
  if (view.backups.length === before) throw new Error('no backup appeared in 60 s');
  const newest = view.backups[0];
  const { jobId } = await api(`/api/vms/master/backups/${newest.id}/restore`, {});
  let job;
  for (let tries = 0; tries < 60; tries++) { await wait(500); job = await api(`/api/transfers/${jobId}`); if (job.state !== 'running') break; }
  if (job.state !== 'done') throw new Error(`restore ${job.state}: ${job.error ?? ''}`);
  return `${newest.files} files backed up; restored ${job.doneFiles} into Transfers/restore-${newest.id}`;
});
await check('one real message is answered by the Codex inside the VM', async () => {
  if (!chat) throw skip('run with --chat to include one small model call');
  needsThread();
  const models = (await api('/api/models')).models.filter(model => !model.provider);
  // Small models only; the largest model is never used for a check.
  const model = models.find(entry => /luna/i.test(entry.model)) ?? models.find(entry => !/astra/i.test(entry.model));
  if (!model) throw new Error('no small Codex model is offered');
  await api(`/api/threads/${thread.id}/messages`, { text: 'Reply with exactly: VM OK', clientMessageId: crypto.randomUUID(), model: model.model, effort: 'low' });
  let snapshot;
  for (let tries = 0; tries < 120; tries++) { await wait(1000); snapshot = await api(`/api/threads/${thread.id}`); if (!snapshot.activeTurn && snapshot.messages.at(-1)?.role === 'assistant') break; }
  const answer = snapshot.messages.at(-1)?.text ?? '';
  if (!/VM OK/i.test(answer)) throw new Error(`answer was "${answer.slice(0, 80)}"`);
  return `${model.model} answered "${answer.slice(0, 40)}"`;
});
if (thread) await check('the check conversation is removed again', async () => { await api(`/api/threads/${thread.id}/delete`, {}); });

const width = Math.max(...results.map(([, name]) => name.length));
for (const [state, name, detail] of results) console.log(`${state.padEnd(4)}  ${name.padEnd(width)}  ${detail}`);
const failed = results.filter(([state]) => state === 'FAIL').length;
console.log(`\n${results.filter(([state]) => state === 'pass').length} passed, ${failed} failed, ${results.filter(([state]) => state === 'skip').length} skipped.`);
console.log('Not checked here, because they need you at the screen: the macOS file panel of a transfer, the VM window\'s "Control this VM", and an idle shutdown\'s 60 s warning.');
process.exitCode = failed ? 1 : 0;
