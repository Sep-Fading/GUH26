import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { launchApp, json, eventually, stopChild, events } from './support/harness.mjs';
import { startBroker } from '../scripts/vm/broker.mjs';
import { startControl } from '../scripts/vm/control.mjs';

// Build it with `cargo build --examples`; it plays the VM's worker in these tests.
const worker = fileURLToPath(new URL('../target/debug/examples/standin-worker', import.meta.url));
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });

// A fake Codex for the stand-in VM: owns one task and records where it was started.
// The worker narrows PATH like a guest would, so the script names its interpreter exactly.
const guestCodex = cwd => `#!${process.execPath}
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';
const cwd = ${JSON.stringify(cwd)}, id = '019a7777-7777-7777-8777-777777777777';
const thread = { id, cwd, source: 'appServer', modelProvider: 'openai', name: 'VM task', preview: 'VM task',
  createdAt: 1900000001, updatedAt: 1900000001, status: { type: 'idle' }, turns: [] };
const say = frame => process.stdout.write(JSON.stringify(frame) + '\\n');
createInterface({ input: process.stdin }).on('line', line => {
  const msg = JSON.parse(line);
  // An answer to a tool call this Codex made: kept, so a test can read what the Mac said.
  if (!msg.method && String(msg.id).startsWith('call-')) appendFileSync(cwd + '/answers', JSON.stringify(msg) + '\\n');
  if (msg.method === 'initialized' || !msg.method) return;
  if (msg.method === 'thread/start') appendFileSync(cwd + '/starts', JSON.stringify(msg.params) + '\\n');
  if (msg.method === 'turn/start') {
    // A message of the form call {"tool": ..., "arguments": ...} makes the model call that tool.
    const turn = { id: '019a8888-8888-7888-8888-' + String(Date.now()).slice(-12).padStart(12, '0'), status: 'inProgress', items: [] };
    say({ id: msg.id, result: { turn } });
    say({ method: 'turn/started', params: { threadId: id, turn } });
    const words = msg.params.input.find(part => part.type === 'text')?.text ?? '';
    if (words.startsWith('call ')) {
      const { tool, arguments: args } = JSON.parse(words.slice(5));
      const callId = 'call-' + turn.id;
      say({ id: callId, method: 'item/tool/call', params: { tool, threadId: id, turnId: turn.id, callId, arguments: args } });
    }
    return;
  }
  const result = msg.method === 'config/read' ? { config: { mcp_servers: {} } }
    : msg.method === 'account/read' ? { account: { type: 'chatgpt', email: 'guest@example.test', planType: 'plus' } }
    : msg.method === 'model/list' ? { data: [], nextCursor: null }
    : msg.method === 'thread/list' ? { data: [], nextCursor: null }
    : msg.method === 'thread/start' ? { thread, model: 'fixture-model', reasoningEffort: null } : {};
  process.stdout.write(JSON.stringify({ id: msg.id, result }) + '\\n');
});
`;

// Creates the private link directory a VM session would own, with no VM running.
async function links(t) {
  const dir = await mkdtemp('/private/tmp/mc-hv-');
  const master = join(dir, 'master'), guest = join(dir, 'guest');
  await mkdir(master, { mode: 0o700 });
  await mkdir(join(dir, 'dedicated'), { mode: 0o700 });
  await mkdir(guest, { mode: 0o700 });
  const controlKey = randomBytes(32), workerKey = randomBytes(32);
  const states = { master: 'off' };
  const budget = { vms: [{ id: '/vms/master', state: 'stopped', cpuCount: 2, memoryBytes: 8 * 1024 ** 3 }],
    limits: { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 },
    capacity: { cpuCount: 5, memoryBytes: 12 * 1024 ** 3, diskBytes: 231 * 1024 ** 3 },
    used: { cpuCount: 0, memoryBytes: 0, diskBytes: 64 * 1024 ** 3 }, exceeded: [], freeBytes: 600 * 1024 ** 3 };
  const control = await startControl(dir, controlKey, {
    status: vmId => ({ state: states[vmId] ?? 'missing' }),
    start: vmId => { states[vmId] = 'starting'; return { state: 'starting' }; },
    list: () => [],
    // The shared VM budget, as the real session would report it.
    ledger: () => budget,
    limits: values => { budget.limits = values; return budget; },
    reconcile: () => ({ ...budget, released: ['/vms/lost'], refused: [] }),
    forget: () => ({ ...budget, forgotten: [] }),
  });
  const broker = await startBroker(master);
  await writeFile(join(dir, 'control.key'), controlKey.toString('hex'), { mode: 0o600 });
  await writeFile(join(dir, 'worker.json'), JSON.stringify({ key: workerKey.toString('hex'), cwd: guest }), { mode: 0o600 });
  await writeFile(join(dir, 'links.json'), JSON.stringify({ controlSocket: join(dir, 'control.sock'),
    controlKey: join(dir, 'control.key'), worker: join(dir, 'worker.json'),
    master: join(master, 'controller.sock'), dedicated: join(dir, 'dedicated') }), { mode: 0o600 });
  await writeFile(join(dir, 'guest-codex.mjs'), guestCodex(guest), { mode: 0o700 });
  await chmod(join(dir, 'guest-codex.mjs'), 0o700);
  let child;
  t.after(async () => { if (child) await stopChild(child); await broker.close(); await control.close();
    await rm(dir, { recursive: true, force: true }); });
  return { dir, guest, links: join(dir, 'links.json'),
    // Stands in for a booted Master VM: the same worker protocol over the broker socket.
    // Stands in for the VM shutting down.
    async stopMaster() { if (child) await stopChild(child); child = undefined; },
    bootMaster(env = {}) {
      // A booted VM is what the real session reports as running.
      states.master = 'running';
      child = spawn(worker, [join(dir, 'worker.json'), join(master, 'worker.sock'),
        join(dir, 'control.sock'), join(dir, 'control.key')],
      { env: { ...process.env, CODEX_BIN: join(dir, 'guest-codex.mjs'), ...env }, stdio: 'ignore' });
    } };
}

