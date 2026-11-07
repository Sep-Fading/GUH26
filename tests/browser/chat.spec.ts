import { test as base, expect, type Page } from '@playwright/test';
import { launchApp, eventually } from '../support/harness.mjs';
import { signedOut, turn, delta, itemDone, turnDone, thread, message } from '../support/fixtures.mjs';
// Opens the composer's Tools dropdown.
import { openTools, newChatButton } from './settings.js';

type App = Awaited<ReturnType<typeof launchApp>>;
const test = base.extend<{ app: App; signedOutFixture: boolean }>({
  signedOutFixture: [false, { option: true }],
  app: async ({ signedOutFixture }, use) => {
    const app = await launchApp(signedOutFixture ? { 'account/read': { result: signedOut } } : {});
    try { await use(app); } finally { await app.close(); }
  },
});

test('startup status clears without reloading or sending work', async ({ page, app }, info) => {
  let waiting = true;
  let navigations = 0;
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) navigations++; });
  await page.route('**/api/**', route => {
    const path = new URL(route.request().url()).pathname;
    if (!waiting) return route.continue();
    return route.fulfill({ status: path === '/api/health' ? 200 : 503,
      contentType: 'application/json', body: JSON.stringify(path === '/api/health'
        ? { backend: 'ok', codex: 'unavailable', startup: 'waiting_for_worker' }
        : { error: { code: 'worker_starting', outcomeUnknown: false,
          message: 'Unlock the Master VM or finish its setup. MiniClaw will reconnect automatically.' } }),
    });
  });
  await page.goto(app.url);
  await expect(page.getByRole('status')).toHaveText('Waiting for Master VM');
  await expect(page.getByRole('alert')).toContainText('Unlock the Master VM');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath('waiting.png') });
  waiting = false;
  await expect(page.getByRole('status')).toHaveText('Backend connected');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true }))
    .toBeEnabled({ timeout: 20_000 });
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(navigations).toBe(1);
  expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
});

async function openTask(page: Page, app: App) {
  await page.goto(app.url);
  // New chat remains reachable on mobile even when task navigation is collapsed.
  await (await newChatButton(page)).click();
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
}
async function submit(page: Page, app: App, text = 'Hello') {
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill(text);
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/start'), 'message submitted');
}
async function complete(app: App, text: string) {
  const saved = turn('completed', [message(text)]);
  await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { turns: [saved] }) } });
  await app.set('thread/turns/list', { result: { data: [saved], nextCursor: null } });
  await app.emit(itemDone(text), turnDone());
}

test.describe('signed-out account', () => {
  test.use({ signedOutFixture: true });
  test('UI-01: sign-in is visible and sending is unavailable', async ({ page, app }) => {
    await page.goto(app.url);
    await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
    await expect(page.getByText('reader@example.test', { exact: true })).toHaveCount(0);
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  });
});

test('UI-02: composer disables immediately while acknowledgement is pending', async ({ page, app }) => {
  await openTask(page, app);
  await app.set('turn/start', { hold: true });
  await submit(page, app);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
  expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(1);
});

test('UI-03: assistant text appears incrementally and final text is not duplicated', async ({ page, app }) => {
  await openTask(page, app); await submit(page, app);
  const conversation = page.getByRole('region', { name: 'Conversation', exact: true });
  const assistant = conversation.getByRole('article', { name: 'Assistant message', exact: true });
  await app.emit(delta('Hel'));
  await expect(assistant).toHaveText('Hel');
  await app.emit(delta('lo'));
  await expect(assistant).toHaveText('Hello');
  await complete(app, 'Hello.');
  await expect(assistant).toHaveText('Hello.');
  await expect(assistant).toHaveCount(1);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
});

test('UI-04: failed submission preserves the draft and shows an error', async ({ page, app }) => {
  await openTask(page, app);
  await app.set('turn/start', { error: { code: -32602, message: 'Input rejected' } });
  await submit(page, app, 'Keep this draft');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue('Keep this draft');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  await expect(page.getByRole('alert')).toBeVisible();
});

