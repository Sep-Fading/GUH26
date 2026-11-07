// Checks MCP servers in a real browser: the Settings panel, the switch in a conversation's
// permissions, and the approval popup of one tool.
import { test, expect } from '@playwright/test';
// Prepares a server folder outside MiniClaw's data.
import { copyFile, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
// Locates the temporary folder and the fake server.
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings, openTools } from './settings.js';

/** Makes a folder that holds a copy of the fake MCP server. */
async function serverFolder() {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-mcp-ui-')));
  await copyFile(new URL('../support/fake-mcp.mjs', import.meta.url), join(folder, 'server.mjs'));
  return folder;
}

test('a local server is added, checked, edited to run unrestricted with a warning, and deleted', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const folder = await serverFolder();
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'MCP servers');
    await expect(dialog.getByText('No MCP servers yet.')).toBeVisible();
    await dialog.getByRole('button', { name: 'New server' }).click();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('notes');
    await expect(dialog.getByRole('radio', { name: 'Local command' })).toBeChecked();
    await dialog.getByRole('textbox', { name: 'Command' }).fill(process.execPath);
    await dialog.getByRole('textbox', { name: 'Arguments' }).fill(join(folder, 'server.mjs'));
    await dialog.getByRole('textbox', { name: 'Folder', exact: true }).fill(folder);
    // A server that drives an app through the app's own local socket, as Blender's does on 9876.
    await dialog.getByRole('textbox', { name: 'Local ports' }).fill('9876, 9877');
    // A server that an app connects to, as Roblox Studio's plugin does on 13469.
    await dialog.getByRole('textbox', { name: 'Listening ports' }).fill('13469');
    await dialog.getByRole('textbox', { name: 'Value name' }).fill('API_KEY');
    const value = dialog.getByLabel('Value', { exact: true });
    await expect(value).toHaveAttribute('type', 'password');
    await value.fill('twelve-chars');
    await dialog.getByRole('button', { name: 'Add value' }).click();
    await dialog.getByRole('button', { name: 'Save server' }).click();
    const row = dialog.getByRole('group', { name: 'notes' });
    await expect(row).toContainText('Local command');
    await expect(row).toContainText('Sandboxed');
    await expect(row).toContainText('API_KEY');
    await expect(row).toContainText('Local ports: 9876, 9877');
    await expect(row).toContainText('Listens on: 13469');
    expect((await json(await app.request('/api/mcp/servers'))).servers[0].ports).toEqual([9876, 9877]);
    expect((await json(await app.request('/api/mcp/servers'))).servers[0].listen).toEqual([13469]);
    await expect(dialog.getByText('twelve-chars')).toHaveCount(0);

    await row.getByRole('button', { name: 'Check connection' }).click();
    await expect(row.getByText('write_note')).toBeVisible({ timeout: 15000 });
    await expect(row).toContainText('secret:12');
    await page.screenshot({ path: info.outputPath('mcp-servers.png') });

    await row.getByRole('button', { name: 'Edit' }).click();
    await dialog.getByRole('switch', { name: 'Run outside the sandbox' }).click();
    await expect(dialog.getByRole('alert').filter({ hasText: 'can read and change anything you can' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Save server' }).click();
    await expect(row).toContainText('Unrestricted');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    await row.getByRole('button', { name: 'Delete server' }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Delete server' }).click();
    await expect(dialog.getByText('No MCP servers yet.')).toBeVisible();
  } finally { await app.close(); await rm(folder, { recursive: true, force: true }); }
});

test('a remote server keeps its token write-only, and servers are imported from either provider', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await writeFile(join(app.peer.codexHome, 'config.toml'),
      '[mcp_servers.figma]\nurl = "https://mcp.figma.example/mcp"\nbearer_token_env_var = "FIGMA_TOKEN"\n');
    await page.goto(app.url);
    const dialog = await openSettings(page, 'MCP servers');
    await dialog.getByRole('button', { name: 'New server' }).click();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('tracker');
    await dialog.getByRole('radio', { name: 'Remote address' }).click();
    await dialog.getByRole('textbox', { name: 'Address' }).fill('https://mcp.example.com/mcp');
    const token = dialog.getByLabel('Token', { exact: true });
    await expect(token).toHaveAttribute('type', 'password');
    await token.fill('remote-secret');
    await dialog.getByRole('button', { name: 'Save server' }).click();
    const row = dialog.getByRole('group', { name: 'tracker' });
    await expect(row).toContainText('Token saved');
    await row.getByRole('button', { name: 'Edit' }).click();
    await expect(dialog.getByLabel('Token', { exact: true })).toHaveValue('');
    await dialog.getByRole('button', { name: 'Cancel' }).click();

    await dialog.getByRole('button', { name: 'Import servers' }).click();
    await expect(dialog.getByRole('radio', { name: 'Codex' })).toBeChecked();
    const source = dialog.getByRole('group', { name: 'figma' });
    await expect(source).toContainText('https://mcp.figma.example/mcp');
    await source.getByRole('button', { name: 'Import' }).click();
    const imported = dialog.getByRole('group', { name: 'figma' });
    await expect(imported).toContainText('Enter its token');
    await dialog.getByRole('radio', { name: 'Claude Code' }).click();
    await expect(dialog.getByText('Nothing to import from Claude Code.')).toBeVisible();
  } finally { await app.close(); }
});

