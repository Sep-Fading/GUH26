// Compares persistence, permissions, and protocol payloads.
import assert from 'node:assert/strict';
// Runs isolated project and deletion cases.
import { test } from 'node:test';
// Creates independent synthetic conversation request identifiers.
import { randomUUID } from 'node:crypto';
// Prepares real folders and the aliases that name them.
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts disposable backends with inspectable requests.
import { launchApp, json, eventually } from './support/harness.mjs';
// Supplies synthetic conversations without paid inference.
import { THREAD, OTHER, CLIENT, started, thread, signedOut } from './support/fixtures.mjs';

// Sends an explicit same-origin management mutation.
const post = (app, path, body = {}) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
// Creates a stored record and returns its acknowledged value.
const add = async (app, kind, body) =>
  (await json(await post(app, `/api/${kind}s`, kind === 'project'
    ? { environment: { kind: 'host' }, ...body } : body), 201))[kind];
// Replaces a stored definition using its last observed revision.
const edit = (app, kind, value, changes) => {
  const { id, ...body } = value;
  return post(app, `/api/${kind}s/${id}`, { ...body, ...changes });
};
// Requests confirmed deletion with optimistic revision checks where applicable.
const remove = (app, kind, value) => post(app, `/api/${kind}s/${value.id}/delete`,
  value.revision ? { revision: value.revision } : {});
// Creates an active skill with distinctive source text.
const skill = (app, name) => add(app, 'skill', { name, instructions: `${name} instructions`, active: true });
// Creates an activated named agent with its own skill selection.
const agent = (app, skillIds = []) => add(app, 'agent', {
  name: 'Editor', instructions: 'Agent instructions', active: true, skillIds,
});

test('projects persist settings and reject stale or invalid writes', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, []);
  const p = await add(app, 'project', { name: 'Game', agentIds: [], skillIds: [] });
  assert.equal(p.revision, 1);
  const changed = (await json(await edit(app, 'project', p, { name: 'Studio' }))).project;
  await json(await edit(app, 'project', p, { name: 'Stale' }), 409);
  for (const body of [{ name: '' }, { name: 'x'.repeat(81) }, { name: 'Game', cwd: '/' },
    { name: 'Game', agentIds: [CLIENT, CLIENT] }, { name: 'Game', skillIds: [CLIENT] }]) {
    assert.ok([400, 404].includes((await post(app, '/api/projects', body)).status));
  }
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, [changed]);
});

test('project skills apply only to default chats and survive restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const shared = await skill(app, 'Project');
  const own = await skill(app, 'Specialist');
  const a = await agent(app, [own.id]);
  const p = await add(app, 'project', { name: 'Game', agentIds: [a.id], skillIds: [shared.id] });
  const normal = (await json(await post(app, '/api/threads', { projectId: p.id }), 201)).thread;
  assert.equal(normal.projectId, p.id);
  assert.equal(normal.agentId, undefined);
  let requests = (await app.inspect()).requests.filter(r => r.method === 'thread/start');
  assert.match(requests[0].params.developerInstructions, /Project instructions/);
  assert.doesNotMatch(requests[0].params.developerInstructions, /Specialist/);
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { projectId: p.id, agentId: a.id }), 201);
  requests = (await app.inspect()).requests.filter(r => r.method === 'thread/start');
  assert.match(requests[1].params.developerInstructions, /Specialist instructions/);
  assert.doesNotMatch(requests[1].params.developerInstructions, /Project instructions/);
  assert.deepEqual(requests[1].params.environments, []);
  await json(await edit(app, 'skill', shared, { instructions: 'New project instructions' }));
  await app.restart();
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.thread.projectId, p.id);
  const resumed = (await app.inspect()).requests.find(r => r.method === 'thread/resume');
  assert.equal(resumed.params.developerInstructions, requests[0].params.developerInstructions);
  const list = await json(await app.request('/api/threads'));
  assert.equal(list.threads.find(c => c.id === THREAD).projectId, p.id);
});

