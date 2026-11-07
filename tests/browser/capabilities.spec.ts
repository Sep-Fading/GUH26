// Checks the composer's Tools dropdown: one place to switch a conversation's capabilities, where
// what the agent does not allow is greyed out and says why.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Resolves the isolated Claude test executable.
import { fileURLToPath } from 'node:url';
// Opens the Tools dropdown.
import { openTools, newChatButton, openSettings } from './settings.js';

const MAIN = '00000000-0000-0000-0000-000000000000';

test('tools are configurable before the first prompt', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const post = (path: string, body: unknown) => app.request(path, { method: 'POST', body });
    const { server } = await json(await post('/api/mcp/servers', { name: 'notes', kind: 'stdio', command: '/usr/bin/true' }), 201);
    await json(await post(`/api/permissions/agent/${MAIN}`, { target: { target_type: 'feature', name: 'network' }, effect: 'allow' }));
    await json(await post(`/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await input.fill('Use my selected tools on this first prompt');
    const tools = await openTools(page);
    for (const name of ['Network for commands', 'notes', 'Coding session', 'Full access']) {
      const toggle = tools.getByRole('switch', { name, exact: true });
      await expect(toggle).toBeEnabled();
      await toggle.click();
      await expect(toggle).toBeChecked();
    }
    await tools.getByRole('switch', { name: 'Web search', exact: true }).click();
    await expect(tools.getByRole('switch', { name: 'Web search', exact: true })).not.toBeChecked();
    await page.screenshot({ path: info.outputPath('draft-tools.png'), animations: 'disabled' });
    expect((await app.inspect()).requests.some(r => r.method === 'thread/start' || r.method === 'turn/start')).toBe(false);
    await page.getByRole('button', { name: 'Tools', exact: true }).click();

    // A failed creation leaves both the prompt and its switches ready for an explicit retry.
    await page.route('**/api/threads', route => route.request().method() === 'POST'
      ? route.fulfill({ status: 503, json: { error: { message: 'Creation unavailable' } } }) : route.continue(), { times: 1 });
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Creation unavailable');
    await expect(input).toHaveValue('Use my selected tools on this first prompt');
    await expect((await openTools(page)).getByRole('switch', { name: 'notes', exact: true })).toBeChecked();
    await page.getByRole('button', { name: 'Tools', exact: true }).click();
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    const requests = (await app.inspect()).requests;
    expect(requests.filter(r => r.method === 'thread/start')).toHaveLength(1);
    expect(requests.find(r => r.method === 'thread/start')?.params.config?.web_search).not.toBe('live');
    const first = requests.find(r => r.method === 'turn/start')?.params.additionalContext.miniclaw_environment.value;
    expect(first).toMatch(/Network for commands: on/);
    expect(first).toMatch(/MCP servers: notes/);
    expect(first).toMatch(/Coding session.*: on/);
    expect(first).toMatch(/Full access.*: on/);

    await (await newChatButton(page)).click();
    const fresh = await openTools(page);
    for (const name of ['Network for commands', 'notes', 'Coding session', 'Full access']) {
      await expect(fresh.getByRole('switch', { name, exact: true })).not.toBeChecked();
    }
    await expect(fresh.getByRole('switch', { name: 'Web search', exact: true })).toBeChecked();
  } finally { await app.close(); }
});

test('capabilities are switched from one dropdown; what the agent does not allow is greyed and explained', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await json(await app.request('/api/mcp/servers', { method: 'POST', body: { name: 'notes', kind: 'stdio', command: '/usr/bin/true' } }), 201);
    const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
    await page.goto(app.url);
    const tools = await openTools(page);
    // The 2026-09-26 timed shell requirement adds its own off-by-default switch.
    await expect(tools.getByRole('switch')).toHaveCount(14);
    for (const name of ['Web search', 'Image generation', 'Network for commands', 'Unsandboxed shell', 'Web fetch', 'Subagents', 'Task tracking', 'notes', 'Computer Use', 'Ask me questions', 'Plan mode', 'Coding session', 'Ask about every command', 'Full access']) {
      await expect(tools.getByRole('switch', { name, exact: true })).toBeVisible();
    }
    await expect(tools.getByRole('switch', { name: 'Unsandboxed shell', exact: true })).not.toBeChecked();
    await expect(tools.getByRole('switch', { name: 'Web search', exact: true })).toBeChecked();
    const network = tools.getByRole('switch', { name: 'Network for commands', exact: true });
    await expect(network).toBeDisabled();
    await tools.getByText('Network for commands', { exact: true }).hover();
    await expect(page.getByRole('tooltip')).toContainText('Not available for the main agent');
    await page.screenshot({ path: info.outputPath('tools.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    // Once the main agent allows it, the conversation can switch it on, and off again.
    await json(await app.request(`/api/permissions/agent/${MAIN}`, { method: 'POST', body: { target: { target_type: 'feature', name: 'network' }, effect: 'allow' } }));
    // The change was made outside this page, so the page is loaded again, as after a visit to Settings.
    await page.reload();
    const again = await openTools(page);
    const allowed = again.getByRole('switch', { name: 'Network for commands', exact: true });
    await expect(allowed).toBeEnabled();
    await expect(allowed).not.toBeChecked();
    await allowed.click();
    await expect(allowed).toBeChecked();
    await allowed.click();
    await expect(allowed).not.toBeChecked();
    const capabilities = async () => (await json(await app.request(`/api/capabilities?thread=${thread.id}`))).capabilities;
    expect((await capabilities()).find((item: { key: string }) => item.key === 'network').on).toBe(false);
    await again.getByRole('switch', { name: 'Web search', exact: true }).click();
    await expect.poll(async () => (await capabilities()).find((item: { key: string }) => item.key === 'web_search').on).toBe(false);
    // The trigger counts what is on.
    // With search and network off, asking is the one thing still on.
    await expect(page.getByRole('button', { name: 'Tools', exact: true })).toContainText('1');
  } finally { await app.close(); }
});

test('project memory is a switch in Tools, on by default, and only a project conversation has it', async ({ page }) => {
  const app = await launchApp();
  try {
    await json(await app.request('/api/projects', { method: 'POST', body: { name: 'Game', environment: { kind: 'host' } } }), 201);
    await page.goto(app.url);
    await expect((await openTools(page)).getByRole('switch', { name: 'Project memory', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Tools', exact: true }).click();
    // The dropdown has to be gone before it is opened again.
    await expect(page.getByRole('dialog', { name: 'Tools for this conversation' })).toHaveCount(0);
    const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
    if (await toggle.isVisible()) await toggle.click();
    await page.getByRole('button', { name: 'New chat in Game', exact: true }).click();
    await expect((await openTools(page)).getByRole('switch', { name: 'Project memory', exact: true })).toBeChecked();
  } finally { await app.close(); }
});

test('full access is a switch in Tools that shows a red badge on the composer, off by default', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    await (await newChatButton(page)).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Start');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await expect(page.getByRole('status').filter({ hasText: 'Full access' })).toHaveCount(0);
    const tools = await openTools(page);
    await expect(tools.getByRole('switch', { name: 'Ask about every command', exact: true })).not.toBeChecked();
    const full = tools.getByRole('switch', { name: 'Full access', exact: true });
    await expect(full).not.toBeChecked();
    // Requirement change 2026-09-22: a switch flips during the response too, and the badge follows.
    await expect(full).toBeEnabled();
    await full.click();
    await expect(full).toBeChecked();
    await expect(page.getByRole('status').filter({ hasText: 'Full access' })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  } finally { await app.close(); }
});


test('Claude tool switches stay independent after reload', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } },
    { env: { CLAUDE_BIN: fileURLToPath(new URL('../support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: '/dev/null' } });
  try {
    for (const name of ['web_fetch', 'subagents', 'task_tracking']) {
      await json(await app.request(`/api/permissions/agent/${MAIN}`, { method: 'POST',
        body: { target: { target_type: 'feature', name }, effect: 'allow' } }));
    }
    const { thread } = await json(await app.request('/api/threads', { method: 'POST',
      body: { environment: { kind: 'host' }, model: 'claude-sonnet-5' } }), 201);
    // The same independent allowances are exposed in the main agent's settings.
    await page.goto(app.url);
    const settings = await openSettings(page, 'Agents');
    await settings.getByRole('button', { name: 'Main agent', exact: true }).click();
    const allowances = settings.getByRole('group', { name: 'Capabilities' });
    for (const name of ['Web fetch', 'Subagents', 'Task tracking']) {
      await expect(allowances.getByRole('switch', { name, exact: true })).toBeChecked();
    }
    await allowances.getByRole('switch', { name: 'Web fetch', exact: true }).click();
    await expect(allowances.getByRole('switch', { name: 'Subagents', exact: true })).toBeChecked();
    await expect(allowances.getByRole('switch', { name: 'Task tracking', exact: true })).toBeChecked();
    await allowances.getByRole('switch', { name: 'Web fetch', exact: true }).click();
    await expect(allowances.getByRole('switch', { name: 'Web fetch', exact: true })).toBeChecked();

    await page.goto(app.url);
    let tools = await openTools(page);
    await expect(tools.getByText('Web fetch and planning tools', { exact: true })).toHaveCount(0);
    for (const name of ['Web fetch', 'Subagents', 'Task tracking']) {
      await expect(tools.getByRole('switch', { name, exact: true })).toBeEnabled();
      await expect(tools.getByRole('switch', { name, exact: true })).not.toBeChecked();
    }
    await tools.getByRole('switch', { name: 'Task tracking', exact: true }).click();
    await expect(tools.getByRole('switch', { name: 'Task tracking', exact: true })).toBeChecked();
    await page.reload();
    tools = await openTools(page);
    await expect(tools.getByRole('switch', { name: 'Task tracking', exact: true })).toBeChecked();
    for (const name of ['Web fetch', 'Subagents', 'Plan mode']) {
      await expect(tools.getByRole('switch', { name, exact: true })).not.toBeChecked();
    }
    await expect(tools.getByRole('switch', { name: 'Web search', exact: true })).toBeChecked();
    const { capabilities } = await json(await app.request(`/api/capabilities?thread=${thread.id}`));
    expect(capabilities.filter((item: { on: boolean }) => item.on).map((item: { key: string }) => item.key))
      .toEqual(['web_search', 'task_tracking', 'questions']);
    await page.screenshot({ path: info.outputPath('separate-tools.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); }
});
