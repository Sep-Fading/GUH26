// Compares storage and protocol behavior precisely.
import assert from 'node:assert/strict';
// Runs isolated skill management acceptance cases.
import { test } from 'node:test';
// Creates disposable sources without touching installed skills.
import { mkdir, writeFile, readFile, symlink } from 'node:fs/promises';
// Locates sources within disposable Codex homes.
import { join } from 'node:path';
// Starts local fixtures and checks JSON responses.
import { launchApp, json, eventually } from './support/harness.mjs';
import { assertGate } from './support/sandbox.mjs';
import { linkOrRefused, NO_LINKS } from './support/windows-links.mjs';
// Supplies synthetic authentication and conversation identifiers.
import { signedOut, THREAD, CLIENT } from './support/fixtures.mjs';

// Sends an explicit user mutation with the local request header.
const post = (app, path, body) => app.request(path, { method: 'POST',
  body: path === '/api/threads' ? { environment: { kind: 'host' }, ...body } : body });
// Creates a bounded instruction document, inactive unless requested.
const draft = (extra = {}) => ({ name: 'Clarity', description: 'Review wording.',
  instructions: '# Review\n\nPreserve `API_NAMES`.\n', ...extra });
// Saves a skill and requires its creation acknowledgement.
const save = async (app, extra = {}) =>
  (await json(await post(app, '/api/skills', draft(extra)), 201)).skill;
// Updates the complete saved revision without its response-only identifier.
const edit = (app, skill, extra) => {
  const { id, ...body } = skill;
  return post(app, `/api/skills/${id}`, { ...body, ...extra });
};
// Creates a profile with explicitly selected skills.
const agent = async (app, skillIds, active = true) => (await json(await post(app,
  '/api/agents', { name: 'Editor', instructions: 'Review text.', skillIds, active }), 201)).agent;
// While Windows cannot enforce the sandbox, a host agent chat is offered no sandboxed tool: its
// dynamic tools lack miniclaw_scoped_native and keep exactly the MCP gateway and the question tool.
// Only that report selects the Windows list, so the full list is checked once the gate opens.
const GATE_CLOSED_TOOLS = ['miniclaw_mcp', 'miniclaw_ask'];
// Tells whether the offered tool names show the closed Windows sandbox gate.
const sandboxGateClosed = names => process.platform === 'win32' && !names.includes('miniclaw_scoped_native');

test('skills persist exact source and reject stale edits', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, []);
  const skill = await save(app);
  assert.equal(skill.active, false);
  assert.equal(skill.revision, 1);
  assert.equal(skill.instructions, draft().instructions);
  const changed = (await json(await edit(app, skill, { active: true }))).skill;
  assert.equal(changed.revision, 2);
  assert.equal((await edit(app, skill, { name: 'stale' })).status, 409);
  await app.restart();
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, [changed]);
});

test('skill management requires authentication and local headers', async t => {
  const app = await launchApp(); t.after(() => app.close());
  for (const headers of [{ Origin: 'https://elsewhere.example' },
    { 'X-MiniClaw-Request': '' }, { 'Sec-Fetch-Site': 'cross-site' }]) {
    assert.equal((await app.request('/api/skills', {
      method: 'POST', body: draft(), headers,
    })).status, 403);
  }
  await app.set('account/read', { result: signedOut });
  await app.request('/api/account');
  for (const path of ['/api/skills', '/api/skills/sources']) {
    assert.equal((await app.request(path)).status, 401);
  }
  assert.equal((await post(app, '/api/skills', draft())).status, 401);
  assert.equal((await post(app, '/api/skills/import', { source: 'clarity' })).status, 401);
});

test('skill data rejects invalid values and authority fields', async t => {
  const app = await launchApp(); t.after(() => app.close());
  for (const extra of [{ name: '' }, { name: 'x'.repeat(81) }, { instructions: '' },
    { instructions: 'x'.repeat(16385) }, { instructions: 'a\0b' },
    { description: 'x'.repeat(501) }, { command: 'sh' }, { active: true, tools: ['shell'] },
    { revision: 1 }]) {
    assert.equal((await post(app, '/api/skills', draft(extra))).status, 400);
  }
  await json(await post(app, '/api/skills/not-a-uuid', draft()), 400);
});