test('project membership gates new chats and later turns', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const a = await agent(app);
  const p = await add(app, 'project', { name: 'Game' });
  await json(await post(app, '/api/threads', { projectId: p.id, agentId: a.id }), 403);
  const allowed = (await json(await edit(app, 'project', p, { agentIds: [a.id] }))).project;
  await json(await post(app, '/api/threads', { projectId: p.id, agentId: a.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Work', clientMessageId: CLIENT,
  }), 202);
  await json(await edit(app, 'project', allowed, { agentIds: [] }));
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/interrupt'));
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Again', clientMessageId: CLIENT,
  }), 403);
});

test('inactive project skills do not restrict specialist skills', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Shared');
  const a = await agent(app);
  const p = await add(app, 'project', { name: 'Game', agentIds: [a.id], skillIds: [s.id] });
  await json(await edit(app, 'skill', s, { active: false }));
  await json(await post(app, '/api/threads', { projectId: p.id }), 409);
  await json(await post(app, '/api/threads', { projectId: p.id, agentId: a.id }), 201);
});

test('project skill revocation cancels and blocks default chats', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Shared');
  const p = await add(app, 'project', { name: 'Game', skillIds: [s.id] });
  await json(await post(app, '/api/threads', { projectId: p.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Work', clientMessageId: CLIENT,
  }), 202);
  await json(await remove(app, 'skill', s));
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/interrupt'));
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Again', clientMessageId: CLIENT,
  }), 409);
});

test('deleted agents preserve history without falling back to default policy', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const a = await agent(app);
  await json(await post(app, '/api/threads', { agentId: a.id }), 201);
  await json(await remove(app, 'agent', { ...a, revision: 99 }), 409);
  await json(await remove(app, 'agent', a));
  assert.deepEqual((await json(await app.request('/api/agents'))).agents, []);
  await app.restart();
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal(snapshot.thread.agentId, a.id);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Again', clientMessageId: CLIENT,
  }), 409);
  await json(await post(app, '/api/threads', { agentId: a.id }), 404);
  await json(await post(app, `/api/threads/${THREAD}/computer`, { enabled: true }), 403);
});

test('skill deletion checks revision and leaves named chat snapshots readable', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const s = await skill(app, 'Own');
  const a = await agent(app, [s.id]);
  await json(await post(app, '/api/threads', { agentId: a.id }), 201);
  await json(await remove(app, 'skill', { ...s, revision: 99 }), 409);
  await json(await remove(app, 'skill', s));
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, []);
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Again', clientMessageId: CLIENT,
  }), 409);
});

test('chat deletion rejects foreign, busy, and descendant-bearing targets', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/delete', { result: {} });
  await app.set('thread/read', { result: { thread: thread('/foreign') } });
  await json(await remove(app, 'thread', { id: THREAD }), 404);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd) } });
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd, { id: OTHER })], nextCursor: null } });
  await json(await remove(app, 'thread', { id: THREAD }), 409);
  await app.set('thread/list', { result: { data: [], nextCursor: null } });
  await json(await post(app, '/api/threads', {}), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Busy', clientMessageId: CLIENT,
  }), 202);
  await json(await remove(app, 'thread', { id: THREAD }), 409);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/delete'), false);
});