test('a conversation switches an allowed server off and on, and the tool popup shows what is about to be sent', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { server } = await json(await app.request('/api/mcp/servers', { method: 'POST',
      body: { name: 'notes', kind: 'stdio', command: '/usr/bin/true', unrestricted: true } }), 201);
    const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
    // Requirement change 2026-09-21: the agent's configuration allows a server, and the conversation
    // switches it from the Tools dropdown.
    await json(await app.request('/api/permissions/agent/00000000-0000-0000-0000-000000000000', { method: 'POST',
      body: { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' } }));
    await page.goto(app.url);
    const toggle = (await openTools(page)).getByRole('switch', { name: 'notes', exact: true });
    await expect(toggle).not.toBeChecked();
    await toggle.click();
    await expect(toggle).toBeChecked();
    await expect.poll(async () => (await json(await app.request(`/api/permissions/conversation/${thread.id}`))).sources.conversation
      .some((rule: { target: { server?: string }; effect: string }) => rule.target.server === server.id && rule.effect === 'allow')).toBe(true);
    await toggle.click();
    await expect(toggle).not.toBeChecked();
    await toggle.click();
    await expect(toggle).toBeChecked();
    await page.keyboard.press('Escape');

    const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
    snapshot.scopedApproval = { id: 'fixture-approval', expiresInSeconds: 60, hosts: [], domains: [],
      target: { target_type: 'mcp_tool', server: server.id, tool: 'write_note' }, serverName: 'notes',
      arguments: { path: '/tmp/note.txt', text: 'hello' }, unrestricted: true, remote: false };
    await page.route(`**/api/threads/${thread.id}`, route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(snapshot) }));
    await page.route(`**/api/threads/${thread.id}/events`, route => route.abort());
    let sent = '';
    await page.route(`**/api/threads/${thread.id}/scoped-approvals/*`, route => {
      sent = route.request().postData() ?? '';
      return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
    });
    await page.reload();
    const dialog = page.getByRole('dialog', { name: 'Approve agent access' });
    await expect(dialog.getByText('MCP server: notes')).toBeVisible();
    await expect(dialog.getByText('Tool: write_note')).toBeVisible();
    await expect(dialog.getByText('"path": "/tmp/note.txt"')).toBeVisible();
    await expect(dialog.getByText('This server runs outside the sandbox')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Approve for this session including whole domains' })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('mcp-approval.png') });
    await dialog.getByRole('button', { name: 'Approve Once', exact: true }).click();
    expect(JSON.parse(sent).action).toBe('approve_once');
  } finally { await app.close(); }
});

test('the provider dropdown limits who may use a server and shows that provider\'s own fields', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'MCP servers');
    await dialog.getByRole('button', { name: 'New server' }).click();
    const provider = dialog.getByRole('combobox', { name: 'Provider' });
    await expect(provider).toContainText('Codex and Claude');
    // With both, every field is there.
    for (const name of ['Startup timeout', 'Tool timeout', 'Enabled tools', 'Disabled tools']) await expect(dialog.getByRole('textbox', { name })).toBeVisible();

    // Claude's own configuration has a timeout and no tool lists.
    await provider.click();
    await page.getByRole('option', { name: 'Claude', exact: true }).click();
    await expect(dialog.getByRole('textbox', { name: 'Tool timeout' })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Enabled tools' })).toHaveCount(0);
    await expect(dialog.getByText('Claude Code names these type, command, args and env.')).toBeVisible();

    // Codex's has both timeouts and both lists.
    await provider.click();
    await page.getByRole('option', { name: 'Codex', exact: true }).click();
    await expect(dialog.getByText('Codex names these command, args, env, cwd, startup_timeout_sec, tool_timeout_sec, enabled_tools and disabled_tools.')).toBeVisible();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('blender');
    await dialog.getByRole('textbox', { name: 'Command' }).fill('/usr/bin/true');
    await dialog.getByRole('textbox', { name: 'Tool timeout' }).fill('900');
    await dialog.getByRole('textbox', { name: 'Disabled tools' }).fill('execute_blender_code');
    await page.screenshot({ path: info.outputPath('mcp-provider.png') });
    await dialog.getByRole('button', { name: 'Save server' }).click();
    const row = dialog.getByRole('group', { name: 'blender' });
    await expect(row).toContainText('Codex only');
    const saved = (await json(await app.request('/api/mcp/servers'))).servers[0];
    expect([saved.provider, saved.toolSeconds, saved.disabledTools, saved.startupSeconds]).toEqual(['codex', 900, ['execute_blender_code'], 15]);

    // A remote server for Claude takes headers; their values are typed like a password and never shown again.
    await dialog.getByRole('button', { name: 'New server' }).click();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('tracker');
    await dialog.getByRole('radio', { name: 'Remote address' }).click();
    await dialog.getByRole('textbox', { name: 'Address' }).fill('https://mcp.example.com/mcp');
    await dialog.getByRole('textbox', { name: 'Header name' }).fill('X-Api-Key');
    const value = dialog.getByLabel('Header value', { exact: true });
    await expect(value).toHaveAttribute('type', 'password');
    await value.fill('header-secret');
    await dialog.getByRole('button', { name: 'Add header' }).click();
    await dialog.getByRole('button', { name: 'Save server' }).click();
    await expect(dialog.getByRole('group', { name: 'tracker' })).toContainText('Headers: X-Api-Key');
    await expect(dialog.getByText('header-secret')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); }
});