test('Codex import is explicit and copies only SKILL.md', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const root = join(app.peer.codexHome, 'skills', 'clarity');
  await mkdir(root, { recursive: true });
  const source = '---\nname: clarity\ndescription: Keep exact terms.\n---\n\n# Review\nKeep `code`.\n';
  await writeFile(join(root, 'SKILL.md'), source);
  await writeFile(join(root, 'run.sh'), 'exit 99\n');
  const listed = await json(await app.request('/api/skills/sources'));
  assert.deepEqual(listed.sources, [{ id: 'clarity', name: 'clarity' }]);
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, []);
  const { skill } = await json(await post(app, '/api/skills/import', { source: 'clarity' }), 201);
  assert.equal(skill.instructions, source);
  assert.equal(skill.active, false);
  await json(await edit(app, skill, { instructions: '# Changed locally' }));
  assert.equal(await readFile(join(root, 'SKILL.md'), 'utf8'), source);
  assert.equal(await readFile(join(root, 'run.sh'), 'utf8'), 'exit 99\n');
  assert.equal((await app.inspect()).requests.some(r => r.method === 'thread/start'), false);
});

test('import rejects traversal, symlinks and oversized source files', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const root = join(app.peer.codexHome, 'skills');
  await mkdir(join(root, 'large'), { recursive: true });
  await writeFile(join(root, 'large', 'SKILL.md'), 'x'.repeat(16385));
  if (!await linkOrRefused(join(root, 'large'), join(root, 'linked'))) return t.skip(NO_LINKS);
  await mkdir(join(root, 'linked-file'));
  await symlink(join(root, 'large', 'SKILL.md'), join(root, 'linked-file', 'SKILL.md'));
  for (const source of ['../auth.json', '/etc/passwd', 'large', 'linked', 'linked-file', 'missing']) {
    const response = await post(app, '/api/skills/import', { source });
    assert.ok([400, 404].includes(response.status), `${source}: ${response.status}`);
  }
  assert.deepEqual((await json(await app.request('/api/skills'))).skills, []);
});

test('only existing active skills attach to active agents', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const skill = await save(app);
  const profile = { name: 'Editor', instructions: 'Review.', active: true };
  assert.equal((await post(app, '/api/agents', { ...profile, skillIds: [CLIENT] })).status, 404);
  assert.equal((await post(app, '/api/agents', { ...profile, skillIds: [skill.id] })).status, 409);
  assert.equal((await post(app, '/api/agents', {
    ...profile, active: false, skillIds: [skill.id, skill.id],
  })).status, 400);
});

test('chat skills are pinned across edits and restart without granting tools', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const skill = await save(app, { active: true });
  const profile = await agent(app, [skill.id]);
  await json(await post(app, '/api/threads', { agentId: profile.id }), 201);
  const start = (await app.inspect()).requests.find(r => r.method === 'thread/start').params;
  assert.ok(start.developerInstructions.includes(skill.instructions));
  assert.ok(start.developerInstructions.includes(profile.instructions));
  assert.deepEqual(start.environments, []);
  const names = (start.dynamicTools ?? []).map(tool => tool.name);
  if (await assertGate(app, sandboxGateClosed(names))) assert.deepEqual(names, GATE_CLOSED_TOOLS);
  // Requirement change 2026-09-19: host agent chats get the one MiniClaw tool, enforced by Seatbelt.
  // Requirement change 2026-09-21: every conversation on this Mac also gets the MCP gateway, which reaches nothing until a server is switched on.
  else assert.deepEqual((start.dynamicTools ?? []).map(tool => tool.name), ['miniclaw_scoped_native', 'miniclaw_mcp', 'miniclaw_ask']);
  assert.deepEqual(start.selectedCapabilityRoots, []);
  await json(await edit(app, skill, { instructions: 'Changed instructions.' }));
  await app.restart();
  await json(await app.request(`/api/threads/${THREAD}`));
  const resume = (await app.inspect()).requests.find(r => r.method === 'thread/resume').params;
  assert.equal(resume.developerInstructions, start.developerInstructions);
});