test('chat deletion reaches Codex and persists local removal only after success', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await json(await post(app, '/api/threads', {}), 201);
  await app.set('thread/delete', { error: { code: -32600, message: 'Fixture failure' } });
  await json(await remove(app, 'thread', { id: THREAD }), 502);
  await json(await app.request(`/api/threads/${THREAD}`));
  await app.set('thread/delete', { result: {} });
  await json(await remove(app, 'thread', { id: THREAD }));
  assert.equal((await json(await app.request('/api/threads'))).threads.length, 0);
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`), 404);
});

test('new management routes require sign-in and local mutation headers', async t => {
  const app = await launchApp(); t.after(() => app.close());
  for (const path of ['/api/projects', `/api/agents/${CLIENT}/delete`,
    `/api/skills/${CLIENT}/delete`, `/api/threads/${THREAD}/delete`]) {
    assert.equal((await app.request(path, { method: 'POST', body: {},
      headers: { Origin: 'https://elsewhere.example' } })).status, 403);
  }
  await app.set('account/read', { result: signedOut });
  await json(await app.request('/api/projects'), 401);
  await json(await post(app, '/api/projects', { name: 'Game' }), 401);
  await json(await remove(app, 'thread', { id: THREAD }), 401);
});

test('two project conversations can respond independently', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const first = await agent(app);
  const second = await add(app, 'agent', {
    name: 'Designer', instructions: 'Design instructions', active: true,
  });
  const p = await add(app, 'project', { name: 'Game', agentIds: [first.id, second.id] });
  for (const [id, profile] of [[THREAD, first], [OTHER, second]]) {
    await app.set('thread/start', { result: started(app.peer.cwd, { id }) });
    await json(await post(app, '/api/threads', { projectId: p.id, agentId: profile.id }), 201);
    await app.set('turn/start', { result: { turn: { id: randomUUID(), status: 'inProgress', items: [] } } });
    await json(await post(app, `/api/threads/${id}/messages`, {
      text: id, clientMessageId: randomUUID(),
    }), 202);
  }
  for (const [id, profile] of [[THREAD, first], [OTHER, second]]) {
    const snapshot = await json(await app.request(`/api/threads/${id}`));
    assert.equal(snapshot.thread.projectId, p.id);
    assert.equal(snapshot.thread.agentId, profile.id);
    assert.ok(snapshot.activeTurn);
    assert.equal(snapshot.messages[0].text, id);
  }
  const starts = (await app.inspect()).requests.filter(r => r.method === 'thread/start');
  assert.deepEqual(starts.map(r => r.params.developerInstructions),
    [first.instructions, second.instructions]);
});

test('computer switching retains pinned default project instructions', async t => {
  const previous = process.env.MINICLAW_HELPER_BIN;
  process.env.MINICLAW_HELPER_BIN = '/usr/bin/true';
  const app = await launchApp();
  if (previous === undefined) delete process.env.MINICLAW_HELPER_BIN;
  else process.env.MINICLAW_HELPER_BIN = previous;
  t.after(() => app.close());
  const s = await skill(app, 'Pinned');
  const p = await add(app, 'project', { name: 'Game', skillIds: [s.id] });
  await json(await post(app, '/api/threads', { projectId: p.id }), 201);
  await app.set('thread/archive', { result: {} });
  await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
  // The expectation follows the backend's report: a Windows helper is a configured
  // codex-computer-use.exe, which /usr/bin/true is not, so there Computer Use is unavailable
  // and switching it on changes nothing. A machine with a helper checks the switch itself.
  const first = await post(app, `/api/threads/${THREAD}/computer`, { enabled: true });
  if (first.status === 503) {
    const { error } = await json(first, 503);
    assert.equal(error.code, 'computer_unavailable');
    assert.equal(process.platform, 'win32', 'only the Windows helper rule refuses this helper');
    assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/resume'), false);
    const unchanged = await json(await post(app, `/api/threads/${THREAD}/computer`, { enabled: false }));
    assert.equal(unchanged.thread.computerUse, false);
    return;
  }
  const resumed = async () => (await app.inspect()).requests.filter(r => r.method === 'thread/resume').at(-1);
  await json(first);
  assert.match((await resumed()).params.developerInstructions, /Pinned instructions/);
  await json(await post(app, `/api/threads/${THREAD}/computer`, { enabled: false }));
  assert.match((await resumed()).params.developerInstructions, /Pinned instructions/);
});

test('chat deletion also rejects archived descendants', async t => {
  const app = await launchApp(); t.after(() => app.close());
  await app.set('thread/list', { result: { data: [], nextCursor: null }, cases: [
    { params: { ancestorThreadId: THREAD, archived: true }, handler: {
      result: { data: [thread(app.peer.cwd, { id: OTHER })], nextCursor: null },
    } },
  ] });
  await json(await remove(app, 'thread', { id: THREAD }), 409);
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/delete'), false);
});

// Requirement change 2026-09-21: provider web search is on by default, with a switch to turn it off.
test('Codex web search is on by default, and a project that switches it off has the last word', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const p = await add(app, 'project', { name: 'Research', environment: { kind: 'host' } });
  const off = { target: { target_type: 'feature', name: 'no_web_search' }, effect: 'allow' };
  const last = async method => (await app.inspect()).requests.filter(r => r.method === method).at(-1);
  // Codex itself is launched with web search disabled; MiniClaw switches it on per conversation.
  await json(await post(app, '/api/threads', { projectId: p.id }), 201);
  assert.deepEqual((await last('thread/start')).params.config, { web_search: 'live' });
  // A project that switches search off starts its conversations without it, and nothing else in the policy moves.
  await json(await post(app, `/api/permissions/project/${p.id}`, off));
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await json(await post(app, '/api/threads', { projectId: p.id }), 201);
  assert.equal((await last('thread/start')).params.config?.web_search, undefined);
  // An existing conversation picks the project's choice up when it is loaded again.
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal((await last('thread/resume')).params.config.web_search, 'disabled');
  assert.equal((await last('thread/resume')).params.config['features.shell_tool'], false);
  // What the project switched off, a conversation cannot bring back.
  assert.equal((await post(app, `/api/permissions/conversation/${THREAD}`, { ...off, effect: 'block' })).status, 403);
  // With the project's switch removed, the conversation is loaded with web search again.
  await json(await post(app, `/api/permissions/project/${p.id}`, { ...off, effect: 'remove' }));
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  assert.equal((await last('thread/resume')).params.config.web_search, 'live');
});

test('deleting a project deletes its conversations at the provider and leaves other chats alone', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const p = await add(app, 'project', { name: 'Short-lived', agentIds: [], skillIds: [] });
  const inside = (await json(await post(app, '/api/threads', { projectId: p.id }), 201)).thread;
  assert.equal(inside.projectId, p.id);
  await json(await post(app, `/api/projects/${p.id}/delete`, { revision: 99 }), 409);
  await json(await post(app, `/api/projects/${p.id}/delete`, { revision: p.revision, deleteVm: true,
    confirmName: 'Short-lived' }), 400);
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/projects/${p.id}/delete`, { revision: p.revision }));
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, []);
  const deleted = (await app.inspect()).requests.filter(r => r.method === 'thread/delete');
  assert.deepEqual(deleted.map(r => r.params.threadId), [inside.id]);
  assert.equal((await app.request(`/api/threads/${inside.id}`)).status, 404);
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, []);
});