// Keeps storage failures visible and preserves unsent work through recovery.
test('storage refresh failure is visible and recovers without resending', async ({ page, app }) => {
  await openTask(page, app);
  await submit(page, app);
  await complete(app, 'Retained reply');
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(composer).toBeEnabled();
  await composer.fill('Keep this unsent draft');
  let unavailable = true;
  await page.route('**/api/threads', route => unavailable
    ? route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({
      error: { code: 'agent_storage_unavailable',
        message: 'Agent storage is unavailable. No new work was authorized.' },
    }) }) : route.continue());
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByRole('alert')).toContainText('Agent storage is unavailable.');
  await expect(composer).toHaveValue('Keep this unsent draft');
  await expect(composer).toBeDisabled();
  unavailable = false;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(composer).toBeEnabled();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(composer).toHaveValue('Keep this unsent draft');
  await expect(page.getByRole('article', { name: 'Assistant message' })).toHaveText('Retained reply');
  expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(1);
});

test('UI-05: Stop remains pending until Codex confirms interruption', async ({ page, app }) => {
  await openTask(page, app); await submit(page, app); await app.emit(delta('Partial reply'));
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/interrupt'), 'interrupt request');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeDisabled();
  await app.emit(itemDone('Partial reply'), turnDone('interrupted'));
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  await expect(page.getByRole('region', { name: 'Conversation', exact: true })).toContainText('Partial reply');
});

test('UI-06: missing usage shows unavailable instead of a fabricated zero', async ({ page, app }) => {
  await app.set('account/rateLimits/read', { result: { rateLimits: {}, rateLimitsByLimitId: {} } });
  await page.goto(app.url);
  const usage = page.getByRole('region', { name: 'Codex usage', exact: true });
  await expect(usage).toContainText(/unavailable/i);
  await expect(usage).not.toContainText('0%');
});

test('UI-07: Shift+Enter makes a newline; Enter submits once', async ({ page, app }) => {
  await openTask(page, app);
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('First'); await composer.press('Shift+Enter'); await composer.press('a');
  await expect(composer).toHaveValue('First\na');
  expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  await composer.press('Enter');
  const log = await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/start'));
  expect(log.requests.filter(r => r.method === 'turn/start')).toHaveLength(1);
});

test('UI-08: generated Markdown cannot execute scripts or fetch remote images', async ({ page, app }) => {
  let imageRequests = 0;
  await page.route('https://tracking.example/**', route => { imageRequests++; return route.abort(); });
  await openTask(page, app); await submit(page, app);
  const payload = 'Safe text <img src="https://tracking.example/pixel" onerror="window.__xss=1">\n\n![track](https://tracking.example/pixel2)\n\n[unsafe](javascript:window.__xss=1)';
  await app.emit(delta(payload)); await complete(app, payload);
  await expect(page.getByRole('region', { name: 'Conversation', exact: true })).toContainText('Safe text');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  expect(await page.evaluate(() => (window as Window & { __xss?: number }).__xss)).toBeUndefined();
  await expect(page.locator('[onerror], a[href^="javascript:"], img[src^="https://tracking.example"]')).toHaveCount(0);
  expect(imageRequests).toBe(0);
});

test('assistant Markdown renders formatting, copyable code, real links and images only on request',
  async ({ page, app, context }, info) => {
    let imageRequests = 0;
    const pixel = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=', 'base64');
    await page.route('https://img.example/**', route => { imageRequests++;
      return route.fulfill({ contentType: 'image/png', body: pixel }); });
    await openTask(page, app); await submit(page, app);
    const reply = '## Plan\n\n**First** step\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```js\nconst a = 1;\n```\n\n'
      + '[the docs](https://docs.example/page)\n\n![chart](https://img.example/chart.png)';
    await app.emit(delta(reply)); await complete(app, reply);
    const message = page.getByRole('article', { name: 'Assistant message', exact: true });
    await expect(message.getByRole('heading', { name: 'Plan' })).toBeVisible();
    await expect(message.locator('strong')).toHaveText('First');
    await expect(message.getByRole('cell')).toHaveCount(2);
    await expect(message.locator('.hljs-keyword')).toHaveText('const');
    const link = message.getByRole('link', { name: 'the docs' });
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('title', 'https://docs.example/page');
    await context.grantPermissions(['clipboard-read', 'clipboard-write']).catch(() => {});
    await message.getByRole('button', { name: 'Copy' }).click();
    await expect(message.getByRole('button', { name: 'Copied' })).toBeVisible();
    expect(imageRequests).toBe(0);
    await message.getByRole('button', { name: 'Load image from img.example' }).click();
    await expect(message.getByRole('img', { name: 'chart' })).toBeVisible();
    expect(imageRequests).toBe(1);
    await page.screenshot({ path: info.outputPath('markdown-message.png') });
    // The user's own message stays exactly as typed.
    await expect(page.getByRole('article', { name: 'Your message', exact: true }).locator('strong, a, h2')).toHaveCount(0);
  });

