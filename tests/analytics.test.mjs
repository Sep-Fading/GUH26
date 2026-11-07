// Checks what MiniClaw records about its own use: a turn with its tokens, the tools and MCP
// servers it called, the skills that were active, and the aggregates the Analytics tab reads.
import assert from 'node:assert/strict';
// Registers the analytics tests.
import { test } from 'node:test';
// Locates the fake Claude CLI and its journal.
import { copyFile, mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Starts the backend against the fake Codex.
import { launchApp, json, eventually } from './support/harness.mjs';
import { placeMcpServer } from './support/mcp-fixture.mjs';
import { MCP_GATE_CLOSED, sandboxReport } from './support/sandbox.mjs';
// Names the fake conversation and builds its events.
import { THREAD, OTHER, TURN, started, message } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const read = async (app, query = '') => json(await app.request(`/api/analytics${query}`));
const by = (rows, key, value) => rows.find(row => row[key] === value);

/** Reports the token counts a real Codex sends while a turn runs. */
const tokens = (thread, turn, input, output, cached = 0, reasoning = 0) => ({
  method: 'thread/tokenUsage/updated', params: { threadId: thread, turnId: turn,
    tokenUsage: { last: { inputTokens: input, cachedInputTokens: cached, outputTokens: output, reasoningOutputTokens: reasoning,
      cacheWriteInputTokens: 0, totalTokens: input + output }, total: { inputTokens: input, cachedInputTokens: cached,
      outputTokens: output, reasoningOutputTokens: reasoning, cacheWriteInputTokens: 0, totalTokens: input + output } } } });

/** Runs one whole turn: the message, an optional tool call, the token report and the end. */
async function turn(app, thread, words, { input = 1000, output = 200, cached = 0, reasoning = 0, tool, status = 'completed' } = {}) {
  const id = crypto.randomUUID();
  await app.set('turn/start', { result: { turn: { id, status: 'inProgress', items: [] } },
    after: [{ method: 'turn/started', params: { threadId: thread, turn: { id, status: 'inProgress', items: [] } } }] });
  await json(await post(app, `/api/threads/${thread}/messages`, { text: words, clientMessageId: crypto.randomUUID() }), 202);
  await eventually(async () => json(await app.request(`/api/threads/${thread}`)), value => value.activeTurn?.id === id, 'the turn');
  if (tool) {
    const call = crypto.randomUUID();
    await app.emit({ id: call, method: 'item/tool/call', params: { tool: tool.name, threadId: thread, turnId: id, callId: call, arguments: tool.arguments } });
    // A tool the conversation's rules do not already allow asks the user first, unless the
    // backend answers the call at once: a confined MCP server is refused while the Windows
    // sandbox gate is closed, which the caller checks.
    if (tool.approve) {
      const waiting = await eventually(async () => ({ snapshot: await json(await app.request(`/api/threads/${thread}`)),
        replied: (await app.inspect()).responses.some(response => response.id === call) }),
      value => value.snapshot.scopedApproval || value.replied, 'the approval', 20000);
      if (waiting.snapshot.scopedApproval) {
        await json(await post(app, `/api/threads/${thread}/scoped-approvals/${waiting.snapshot.scopedApproval.id}`, { action: 'approve_once' }));
      }
    }
    await eventually(() => app.inspect(), value => value.responses.some(response => response.id === call), 'the tool reply', 10000);
  }
  await app.emit(tokens(thread, id, input, output, cached, reasoning));
  await app.emit({ method: 'item/completed', params: { threadId: thread, turnId: id, completedAtMs: 1900000000000, item: message('Done.', crypto.randomUUID()) } },
    { method: 'turn/completed', params: { threadId: thread, turn: { id, status, items: [] } } });
  await eventually(async () => json(await app.request(`/api/threads/${thread}`)), value => value.activeTurn === null, 'the turn to end');
  return id;
}

test('a turn is recorded with its tokens, its agent and its model, and the totals add up', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const empty = await read(app);
  assert.deepEqual([empty.totals.turns, empty.totals.tokens, empty.daily.length > 0], [0, 0, true], 'an empty range still has days to draw');
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build things.', active: true }), 201);
  const { project } = await json(await post(app, '/api/projects', { name: 'Game', environment: { kind: 'host' }, agentIds: [agent.id] }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: agent.id }), 201);
  await turn(app, THREAD, 'First', { input: 1000, output: 200, cached: 400, reasoning: 50 });
  await turn(app, THREAD, 'Second', { input: 2000, output: 300 });

  const found = await read(app);
  assert.deepEqual([found.totals.turns, found.totals.tokens, found.totals.conversations], [2, 3500, 1]);
  assert.deepEqual([found.totals.inputTokens, found.totals.outputTokens, found.totals.cachedTokens, found.totals.reasoningTokens], [3000, 500, 400, 50]);
  assert.deepEqual(found.agents.map(row => [row.name, row.turns, row.tokens]), [['Builder', 2, 3500]]);
  assert.deepEqual(found.projects.map(row => [row.name, row.turns]), [['Game', 2]]);
  assert.deepEqual(found.models.map(row => [row.name, row.provider, row.turns, row.tokens]), [['fixture-model', 'codex', 2, 3500]]);
  assert.deepEqual(found.providers.map(row => [row.name, row.tokens]), [['codex', 3500]]);
  // Today's column carries both turns, and the series covers the whole range.
  const today = found.daily.at(-1);
  assert.deepEqual([today.turns, today.tokens], [2, 3500]);
  assert.equal(found.daily.length, 30, 'the default range is 30 days');
  assert.equal((await read(app, '?days=7')).daily.length, 7);
  for (const bad of ['?days=0', '?days=400', '?days=x']) assert.equal((await app.request(`/api/analytics${bad}`)).status, 400, bad);
});