test('a project folder is saved only under its exact resolved path', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-project-folder-')));
  const folder = join(root, 'ProjectFolderLongName');
  await mkdir(folder);
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const save = workspace => post(app, '/api/projects', { name: 'Folder', environment: { kind: 'host' }, workspace });
  const aliases = { link: join(root, 'Alias'), 'trailing separator': `${folder}${process.platform === 'win32' ? '\\' : '/'}`,
    missing: join(folder, 'missing') };
  if (process.platform === 'win32') {
    await symlink(folder, aliases.link, 'junction');
    const short = execFileSync('cmd.exe', ['/d /c for %I in (' + folder + ') do @echo %~sI'],
      { windowsVerbatimArguments: true, encoding: 'utf8' }).trim();
    assert.notEqual(short, folder, 'this volume generates 8.3 names');
    Object.assign(aliases, { verbatim: `\\\\?\\${folder}`, 'lower case': folder.toLowerCase(),
      'forward slashes': folder.replaceAll('\\', '/'), '8.3 name': short });
  } else {
    await symlink(folder, aliases.link);
  }
  for (const [label, workspace] of Object.entries(aliases)) {
    const response = await save(workspace);
    assert.equal(response.status, 400, `${label} ${workspace}: ${await response.text()}`);
  }
  const { project } = await json(await save(folder), 201);
  assert.equal(project.workspace, folder);
});
