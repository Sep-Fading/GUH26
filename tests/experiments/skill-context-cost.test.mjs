// Measures what repeating skill text costs with the real Codex. The installed Codex runs
// against a fake model endpoint in a disposable home: no credentials, no paid inference.
// Question: when MiniClaw sends the same `additionalContext` on every turn, does the model
// receive it once, or does each turn's copy stay in the conversation and pile up?
import assert from 'node:assert/strict';
// Registers the opt-in measurement.
import { test } from 'node:test';
// Serves the fake model endpoint.
import { createServer } from 'node:http';
// Starts the isolated Codex App Server.
import { spawn } from 'node:child_process';
// Waits for the fake endpoint to listen.
import { once } from 'node:events';
// Reads the bundled policy and removes the disposable home.
import { mkdtemp, readFile, rm } from 'node:fs/promises';
// Locates the temporary folder.
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Parses the App Server's line-delimited output.
import { createInterface } from 'node:readline';
// Waits for turns and stops the child.
import { eventually, stopChild } from '../support/harness.mjs';

const MARKER = 'SKILL_COST_MARKER';
const skills = text => ({ miniclaw_skills: { kind: 'application', value: text } });

test('how often the model receives skill text that is sent on every turn', { timeout: 90000,
  skip: process.env.MINICLAW_RUNTIME_PROBE !== '1' && 'set MINICLAW_RUNTIME_PROBE=1 to run' }, async t => {
  const home = await mkdtemp(join(tmpdir(), 'mc-skill-cost-'));
  const bodies = [];
  const notifications = [];
  let child;
  const server = createServer(async (request, response) => {
    let text = '';
    for await (const chunk of request) text += chunk;
    bodies.push(JSON.parse(text));
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const emit = event => response.write(`data: ${JSON.stringify(event)}\n\n`);
    const id = `response-${bodies.length}`;
    const item = { type: 'message', id: `answer-${bodies.length}`, role: 'assistant', phase: 'final_answer', status: 'completed',
      content: [{ type: 'output_text', text: 'Noted.', annotations: [] }] };
    emit({ type: 'response.created', response: { id } });
    emit({ type: 'response.output_item.added', output_index: 0, item });
    emit({ type: 'response.output_item.done', output_index: 0, item });
    emit({ type: 'response.completed', response: { id, output: [item], usage: { input_tokens: 0, output_tokens: 0 } } });
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
  const policy = JSON.parse(await readFile(new URL('../../server/chat-policy.json', import.meta.url)));
  const config = { ...policy, model: 'probe-model', model_provider: 'probe', 'model_providers.probe.name': 'Local test',
    'model_providers.probe.base_url': `http://127.0.0.1:${server.address().port}/v1`,
    'model_providers.probe.wire_api': 'responses', 'model_providers.probe.requires_openai_auth': false };
  const args = ['app-server', '--listen', 'stdio://'];
  for (const [key, value] of Object.entries(config)) args.push('-c', `${key}=${JSON.stringify(value)}`);
  let nextId = 0;
  const pending = new Map();
  const write = frame => child.stdin.write(`${JSON.stringify(frame)}\n`);
  const call = async (method, params) => {
    const id = ++nextId;
    const reply = new Promise(resolve => pending.set(id, resolve));
    write({ id, method, params });
    const result = await reply;
    assert.equal(result.error, undefined, JSON.stringify(result.error));
    return result.result;
  };
  child = spawn(process.env.CODEX_BIN ?? '/Applications/Codex.app/Contents/Resources/codex', args, {
    cwd: home, env: { HOME: home, CODEX_HOME: home, PATH: process.env.PATH }, stdio: ['pipe', 'pipe', 'ignore'] });
  createInterface({ input: child.stdout }).on('line', line => {
    const frame = JSON.parse(line);
    if (frame.method && frame.id !== undefined) write({ id: frame.id, error: { code: -32601, message: 'Unsupported' } });
    else if (frame.id !== undefined) pending.get(frame.id)?.(frame);
    else notifications.push(frame);
  });
  await call('initialize', { clientInfo: { name: 'miniclaw_skill_cost', version: '0.1.0' }, capabilities: { experimentalApi: true } });
  write({ method: 'initialized' });
  const { thread } = await call('thread/start', { cwd: home, model: 'probe-model', sandbox: 'read-only', approvalPolicy: 'never' });

  // Turns 1-3 send the same text, turn 4 sends nothing, turn 5 sends changed text. Turns 6-8 keep
  // the skills under their own key and add a one-turn extra under a second key on turn 7 only.
  const same = `${MARKER} Always answer in British English.`;
  const changed = skills(`${MARKER} CHANGED Always answer in French.`);
  const extra = { miniclaw_slash: { kind: 'application', value: 'SLASH_MARKER Build an MCP server.' } };
  const plan = [skills(same), skills(same), skills(same), undefined, changed, changed, { ...changed, ...extra }, changed];
  const seen = [];
  for (const [index, additionalContext] of plan.entries()) {
    const offset = bodies.length;
    const { turn } = await call('turn/start', { threadId: thread.id, model: 'probe-model', effort: 'low', environments: [],
      ...(additionalContext ? { additionalContext } : {}), input: [{ type: 'text', text: `Turn ${index + 1}` }] });
    await eventually(() => notifications, values => values.some(value => value.method === 'turn/completed' && value.params.turn.id === turn.id),
      `turn ${index + 1}`, 20000);
    const request = JSON.stringify(bodies[offset]);
    seen.push({ turn: index + 1, keys: additionalContext ? Object.keys(additionalContext).join('+') : 'nothing',
      skillCopies: request.split(MARKER).length - 1, slashCopies: request.split('SLASH_MARKER').length - 1 });
  }
  console.log('SKILL_COST', JSON.stringify(seen));
  assert.equal(seen.length, 8);
});