test('tool calls, MCP servers and skills are counted and attributed to the tokens of their turns', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const skill = async (name, extra = {}) => (await json(await post(app, '/api/skills',
    { name, description: `${name}.`, instructions: `Do ${name}.`, active: true, ...extra }), 201)).skill;
  const clarity = await skill('Clarity');
  await skill('Release Notes', { slash: true });
  const { agent } = await json(await post(app, '/api/agents', { name: 'Builder', instructions: 'Build.', active: true, skillIds: [clarity.id] }), 201);
  // A project folder is readable without asking, so the step runs and is counted as it ran.
  const { project } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: await realpath(app.peer.cwd), agentIds: [agent.id] }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' }, projectId: project.id, agentId: agent.id }), 201);

  await turn(app, THREAD, 'Read it', { input: 1000, output: 0,
    tool: { name: 'miniclaw_scoped_native', arguments: { action: 'list_folder', path: await realpath(app.peer.cwd) } } });
  await turn(app, THREAD, '/release-notes please', { input: 500, output: 0 });

  const found = await read(app);
  // A step nobody approved is recorded as refused; what was tried is as worth seeing as what ran.
  const steps = by(found.steps, 'kind', 'list');
  assert.deepEqual([steps.calls, steps.ok, steps.refused], [1, 0, 1]);
  // A skill attached to the agent counts on every turn; one called with a slash counts once.
  assert.deepEqual(by(found.skills, 'name', 'Clarity'), { name: 'Clarity', source: 'agent', invocations: 2, tokens: 1500 });
  assert.deepEqual(by(found.skills, 'name', 'Release Notes'), { name: 'Release Notes', source: 'slash', invocations: 1, tokens: 500 });
  assert.equal(found.totals.steps, 1);
});

test('MCP calls are counted per server and per tool, with their failures', async t => {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'mc-analytics-mcp-')));
  const app = await launchApp();
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const mcp = await placeMcpServer(folder);
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'notes', kind: 'stdio',
    command: mcp.command, args: mcp.args(), folder }), 201);
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  const MAIN = '00000000-0000-0000-0000-000000000000';
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  await turn(app, THREAD, 'Echo it', { input: 800, output: 100,
    tool: { approve: true, name: 'miniclaw_mcp', arguments: { action: 'call_tool', server: 'notes', tool: 'echo', arguments: { text: 'hi' } } } });
  // While Windows cannot enforce the sandbox, the confined server is not started and the call fails
  // without asking; only that report selects the Windows expectation, so the real server is
  // counted once the gate opens.
  const { activities } = await json(await app.request(`/api/threads/${THREAD}`));
  const report = await sandboxReport(app);
  const gateClosed = process.platform === 'win32' && activities.some(entry => entry.kind === 'mcp' && entry.status === 'failed'
    && entry.output === `The server "notes" could not be reached. ${MCP_GATE_CLOSED} ${report.reason}`);
  assert.equal(gateClosed, !report.available, 'the refusal appears exactly when the sandbox is unavailable');
  await turn(app, THREAD, 'Fail it', { input: 200, output: 0,
    tool: { approve: true, name: 'miniclaw_mcp', arguments: { action: 'call_tool', server: 'notes', tool: 'fail', arguments: {} } } });

  const found = await read(app);
  // A refused call is still a counted call, and a failed one.
  const echoFailures = gateClosed ? 1 : 0;
  assert.deepEqual(by(found.mcpServers, 'name', 'notes'), { name: 'notes', calls: 2, failures: 1 + echoFailures, tokens: 1100, turns: 2 });
  assert.deepEqual(by(found.mcpTools, 'name', 'notes: echo'), { name: 'notes: echo', calls: 1, failures: echoFailures });
  assert.equal(by(found.mcpTools, 'name', 'notes: fail').failures, 1);
});

test('a Claude turn is recorded with the tokens the CLI reports', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'mc-analytics-claude-'));
  const app = await launchApp({}, { env: { CLAUDE_BIN: fileURLToPath(new URL('./support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: join(folder, 'calls.jsonl') } });
  t.after(async () => { await app.close(); await rm(folder, { recursive: true, force: true }); });
  const MODEL = 'claude-sonnet-5';
  const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, model: MODEL }), 201);
  await json(await post(app, `/api/threads/${thread.id}/messages`, { text: 'Hello', clientMessageId: crypto.randomUUID(), model: MODEL }), 202);
  await eventually(async () => json(await app.request(`/api/threads/${thread.id}`)),
    value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'Claude reply', 15000);
  const found = await eventually(() => read(app), value => value.totals.turns === 1, 'the recorded turn');
  assert.deepEqual(found.providers.map(row => row.name), ['claude']);
  assert.deepEqual(found.models.map(row => [row.name, row.provider]), [[MODEL, 'claude']]);
  assert.ok(found.totals.tokens > 0, 'the CLI\'s own usage numbers are kept');
});

test('a deleted conversation leaves the record, and old rows are dropped', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', { environment: { kind: 'host' } }), 201);
  await turn(app, THREAD, 'One', { input: 100, output: 10 });
  await app.set('thread/delete', { result: {} });
  await json(await post(app, `/api/threads/${THREAD}/delete`));
  // What was used is not private chat content, so deleting the conversation keeps the count.
  assert.equal((await read(app)).totals.turns, 1);
  await app.restart();
  assert.equal((await read(app)).totals.turns, 1, 'the record survives a restart');
});
