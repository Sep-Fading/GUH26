#!/usr/bin/env node
// Synthetic Claude Code CLI shaped against claude 2.1.278 `-p --output-format stream-json`.
// Records each invocation in MC_CLAUDE_LOG; the prompt text selects the scenario.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';

const args = process.argv.slice(2);
const log = entry => appendFileSync(process.env.MC_CLAUDE_LOG, `${JSON.stringify(entry)}\n`);
const line = value => process.stdout.write(`${JSON.stringify(value)}\n`);
// The real CLI reports the turn's tokens in its last frame (claude 2.1.278).
const usage = { input_tokens: 1200, output_tokens: 150, cache_read_input_tokens: 300, cache_creation_input_tokens: 0 };
// Initialization models use Claude's public picker metadata; tests can replace the file live.
const models = [
  ['claude-fable-5-1', 'Claude Fable 5.1'], ['claude-opus-5', 'Claude Opus 5'],
  ['claude-sonnet-5', 'Claude Sonnet 5'], ['claude-haiku-4-5-20251001', 'Claude Haiku 4.5'],
].map(([value, displayName]) => ({ value, resolvedModel: value, displayName, description: '',
  supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'] }));

// `auth login` records a marker file; `auth status` is signed in once it exists.
const marker = process.env.MC_CLAUDE_LOGIN_FILE;
if (args[0] === 'auth' && args[1] === 'login') await login();

// Prints what `claude auth login --claudeai` prints (2.1.289 on Windows, 2.1.274 on macOS,
// work/windows-vm/claude-login-exp-vm1.log), then finishes the way MC_CLAUDE_LOGIN_MODE says:
// `auto` as if the browser came back to the CLI's own localhost page, `paste` once a
// `code#state` line arrives on stdin, `fail` at once with the CLI's error. The real CLI opens
// the localhost-redirect page itself unless BROWSER names another opener; MC_CLAUDE_BROWSER_LOG
// records which happened. Every value is made up.
async function login() {
  const mode = process.env.MC_CLAUDE_LOGIN_MODE ?? 'auto';
  const state = 'fake-state-0123456789';
  if (process.env.MC_CLAUDE_LOGIN_PID) writeFileSync(process.env.MC_CLAUDE_LOGIN_PID, String(process.pid));
  if (mode === 'fail') {
    process.stderr.write('Login failed: Request failed with status code 403\n');
    process.exit(1);
  }
  const page = redirect => 'https://claude.com/cai/oauth/authorize?code=true&client_id=00000000-0000-4000-8000-000000000000'
    + `&response_type=code&redirect_uri=${encodeURIComponent(redirect)}&scope=user%3Aprofile+user%3Ainference`
    + `&code_challenge=fake-challenge&code_challenge_method=S256&state=${state}`;
  process.stdout.write('Opening browser to sign in\u2026\n');
  process.stdout.write(`If the browser didn't open, visit: ${page('https://platform.claude.com/oauth/code/callback')}\n`);
  process.stdout.write('Paste code here if prompted > ');
  // Like the real CLI, a BROWSER program is run with the localhost-callback page as its one argument.
  const automatic = page('http://localhost:54545/callback');
  const opener = process.env.BROWSER
    ? { opener: 'BROWSER', browser: process.env.BROWSER, status: spawnSync(process.env.BROWSER, [automatic], { stdio: 'ignore' }).status }
    : { opener: 'cli', url: automatic };
  if (process.env.MC_CLAUDE_BROWSER_LOG) appendFileSync(process.env.MC_CLAUDE_BROWSER_LOG, `${JSON.stringify(opener)}\n`);
  const finish = () => {
    if (marker) writeFileSync(marker, 'signed-in');
    process.stdout.write('Login successful.\n');
    process.exit(0);
  };
  if (mode === 'auto') setTimeout(finish, 300);
  createInterface({ input: process.stdin }).on('line', line => {
    const [code, given, extra] = line.trim().split('#');
    if (!code || !given || extra !== undefined) {
      process.stderr.write('Invalid code. Please make sure the full code was copied.\n');
      return;
    }
    if (process.env.MC_CLAUDE_CODE_LOG) appendFileSync(process.env.MC_CLAUDE_CODE_LOG, `${line.trim()}\n`);
    if (given !== state || code === 'rejected') {
      process.stderr.write('Login failed: Authentication failed: Invalid authorization code\n');
      process.exit(1);
    }
    finish();
  });
  // The CLI keeps waiting for the browser after its input closes.
  await new Promise(() => setInterval(() => {}, 1000));
}
if (args[0] === 'auth' && args[1] === 'status') {
  if (process.env.MC_CLAUDE_STATUS_LOG) appendFileSync(process.env.MC_CLAUDE_STATUS_LOG, `${Date.now()}\n`);
  const signedOut = process.env.MC_CLAUDE_SIGNED_OUT && !(marker && existsSync(marker));
  line(signedOut ? { loggedIn: false }
    : { loggedIn: true, authMethod: 'claude.ai', email: 'claude-reader@example.test', subscriptionType: 'max' });
  process.exit(0);
}

let prompt = '';
for await (const chunk of process.stdin) prompt += chunk;
prompt = prompt.trim();
// Read-only control traffic is separate from prompts and consumes no model turn.
if (prompt.startsWith('{') && JSON.parse(prompt.split('\n')[0]).type === 'control_request') {
  const requests = prompt.split('\n').map(value => JSON.parse(value));
  const modelRead = requests.length === 1 && requests[0].request.subtype === 'initialize';
  if (modelRead && process.env.MC_CLAUDE_MODEL_LOG) appendFileSync(process.env.MC_CLAUDE_MODEL_LOG,
    `${JSON.stringify({ args, requests, apiKey: process.env.ANTHROPIC_API_KEY ?? null })}\n`);
  if (!modelRead && process.env.MC_CLAUDE_LOG) log({ args, requests, apiKey: process.env.ANTHROPIC_API_KEY ?? null });
  for (const { request_id, request } of requests) {
    const response = request.subtype === 'get_usage'
      ? (process.env.MC_CLAUDE_USAGE_FILE ? JSON.parse(readFileSync(process.env.MC_CLAUDE_USAGE_FILE, 'utf8'))
        : { subscription_type: 'max', rate_limits_available: true, rate_limits: {
          five_hour: { utilization: 10, resets_at: '2033-05-18T03:33:20Z' },
          seven_day: { utilization: 51, resets_at: '2033-05-21T03:33:20Z' },
          model_scoped: [{ display_name: 'Sonnet', utilization: 98, resets_at: '2033-05-21T03:33:20Z' }],
        } }) : (process.env.MC_CLAUDE_MODELS_FILE
          ? JSON.parse(readFileSync(process.env.MC_CLAUDE_MODELS_FILE, 'utf8')) : { models });
    line({ type: 'control_response', response: { request_id,
      subtype: response.error ? 'error' : 'success', ...(response.error ? { error: response.error } : { response }) } });
  }
  process.exit(0);
}
// With `--input-format stream-json` the message is one JSON line of content blocks, which is how
// a picture arrives; the scenario is still chosen by the words.
let content = null;
if (args[args.indexOf('--input-format') + 1] === 'stream-json' && args.includes('--input-format')) {
  content = JSON.parse(prompt).message.content;
  prompt = content.filter(block => block.type === 'text').map(block => block.text).join('\n').trim();
}
log({ args, prompt, content, cwd: process.cwd(), apiKey: process.env.ANTHROPIC_API_KEY ?? null });
// What MiniClaw adds for one turn comes before the user's words; the scenario is chosen by the words.
prompt = prompt.replace(/^<miniclaw-context>[\s\S]*?<\/miniclaw-context>\s*/, '');

const option = name => args.includes(name) ? args[args.indexOf(name) + 1] : null;
const session = option('--session-id') ?? option('--resume');
const event = value => line({ type: 'stream_event', event: value, session_id: session });
line({ type: 'system', subtype: 'init', session_id: session, tools: [], model: option('--model') });
event({ type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '' } });
event({ type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'private' } });
event({ type: 'content_block_stop', index: 0 });
if (prompt === 'fail') {
  line({ type: 'result', subtype: 'error_during_execution', is_error: true, session_id: session });
  process.exit(1);
}
// `tool {json}` makes this CLI act as an MCP client of the configured server, call its
// one tool with those arguments, and answer with the tool's text.
// `delegate {json}` does the same with the delegation tool, `mcp {json}` with the MCP gateway, `ask {json}` with the question tool,
// and `tools` only lists the names.
const named = prompt.startsWith('delegate ') ? 'miniclaw_delegate' : prompt.startsWith('mcp ') ? 'miniclaw_mcp'
  : prompt.startsWith('ask ') ? 'miniclaw_ask' : prompt.startsWith('project ') ? 'miniclaw_project' : null;
if (prompt.startsWith('tool ') || named || prompt === 'tools') {
  const server = Object.values(JSON.parse(option('--mcp-config')).mcpServers)[0];
  const child = spawn(server.command, server.args, { env: { ...process.env, ...server.env },
    stdio: ['pipe', 'pipe', 'ignore'] });
  const replies = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  const request = async (id, method, params) => {
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    for (;;) { const reply = JSON.parse((await replies.next()).value); if (reply.id === id) return reply.result; }
  };
  await request(1, 'initialize', { protocolVersion: '2025-06-18', capabilities: {},
    clientInfo: { name: 'fake-claude', version: '0' } });
  child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
  const { tools } = await request(2, 'tools/list', {});
  const tool = named ? tools.find(entry => entry.name === named) : tools[0];
  const result = prompt === 'tools' || !tool ? { content: [{ text: tools.map(entry => entry.name).join(',') }] }
    : await request(3, 'tools/call', { name: tool.name, arguments: JSON.parse(prompt.slice(prompt.indexOf(' ') + 1)) });
  child.kill();
  const answer = `${tool?.name ?? 'none'} error=${result.isError === true}: ${result.content.map(part => part.type === 'image' ? `[image ${part.mimeType} ${part.data.length}]` : part.text).join('|')}`;
  event({ type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } });
  event({ type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: answer } });
  event({ type: 'content_block_stop', index: 1 });
  line({ type: 'result', subtype: 'success', is_error: false, result: answer, usage, session_id: session });
  process.exit(0);
}
event({ type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } });
event({ type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'Hello ' } });
if (prompt === 'hang') await new Promise(() => setInterval(() => {}, 1000));
event({ type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'from Claude' } });
event({ type: 'content_block_stop', index: 1 });
line({ type: 'result', subtype: 'success', is_error: false, result: 'Hello from Claude', usage, session_id: session });