test('the Mac backend serves chat with VM links configured and no VM running', async t => {
  const vm = await links(t);
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: vm.links } });
  t.after(() => app.close());
  const created = await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  assert.deepEqual(created.thread.environment, { kind: 'host' });
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), true);
  const status = await json(await app.request('/api/vms/master/status'));
  assert.equal(status.state, 'off');
  const refused = await post(app, '/api/threads', { environment: { kind: 'vm' } });
  assert.equal(refused.status, 503);
  assert.equal((await refused.json()).error.code, 'vm_unavailable');
});

test('a VM chat runs in the VM worker once it connects, never on the Mac', async t => {
  const vm = await links(t);
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: vm.links } });
  t.after(() => app.close());
  vm.bootMaster();
  const created = await eventually(async () => {
    const response = await post(app, '/api/threads', { environment: { kind: 'vm' } });
    return response.status === 201 ? response.json() : null;
  }, Boolean, 'VM worker attachment', 15000);
  assert.equal(created.thread.vmId, 'master');
  assert.equal(created.thread.id, '019a7777-7777-7777-8777-777777777777');
  const start = JSON.parse((await readFile(join(vm.guest, 'starts'), 'utf8')).trim());
  assert.equal(start.cwd, vm.guest);
  // A plain VM chat gets the file tool too: the VM is the sandbox, so no agent or project is needed.
  // The gateway and the question tool come with it, answered by the Mac; it is in no project, so
  // it has no project tool.
  assert.deepEqual(start.dynamicTools.map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask']);
  assert.match(start.baseInstructions, /MCP servers run on the user's Mac, not in this VM/);
  assert.match(start.baseInstructions, /inside a MiniClaw virtual machine/);
  assert.ok(start.baseInstructions.includes(vm.guest), 'the model is told its workspace folder');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

/** Boots the stand-in VM and starts one conversation in it, in a project when one is named. */
async function vmChat(t, { worker = {}, project = false } = {}) {
  const vm = await links(t);
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: vm.links } });
  t.after(() => app.close());
  vm.bootMaster(worker);
  const made = project ? (await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'vm', dedicated: false } }), 201)).project : null;
  const created = await eventually(async () => {
    const response = await post(app, '/api/threads', { environment: { kind: 'vm' }, ...(made ? { projectId: made.id } : {}) });
    return response.status === 201 ? response.json() : null;
  }, Boolean, 'VM worker attachment', 15000);
  const start = JSON.parse((await readFile(join(vm.guest, 'starts'), 'utf8')).trim().split('\n').at(-1));
  /** Makes the guest's model call a tool and returns what the Mac answered. */
  const call = async (tool, args) => {
    const before = await readFile(join(vm.guest, 'answers'), 'utf8').then(text => text.trim().split('\n').length, () => 0);
    await json(await post(app, `/api/threads/${created.thread.id}/messages`,
      { text: `call ${JSON.stringify({ tool, arguments: args })}`, clientMessageId: crypto.randomUUID() }), 202);
    return eventually(async () => (await readFile(join(vm.guest, 'answers'), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean),
      lines => lines.length > before, `the Mac's answer to ${tool}`, 15000).then(lines => JSON.parse(lines.at(-1)).result);
  };
  return { app, vm, thread: created.thread.id, project: made, start, call };
}