test('a turn shows what the tool did: commands with output, edits as coloured diffs, refusals', async ({ page, app }, info) => {
  await openTask(page, app); await submit(page, app);
  const reply = 'I renamed the function and ran the tests.';
  await app.emit(delta(reply)); await complete(app, reply);
  const snapshotUrl = /\/api\/threads\/[0-9a-f-]{36}$/;
  await page.route(snapshotUrl, async route => {
    const response = await route.fetch();
    const body = await response.json();
    const turnId = body.messages.find((m: { role: string }) => m.role === 'assistant')?.turnId;
    body.activities = [
      { id: 'a1', turnId, kind: 'search', title: 'fn old_name', detail: '/Users/me/site/src', status: 'ok',
        startedAt: 1000, endedAt: 1040, output: '/Users/me/site/src/lib.rs:12: fn old_name() {' },
      { id: 'a2', turnId, kind: 'edit', title: '/Users/me/site/src/lib.rs', status: 'ok', startedAt: 2000, endedAt: 2012,
        diff: '@@ -10,5 +10,5 @@\n /// Greets.\n-fn old_name() {\n+fn new_name() {\n     println!("hi");\n }\n' },
      { id: 'a3', turnId, kind: 'command', title: 'cargo test --quiet', detail: '/Users/me/site', status: 'ok',
        startedAt: 3000, endedAt: 7400, exitCode: 101, output: 'running 3 tests\ntest greets ... FAILED' },
      { id: 'a4', turnId, kind: 'read', title: '/Users/me/.ssh/id_ed25519', status: 'refused', startedAt: 8000, endedAt: 8001 },
    ];
    return route.fulfill({ response, json: body });
  });
  await page.reload();
  const list = page.getByRole('group', { name: 'Activity' });
  await expect(list).toContainText('4 steps');
  await expect(list.getByRole('button', { name: /src\/lib\.rs/ })).toContainText('+1');
  await expect(list).toContainText('exit 101');
  await expect(list).toContainText('Refused');
  await expect(list.getByText('+fn new_name() {')).toHaveCount(0);
  await list.getByRole('button', { name: /src\/lib\.rs/ }).click();
  await expect(list.getByText('+fn new_name() {')).toHaveClass(/diff-added/);
  await expect(list.getByText('-fn old_name() {')).toHaveClass(/diff-removed/);
  await list.getByRole('button', { name: /cargo test/ }).click();
  await expect(list.getByText('test greets ... FAILED')).toBeVisible();
  await page.screenshot({ path: info.outputPath('activity.png') });
  await list.getByRole('button', { name: /^Activity/ }).click();
  await expect(list.getByRole('button', { name: /cargo test/ })).toHaveCount(0);
});

test('a delegated run shows as a named block with its activity and answer, never as the parent speaking', async ({ page, app }, info) => {
  await openTask(page, app); await submit(page, app);
  const reply = 'Reviewer found two typos; I fixed both.';
  await app.emit(delta(reply)); await complete(app, reply);
  await page.route(/\/api\/threads\/[0-9a-f-]{36}$/, async route => {
    const response = await route.fetch();
    const body = await response.json();
    const turnId = body.messages.find((m: { role: string }) => m.role === 'assistant')?.turnId;
    body.delegations = [{ id: 'run-1', turnId, thread: null, agent: 'Reviewer', task: 'Review the release notes for typos.',
      mode: 'wait', status: 'done', result: 'Two typos:\n\n- line 4: "teh"\n- line 9: "recieve"', started: 1000, ended: 43000, detached: false }];
    body.activities = [{ id: 'r1', turnId, runId: 'run-1', agent: 'Reviewer', kind: 'read', title: '/Users/me/site/NOTES.md',
      status: 'ok', startedAt: 1, endedAt: 9 }];
    return route.fulfill({ response, json: body });
  });
  await page.reload();
  const block = page.getByRole('group', { name: 'Delegated to Reviewer' });
  await expect(block).toContainText('Finished');
  await expect(block.getByRole('article', { name: 'Answer from Reviewer' })).toContainText('line 4');
  await expect(block.getByRole('group', { name: 'Activity' })).toContainText('NOTES.md');
  await expect(page.getByRole('article', { name: 'Assistant message', exact: true })).toHaveText(reply);
  await page.screenshot({ path: info.outputPath('delegation.png') });
});

