// Checks runtime inventories and dynamic tool receipts.
import assert from 'node:assert/strict';
// Runs bounded installed runtime compatibility probes.
import { test } from 'node:test';
// Serves fake inference on disposable loopback ports.
import { createServer } from 'node:http';
// Starts isolated Codex App Server processes.
import { spawn } from 'node:child_process';
// Waits for the fake provider listener.
import { once } from 'node:events';
// Reads policy and removes disposable probe homes.
import { mkdtemp, readFile, rm } from 'node:fs/promises';
// Locates the operating system's temporary directory.
import { tmpdir } from 'node:os';
// Builds paths inside the disposable home.
import { join } from 'node:path';
// Parses line-delimited App Server protocol output.
import { createInterface } from 'node:readline';
// Captures production parameters and manages fixtures.
import { launchApp, json, eventually, stopChild } from './support/harness.mjs';
// Identifies the fixture's saved agent conversation.
import { THREAD } from './support/fixtures.mjs';

// Real installed Codex, fake inference, disposable home, no native tools or credentials.
// The default runtime is the macOS app's binary; a Windows run needs an explicit real Codex.
const skip = process.platform === 'win32' && !process.env.CODEX_BIN
  && 'needs a real installed Codex: the default is /Applications/Codex.app on macOS; set CODEX_BIN to run it on Windows';