test('deactivation blocks pinned chats and cancels active work', async t => {
  const app = await launchApp(); t.after(() => app.close());
  const skill = await save(app, { active: true });
  const profile = await agent(app, [skill.id]);
  await json(await post(app, '/api/threads', { agentId: profile.id }), 201);
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Review', clientMessageId: CLIENT,
  }), 202);
  await json(await edit(app, skill, { active: false }));
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/interrupt'));
  await app.restart();
  assert.equal((await app.request(`/api/threads/${THREAD}`)).status, 200);
  assert.equal((await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Again', clientMessageId: CLIENT,
  })).status, 409);
  assert.equal((await post(app, '/api/threads', { agentId: profile.id })).status, 409);
});

test('skills import from Codex or from Claude Code, and an imported skill serves chats of either provider', async t => {
  const { mkdir: makeDir, mkdtemp: makeTemp, writeFile: write, rm: remove, readFile: read } = await import('node:fs/promises');
  const { tmpdir: temp } = await import('node:os');
  const claudeHome = await makeTemp(join(temp(), 'mc-claude-skills-'));
  t.after(() => remove(claudeHome, { recursive: true, force: true }));
  const app = await launchApp({}, { env: { CLAUDE_CONFIG_DIR: claudeHome } });
  t.after(() => app.close());
  await makeDir(join(app.peer.codexHome, 'skills', 'from-codex'), { recursive: true });
  await write(join(app.peer.codexHome, 'skills', 'from-codex', 'SKILL.md'), '# Codex skill\n\nWrite tests first.');
  await makeDir(join(claudeHome, 'skills', 'from-claude'), { recursive: true });
  await write(join(claudeHome, 'skills', 'from-claude', 'SKILL.md'), '---\nname: from-claude\n---\n\nPrefer plain words.');
  // A credential beside the skills is never listed or readable through this route.
  await write(join(claudeHome, '.credentials.json'), '{"token":"SECRET"}');

  const names = async provider => (await json(await app.request(`/api/skills/sources?provider=${provider}`))).sources.map(s => s.name);
  assert.deepEqual(await names('codex'), ['from-codex']);
  assert.deepEqual(await names('claude'), ['from-claude']);
  assert.deepEqual((await json(await app.request('/api/skills/sources'))).sources.map(s => s.name), ['from-codex'], 'Codex stays the default');
  assert.equal((await app.request('/api/skills/sources?provider=other')).status, 400);

  const { skill } = await json(await post(app, '/api/skills/import', { source: 'from-claude', provider: 'claude' }), 201);
  assert.equal(skill.active, false);
  assert.match(skill.instructions, /Prefer plain words\./);
  for (const source of ['../.credentials.json', '.credentials.json', 'from-codex']) {
    assert.ok([400, 404].includes((await post(app, '/api/skills/import', { source, provider: 'claude' })).status), source);
  }
  assert.equal(await read(join(claudeHome, 'skills', 'from-claude', 'SKILL.md'), 'utf8'),
    '---\nname: from-claude\n---\n\nPrefer plain words.', 'the original is untouched');

  // The skill came from Claude Code; a Codex conversation uses it all the same.
  await json(await post(app, `/api/skills/${skill.id}`, { ...skill, id: undefined, active: true }));
  await json(await post(app, '/api/threads', { environment: { kind: 'host' },
    skillSelection: { skillIds: [skill.id], skillGroupIds: [], revision: 0 } }), 201);
  const { THREAD: thread } = await import('./support/fixtures.mjs');
  await json(await post(app, `/api/threads/${thread}/messages`, { text: 'Hello', clientMessageId: crypto.randomUUID() }), 202);
  const turn = (await app.inspect()).requests.find(r => r.method === 'turn/start');
  assert.match(JSON.stringify(turn.params.additionalContext), /Prefer plain words\./);
});