// What the conversation shows, top to bottom: its messages, its own Activity lists and run cards.
const conversationOrder = (page: Page) => page.locator('[data-conversation-content]').evaluate(content =>
  [...content.querySelectorAll('article, [role="group"]')]
    .filter(node => !node.parentElement?.closest('[aria-label^="Delegated to"]'))
    .map(node => node.getAttribute('aria-label') === 'Your message' ? `user ${node.textContent?.trim()}`
      : node.getAttribute('aria-label') === 'Assistant message' ? `assistant ${node.textContent?.trim()}`
        : node.getAttribute('aria-label')));

test('a turn stopped before any reply keeps its Activity above the next message, live and after reload', async ({ page, app }, info) => {
  await openTask(page, app); await submit(page, app, 'First');
  const turnId = turn().id;
  const thread = (await app.inspect()).requests.find(r => r.method === 'turn/start')!.params.threadId;
  // One malformed question is refused, then Stop ends the turn before any reply.
  await app.emit({ id: 'ask-1', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: thread, turnId, callId: 'ask-1', arguments: {} } });
  await expect(page.getByRole('group', { name: 'Activity' })).toContainText('A question MiniClaw could not read');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/interrupt'), 'interrupt');
  await app.emit({ method: 'turn/completed', params: { threadId: thread, turn: turn('interrupted') } });
  const next = { id: 'turn-2', status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn: next }, after: [{ method: 'turn/started', params: { threadId: thread, turn: next } }] });
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Second');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.filter(r => r.method === 'turn/start').length === 2, 'second message');
  await app.emit({ method: 'item/completed', params: { threadId: thread, turnId: 'turn-2', item: message('Second answer', 'assistant-2') } },
    { method: 'turn/completed', params: { threadId: thread, turn: { ...next, status: 'completed' } } });
  const expected = ['user First', 'Activity', 'user Second', 'assistant Second answer'];
  await expect.poll(() => conversationOrder(page)).toEqual(expected);
  await page.screenshot({ path: info.outputPath('stopped-turn-order.png') });
  await page.reload();
  await expect(page.getByRole('article', { name: 'Assistant message', exact: true })).toHaveText('Second answer');
  await expect.poll(() => conversationOrder(page)).toEqual(expected);
});