test('a VM project conversation writes the project\'s memory, which lives on the Mac', async t => {
  const { app, thread, project, start, call } = await vmChat(t, { project: true });
  assert.deepEqual(start.dynamicTools.map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask', 'miniclaw_project']);
  assert.match(start.baseInstructions, /miniclaw_project/);
  const saved = await call('miniclaw_project', { action: 'memory_save', name: 'Engine', description: 'Roblox Studio.', text: 'Luau, strict mode.' });
  assert.equal(saved.success, true);
  assert.deepEqual((await json(await app.request(`/api/projects/${project.id}/memory`))).notes.map(note => [note.name, note.text]), [['Engine', 'Luau, strict mode.']]);
  const shown = (await json(await app.request(`/api/threads/${thread}`))).activities.filter(entry => entry.kind === 'memory');
  assert.deepEqual(shown.map(entry => [entry.title, entry.status]), [['Remembered: Engine', 'ok']]);
});

test('a question from a VM conversation opens the popup on the Mac, and the answer returns to the VM', async t => {
  const { app, thread, call } = await vmChat(t);
  const answer = call('miniclaw_ask', { questions: [{ question: 'Which language?', options: [{ label: 'Luau' }, { label: 'Python' }] }] });
  const waiting = await eventually(async () => json(await app.request(`/api/threads/${thread}`)), value => value.question, 'the popup', 15000);
  await json(await post(app, `/api/threads/${thread}/questions/${waiting.question.id}`, { answers: [{ selected: ['Luau'] }] }));
  const result = await answer;
  assert.equal(result.success, true);
  assert.match(result.contentItems[0].text, /Luau/);
});

test('a VM whose worker was installed before this keeps working with the file tool alone', async t => {
  const { app, start, call } = await vmChat(t, { worker: { MINICLAW_TEST_OLD_WORKER: '1' }, project: true });
  assert.deepEqual(start.dynamicTools.map(tool => tool.name), ['miniclaw_scoped_native']);
  assert.doesNotMatch(start.baseInstructions, /miniclaw_project|miniclaw_ask|miniclaw_mcp/);
  assert.equal((await json(await app.request('/api/virtualisation'))).workerCurrent, false);
});

test('without VM links the VM choice is unavailable', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  assert.equal((await post(app, '/api/threads', { environment: { kind: 'vm' } })).status, 503);
  assert.equal((await app.request('/api/vms/master/status')).status, 503);
});

test('the backend reads the link file a real Mac-backend session writes', async t => {
  const { startPair } = await import('../scripts/vm/session.mjs');
  const root = await mkdtemp('/private/tmp/mc-hvs-');
  await chmod(root, 0o700);
  await mkdir(join(root, 'Master'));
  const native = join(root, 'native');
  await writeFile(native, `#!${process.execPath}\nsetInterval(() => {}, 1000);\n`, { mode: 0o700 });
  await writeFile(join(root, 'worker.json'), JSON.stringify({ key: 'cd'.repeat(32), cwd: '/Users/Shared/MiniClaw/Workspace' }), { mode: 0o600 });
  const session = await startPair({ root, port: 19878, mac: { worker: join(root, 'worker.json') },
    master: { binary: native, guest: join(root, 'Master'), media: 'unused' }, provisioning: { resources: {} } },
  { admit: () => ({ settle() {} }) });
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: session.linksPath } });
  t.after(async () => { await app.close(); await session.close(); await rm(root, { recursive: true, force: true }); });
  assert.equal((await json(await app.request('/api/vms/master/status'))).state, 'off');
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
});

test('a VM chat keeps a copy of its history on the Mac and shows it while the VM is off', async t => {
  const vm = await links(t);
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: vm.links } });
  t.after(() => app.close());
  vm.bootMaster();
  const created = await eventually(async () => {
    const response = await post(app, '/api/threads', { environment: { kind: 'vm' } });
    return response.status === 201 ? response.json() : null;
  }, Boolean, 'VM worker attachment', 15000);
  const id = created.thread.id;
  const online = await json(await app.request(`/api/threads/${id}`));
  assert.equal(online.vmOffline, undefined);
  const copy = join(app.peer.dir, 'vm-transcripts', `${id}.json`);
  // The copy is written off the request path, so it may land a moment later.
  const info = await eventually(() => stat(copy).catch(() => null), Boolean, 'Mac copy of the chat', 5000);
  assert.equal(info.mode & 0o777, 0o600);
  // A finished turn would have stored these; the fixture guest runs no turns.
  const saved = JSON.parse(await readFile(copy, 'utf8'));
  saved.messages = [{ id: 'm1', role: 'user', text: 'Kept on the Mac', complete: true }];
  await writeFile(copy, JSON.stringify(saved), { mode: 0o600 });
  await vm.stopMaster();
  const offline = await eventually(async () => json(await app.request(`/api/threads/${id}`)),
    snapshot => snapshot.vmOffline === true, 'offline snapshot', 15000);
  assert.deepEqual(offline.messages.map(message => message.text), ['Kept on the Mac']);
  assert.equal(offline.thread.createdAt, online.thread.createdAt);
  assert.equal(offline.thread.vmOffline, true);
  assert.equal(offline.activeTurn, null);
});