for (const kind of ['agent', 'delegate', 'project']) {
  const dynamic = kind === 'delegate';
  test(`instruction runtime across restart: ${kind}`, { timeout: 45000, skip }, async t => {
    const app = await launchApp();
    let start, restore;
    const liveTurns = {};
    try {
      const { skill } = await json(await app.request('/api/skills', {
        method: 'POST', body: { name: 'Probe skill', active: true,
          instructions: 'SKILL_PROBE_MARKER: Preserve exact identifiers.' },
      }), 201);
      const { agent } = await json(await app.request('/api/agents', {
        method: 'POST', body: { name: 'Probe', active: true, skillIds: [skill.id],
          instructions: 'Use only explicitly provided tools. Never create or activate agents.' },
      }), 201);
      // A conversation names its machine when it is created.
      let body = { agentId: agent.id, environment: { kind: 'host' } };
      if (kind === 'project') {
        const { project } = await json(await app.request('/api/projects', {
          method: 'POST', body: { name: 'Probe project', skillIds: [skill.id], environment: { kind: 'host' } },
        }), 201);
        body = { projectId: project.id, environment: { kind: 'host' } };
      }
      await json(await app.request('/api/threads', { method: 'POST', body }), 201);
      start = (await app.inspect()).requests.find(r => r.method === 'thread/start').params;
      if (kind === 'project') {
        const { skill: live } = await json(await app.request('/api/skills', {
          method: 'POST', body: { name: 'Live group skill', active: false,
            instructions: 'LIVE_GROUP_MARKER: Preserve exact units.' },
        }), 201);
        const { group } = await json(await app.request('/api/skill-groups', {
          method: 'POST', body: { name: 'Probe group', active: true,
            members: [{ skillId: live.id, enabled: true }] },
        }), 201);
        await json(await app.request(`/api/threads/${THREAD}/skills`, { method: 'POST',
          body: { skillIds: [], skillGroupIds: [group.id], revision: 0 } }));
        for (const state of ['on', 'off']) {
          if (state === 'off') {
            const { id, ...definition } = group;
            await json(await app.request(`/api/skill-groups/${id}`, {
              method: 'POST', body: { ...definition, active: false },
            }));
          }
          const { turnId } = await json(await app.request(`/api/threads/${THREAD}/messages`, {
            method: 'POST', body: { text: 'Probe group', clientMessageId: crypto.randomUUID() },
          }), 202);
          liveTurns[state] = (await app.inspect()).requests.filter(r => r.method === 'turn/start').at(-1).params;
          await app.emit({ method: 'turn/completed', params: { threadId: THREAD,
            turn: { id: turnId, status: 'completed', items: [] } } });
          await eventually(() => app.request(`/api/threads/${THREAD}`).then(json), value => !value.activeTurn);
        }
      }
      await app.restart();
      await json(await app.request(`/api/threads/${THREAD}`));
      restore = (await app.inspect()).requests.find(r => r.method === 'thread/resume').params;
    } finally { await app.close(); }

    const home = await mkdtemp(join(tmpdir(), 'mc-agent-runtime-'));
    const bodies = [];
    const requests = [];
    const notifications = [];
    const issued = new Set();
    let phase = 'initial';
    let child;
    const server = createServer(async (request, response) => {
      let text = '';
      for await (const chunk of request) text += chunk;
      bodies.push(JSON.parse(text));
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      const emit = event => response.write(`data: ${JSON.stringify(event)}\n\n`);
      const responseId = `response-${bodies.length}`;
      emit({ type: 'response.created', response: { id: responseId } });
      let item;
      if (!issued.has(phase)) {
        issued.add(phase);
        item = { type: 'custom_tool_call', id: `item-${phase}`, call_id: `call-${phase}`,
          name: 'exec', namespace: 'functions',
          input: 'text("AGENT_INVENTORY:" + JSON.stringify({' +
            'tools: ALL_TOOLS.map(t => t.name), process: typeof process, require: typeof require}));' +
            (dynamic ? 'text(await tools.delegate_task({agentId: "fixture-agent", assignment: "Test only"}));' : '') };
      } else {
        item = { type: 'message', id: `answer-${phase}`, role: 'assistant',
          phase: 'final_answer', status: 'completed',
          content: [{ type: 'output_text', text: 'Probe finished.', annotations: [] }] };
      }
      emit({ type: 'response.output_item.added', output_index: 0, item });
      emit({ type: 'response.output_item.done', output_index: 0, item });
      emit({ type: 'response.completed', response: { id: responseId, output: [item],
        usage: { input_tokens: 0, output_tokens: 0 } } });
      response.end();
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    t.after(async () => {
      await stopChild(child);
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
      await rm(home, { recursive: true, force: true });
    });
    const policy = JSON.parse(await readFile(new URL('../server/chat-policy.json', import.meta.url)));
    const config = { ...policy, model: 'gpt-6-astra', model_provider: 'probe',
      'model_providers.probe.name': 'Local test',
      'model_providers.probe.base_url': `http://127.0.0.1:${server.address().port}/v1`,
      'model_providers.probe.wire_api': 'responses',
      'model_providers.probe.requires_openai_auth': false };
    const args = ['app-server', '--listen', 'stdio://'];
    for (const [key, value] of Object.entries(config)) args.push('-c', `${key}=${JSON.stringify(value)}`);
    let nextId = 0;
    const pending = new Map();
    // Writes one protocol frame to the current owned process.
    const write = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
    // Correlates a request with its reply and rejects protocol errors.
    async function call(method, params) {
      const id = ++nextId;
      const reply = new Promise(resolve => pending.set(id, resolve));
      write({ id, method, params });
      const result = await reply;
      pending.delete(id);
      assert.equal(result.error, undefined, JSON.stringify(result.error));
      return result.result;
    }
    // Starts an isolated runtime and handles only the test delegation request.
    async function boot() {
      child = spawn(process.env.CODEX_BIN ?? '/Applications/Codex.app/Contents/Resources/codex', args, {
        cwd: home, env: { HOME: home, CODEX_HOME: home, PATH: process.env.PATH },
        stdio: ['pipe', 'pipe', 'ignore'],
      });
      const lines = createInterface({ input: child.stdout });
      lines.on('line', line => {
        const frame = JSON.parse(line);
        if (frame.method && frame.id !== undefined) {
          requests.push({ phase, ...frame });
          if (frame.method === 'item/tool/call' && frame.params.tool === 'delegate_task') {
            write({ id: frame.id, result: { success: true,
              contentItems: [{ type: 'inputText', text: `FAKE_RECEIPT_${phase}` }] } });
          } else {
            write({ id: frame.id, error: { code: -32601, message: 'Unsupported test request' } });
          }
        } else if (frame.id !== undefined) pending.get(frame.id)?.(frame);
        else notifications.push(frame);
      });
      await call('initialize', { clientInfo: { name: 'miniclaw_agent_probe', version: '0.1.0' },
        capabilities: { experimentalApi: true } });
      write({ method: 'initialized' });
    }
    await boot();
    const params = { ...start, cwd: home, model: 'gpt-6-astra' };
    if (dynamic) params.dynamicTools = [{ name: 'delegate_task',
      description: 'Return a fake receipt for protocol testing. Does not start a worker.',
      inputSchema: { type: 'object', properties: {
        agentId: { type: 'string' }, assignment: { type: 'string' },
      }, required: ['agentId', 'assignment'], additionalProperties: false } }];
    const { thread } = await call('thread/start', params);
    for (phase of ['initial', 'resumed', ...(kind === 'project' ? ['group-on', 'group-off'] : [])]) {
      if (phase === 'resumed') {
        await stopChild(child);
        await boot();
        await call('thread/resume', { ...restore, cwd: home, threadId: thread.id, model: 'gpt-6-astra' });
      }
      const offset = bodies.length;
      const overrides = phase.startsWith('group-')
        ? { additionalContext: liveTurns[phase.slice(6)].additionalContext } : {};
      const { turn } = await call('turn/start', { threadId: thread.id, ...overrides,
        model: 'gpt-6-astra', effort: 'low', environments: [],
        input: [{ type: 'text', text: `Probe ${phase}` }] });
      await eventually(() => notifications, values => values.some(value =>
        value.method === 'turn/completed' && value.params.turn.id === turn.id),
      'completed probe turn', 15000);
      const outputs = bodies.slice(offset).flatMap(body => body.input).filter(item =>
        item.type.endsWith('_output') && item.call_id === `call-${phase}`);
      assert.ok(JSON.stringify(bodies[offset]).includes('SKILL_PROBE_MARKER'),
        'The actual inference request must contain pinned skill instructions.');
      if (phase.startsWith('group-')) {
        const latest = JSON.stringify(bodies[offset].input.slice(-3));
        assert.match(latest, phase === 'group-on' ? /LIVE_GROUP_MARKER/ : /Current MiniClaw conversation skills follow/);
        if (phase === 'group-off') assert.doesNotMatch(latest, /LIVE_GROUP_MARKER/);
      }
      const text = outputs.map(item => typeof item.output === 'string' ? item.output :
        (item.output ?? []).map(part => part.text ?? '').join('\n')).join('\n');
      const match = [...text.matchAll(/AGENT_INVENTORY:(\{[^\n]*\})/g)].at(-1);
      assert.ok(match, text);
      const inventory = JSON.parse(match[1]);
      // The model sees the clock and exactly the tools MiniClaw offered when the thread started, nothing else.
      const offered = (start.dynamicTools ?? []).map(tool => tool.name);
      assert.deepEqual(inventory.tools.sort(),
        ['clock__curr_time', ...(dynamic ? ['delegate_task'] : offered)].sort());
      assert.equal(inventory.process, 'undefined');
      assert.equal(inventory.require, 'undefined');
      if (dynamic) {
        assert.match(text, new RegExp(`FAKE_RECEIPT_${phase}`));
        const invocation = requests.filter(r => r.phase === phase && r.params.tool === 'delegate_task');
        assert.equal(invocation.length, 1);
        assert.equal(invocation[0].params.threadId, thread.id);
        assert.equal(invocation[0].params.turnId, turn.id);
        assert.deepEqual(invocation[0].params.arguments, { agentId: 'fixture-agent', assignment: 'Test only' });
      } else assert.equal(requests.length, 0);
      t.diagnostic(`${phase}: ${JSON.stringify(inventory)}`);
    }
    await call('thread/delete', { threadId: thread.id });
    const listed = await call('thread/list', { cwd: home });
    assert.equal(listed.data.some(value => value.id === thread.id), false);
    await eventually(() => notifications, values => values.some(value =>
      value.method === 'thread/deleted' && value.params.threadId === thread.id));
    t.diagnostic('Disposable transcript deleted through installed Codex.');
  });
}