test('the coding session review shows a readable diff, switches to side by side, and writes only after Apply', async ({ page, app }, info) => {
  const { mkdtemp, realpath, writeFile, readFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-review-')));
  const file = join(root, 'greeter.rs');
  const before = ['impl Greeter {', '    /// Greets one person by name.', '    pub fn greet(&self, name: &str) -> String {',
    '        format!("Hello {}", name)', '    }', '', '    /// Counts the greetings so far.', '    pub fn count(&self) -> u32 { self.count }', '}', ''].join('\n');
  const after = before.replace('name: &str) -> String {\n        format!("Hello {}", name)',
    'name: &str, loud: bool) -> String {\n        let text = format!("Hello, {name}");\n        if loud { text.to_uppercase() } else { text }')
    .replace('-> u32 {', '-> u64 {');
  await writeFile(file, before);
  try {
    const post = (path: string, body: unknown) => app.request(path, { method: 'POST', body });
    const { agent } = await (await post('/api/agents', { name: 'Coder', instructions: 'Code.', active: true })).json();
    for (const action of ['read', 'write']) {
      await post(`/api/permissions/agent/${agent.id}`, { target: { target_type: 'folder', path: root, action }, effect: 'allow' });
    }
    const { thread: task } = await (await post('/api/threads', { environment: { kind: 'host' }, agentId: agent.id })).json();
    await post(`/api/permissions/conversation/${task.id}`, { target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow' });
    await page.goto(app.url);
    await expect((await openTools(page)).getByRole('switch', { name: 'Coding session', exact: true })).toBeChecked();
    await submit(page, app, 'Make greet louder');
    await app.emit({ id: 'edit-1', method: 'item/tool/call', params: { tool: 'miniclaw_scoped_native', threadId: task.id,
      turnId: '019a2222-2222-7222-8222-222222222222', callId: 'edit-1', arguments: { action: 'write_file', path: file, text: after } } });
    const dialog = page.getByRole('dialog', { name: 'Review change to greeter.rs' });
    await expect(dialog.getByRole('table', { name: /Changes to/ })).toBeVisible();
    await expect(dialog.locator('mark').first()).toBeVisible();
    expect(await readFile(file, 'utf8')).toBe(before);
    await page.screenshot({ path: info.outputPath('review-unified.png') });
    if (info.project.name === 'desktop') {
      await dialog.getByRole('button', { name: 'Side by side' }).click();
      await expect(dialog.getByRole('table', { name: /side by side/ })).toBeVisible();
      await page.screenshot({ path: info.outputPath('review-split.png') });
      await dialog.getByRole('button', { name: 'Unified' }).click();
    }
    // Keyboard only: focus stays inside the review, and Cmd or Ctrl with Return applies.
    await page.keyboard.press('Tab');
    expect(await dialog.evaluate(el => el.contains(document.activeElement))).toBe(true);
    await page.keyboard.press(process.platform === 'darwin' ? 'Meta+Enter' : 'Control+Enter');
    await expect(dialog).toBeHidden();
    await expect.poll(() => readFile(file, 'utf8')).toBe(after);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('UI-09: completed history returns after a browser refresh', async ({ page, app }) => {
  await openTask(page, app); await submit(page, app); await complete(app, 'A saved answer');
  await expect(page.getByRole('region', { name: 'Conversation', exact: true })).toContainText('A saved answer');
  await page.reload();
  await expect(page.getByRole('region', { name: 'Conversation', exact: true })).toContainText('A saved answer');
  expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(1);
});

test('UI-10: long code does not push the composer beyond the viewport', async ({ page, app }) => {
  await openTask(page, app); await submit(page, app);
  await complete(app, '```text\n' + 'long_identifier_'.repeat(100) + '\n```');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  const metrics = await page.evaluate(() => ({ page: document.documentElement.scrollWidth, viewport: innerWidth }));
  expect(metrics.page).toBeLessThanOrEqual(metrics.viewport + 1);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeInViewport();
});

test('UI-11: external Codex sign-out clears account and conversation across tabs', async ({ page, context, app }) => {
  await openTask(page, app); await submit(page, app); await complete(app, 'Visible before logout');
  const second = await context.newPage(); await second.goto(app.url);
  await expect(second.getByRole('region', { name: 'Conversation', exact: true })).toContainText('Visible before logout');
  await app.set('account/read', { result: signedOut });
  await app.emit({ method: 'account/updated', params: { authMode: null, planType: null } });
  await expect(page.getByText('Visible before logout', { exact: true })).toHaveCount(0);
  await expect(second.getByText('Visible before logout', { exact: true })).toHaveCount(0);
  await expect(second.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  expect((await app.inspect()).requests.filter(r => r.method === 'account/logout')).toHaveLength(0);
});

test('UI-12: an existing Codex session opens chat without a MiniClaw login gate', async ({ page, app }) => {
  await page.goto(app.url);
  await expect(page.getByRole('region', { name: 'Codex usage', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /sign out|log out/i })).toHaveCount(0);
  // On a narrow window the button is in the drawer, so the page is looked at before it is opened.
  const create = await newChatButton(page);
  await expect(create).toBeVisible();
  await create.click();
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  const log = await app.inspect();
  expect(log.requests.filter(r => r.method === 'account/login/start')).toHaveLength(0);
  expect(log.requests.filter(r => r.method === 'account/logout')).toHaveLength(0);
});