test('the settings page reads the VM budget, changes its limits and sees the VM worker version', async t => {
  const vm = await links(t);
  const app = await launchApp({}, { env: { MINICLAW_VM_LINKS: vm.links } });
  t.after(() => app.close());
  const before = await json(await app.request('/api/virtualisation'));
  assert.equal(before.available, true);
  assert.equal(before.master.state, 'off');
  assert.equal(before.workerCurrent, null, 'unknown while the VM is off');
  assert.deepEqual(before.ledger.limits, { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 });
  assert.equal(before.ledger.vms[0].id, '/vms/master');
  const changed = await json(await post(app, '/api/virtualisation/limits',
    { cpuPercent: 40, memoryPercent: 60, diskPercent: 30 }));
  assert.deepEqual(changed.ledger.limits, { cpuPercent: 40, memoryPercent: 60, diskPercent: 30 });
  for (const bad of [{ cpuPercent: 0, memoryPercent: 60, diskPercent: 30 },
    { cpuPercent: 40, memoryPercent: 101, diskPercent: 30 }, { cpuPercent: 40.5, memoryPercent: 60, diskPercent: 30 },
    { cpuPercent: 40, memoryPercent: 60 }]) {
    assert.equal((await post(app, '/api/virtualisation/limits', bad)).status, 400, JSON.stringify(bad));
  }
  assert.deepEqual((await json(await post(app, '/api/virtualisation/reconcile', {}))).ledger.released, ['/vms/lost']);
  assert.deepEqual((await json(await post(app, '/api/virtualisation/forget', {}))).ledger.forgotten, []);
  vm.bootMaster();
  const running = await eventually(async () => json(await app.request('/api/virtualisation')),
    view => view.workerCurrent !== null, 'the VM worker to attach', 15000);
  assert.equal(running.workerCurrent, true, 'a worker built from this source is current');
});

test('without VM links the settings page is told virtualisation is not set up', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual(await json(await app.request('/api/virtualisation')), { available: false });
  assert.equal((await post(app, '/api/virtualisation/forget', {})).status, 503);
});

test('a VM conversation has a terminal in the VM: it echoes, the agent\'s command runs in it, and an older worker has none', async t => {
  const { app, vm, thread, call } = await vmChat(t, { project: true });
  const opened = await json(await post(app, `/api/threads/${thread}/terminal/open`, { cols: 100, rows: 30 }));
  assert.equal(opened.status, 'live');
  const feed = await events(app, `/api/threads/${thread}/terminal/stream?after=0`);
  const screen = () => feed.seen.filter(event => event.event === 'output').map(event => Buffer.from(event.data.bytes, 'base64').toString()).join('');
  await eventually(screen, text => /%/.test(text), 'the VM shell\'s prompt', 15000);
  await json(await post(app, `/api/threads/${thread}/terminal/input`, { text: 'echo VM-TYPED; pwd\r' }));
  await eventually(screen, text => /VM-TYPED\r?\n/.test(text), 'typed output from the VM', 15000);
  assert.ok(screen().includes(vm.guest), 'the shell starts in the VM\'s workspace');
  // The agent's approved command runs in the same shell and is captured for the model.
  // `echo` is on the read-only list, so the agent's command runs in the VM's shell with no popup.
  const result = await call('miniclaw_scoped_native', { action: 'run_shell', text: 'echo VM-AGENT', cwd: vm.guest });
  assert.match(result.contentItems[0].text, /Exit code: 0\nstdout:\nVM-AGENT/);
  await eventually(screen, text => /VM-AGENT\r?\n/.test(text), 'the agent\'s output on screen', 15000);
  await feed.close();
  assert.equal((await json(await app.request('/api/virtualisation'))).workerCurrent, true);
});

test('a VM whose worker predates the terminal has none, and its commands take the one-shot path', async t => {
  const { app, thread, call, vm } = await vmChat(t, { worker: { MINICLAW_TEST_OLD_WORKER: '1' }, project: true });
  const refused = await post(app, `/api/threads/${thread}/terminal/open`, { cols: 100, rows: 30 });
  assert.equal(refused.status, 409);
  assert.match((await refused.json()).error.message, /update-vm/);
  // `echo` is on the read-only list: no popup, and the command still takes the one-shot path.
  assert.match((await call('miniclaw_scoped_native', { action: 'run_shell', text: 'echo ONE-SHOT', cwd: vm.guest })).contentItems[0].text, /ONE-SHOT/);
});
