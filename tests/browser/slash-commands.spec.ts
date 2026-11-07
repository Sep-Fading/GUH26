// Checks the composer's slash menu and the card with which /mcp-create hands a server over.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Builds a saved conversation whose last answer holds a hand-over block.
import { message, turn } from '../support/fixtures.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

test('typing a slash lists the commands, and choosing one fills it in', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await json(await app.request('/api/skills', { method: 'POST', body: { name: 'Release Notes', description: 'Writes release notes.',
      instructions: 'Write release notes.', active: true, slash: true } }), 201);
    await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
    await page.goto(app.url);
    const box = page.getByRole('textbox', { name: 'Message' });
    await box.fill('/');
    const menu = page.getByRole('listbox', { name: 'Slash commands' });
    await expect(menu.getByRole('option')).toHaveText([/\/mcp-create.*Guides you through building an MCP server/, /\/skill-create.*Writes a new skill/,
      /\/agent-create.*Writes a new agent/, /\/release-notes.*Writes release notes\./]);
    await box.fill('/rel');
    await expect(menu.getByRole('option')).toHaveCount(1);
    await page.screenshot({ path: info.outputPath('slash-menu.png') });
    await box.press('Enter');
    await expect(box).toHaveValue('/release-notes ');
    await expect(menu).toHaveCount(0);
    // Escape closes the menu and keeps the text; ordinary text shows no menu.
    await box.fill('/mcp');
    await expect(menu).toBeVisible();
    await box.press('Escape');
    await expect(menu).toHaveCount(0);
    await expect(box).toHaveValue('/mcp');
    await box.fill('a/b');
    await expect(menu).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); }
});

test('a skill can be marked callable with a slash', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Skills');
    await dialog.getByRole('button', { name: 'New skill' }).click();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Release Notes');
    await dialog.getByRole('textbox', { name: 'Instructions' }).fill('Write release notes.');
    await dialog.getByRole('checkbox', { name: 'Callable with a slash' }).check();
    await expect(dialog.getByText('/release-notes')).toBeVisible();
    await dialog.getByRole('button', { name: 'Save skill' }).click();
    await expect.poll(async () => (await json(await app.request('/api/skills'))).skills[0]?.slash).toBe(true);
  } finally { await app.close(); }
});

test('the hand-over card opens the server form filled in, sandboxed, and nothing is saved until Save', async ({ page }, info) => {
  const proposal = { name: 'notes', command: '/usr/bin/true', args: ['--stdio'], folder: '/private/tmp', hosts: ['api.example.com'],
    env: ['NOTES_API_KEY'], unrestricted: true };
  const answer = `It is built and tested.\n\n\`\`\`miniclaw-mcp-server\n${JSON.stringify(proposal)}\n\`\`\``;
  const app = await launchApp({ 'thread/turns/list': { result: { data: [turn('completed', [
    { id: 'user-1', type: 'userMessage', content: [{ type: 'text', text: '/mcp-create notes', text_elements: [] }] },
    message(answer, 'reply-1')])], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const card = page.getByRole('group', { name: 'MCP server notes' });
    await expect(card).toContainText('/usr/bin/true --stdio');
    await expect(card).toContainText('NOTES_API_KEY');
    await page.screenshot({ path: info.outputPath('mcp-card.png') });
    await card.getByRole('button', { name: 'Add this server to MiniClaw' }).click();
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(dialog.getByRole('heading', { name: 'MCP servers' })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('notes');
    await expect(dialog.getByRole('textbox', { name: 'Command' })).toHaveValue('/usr/bin/true');
    await expect(dialog.getByRole('textbox', { name: 'Folder', exact: true })).toHaveValue('/private/tmp');
    await expect(dialog.getByRole('textbox', { name: 'Allowed hosts' })).toHaveValue('api.example.com');
    await expect(dialog.getByRole('textbox', { name: 'Value name' })).toHaveValue('NOTES_API_KEY');
    await expect(dialog.getByRole('switch', { name: 'Run outside the sandbox' })).not.toBeChecked();
    expect((await json(await app.request('/api/mcp/servers'))).servers).toEqual([]);
    await dialog.getByRole('button', { name: 'Save server' }).click();
    await expect(dialog.getByRole('group', { name: 'notes' })).toContainText('Sandboxed');
    // A secret the user did not enter yet is remembered as still to enter, not dropped.
    await expect(dialog.getByRole('group', { name: 'notes' })).toContainText('Enter NOTES_API_KEY');
    const saved = (await json(await app.request('/api/mcp/servers'))).servers[0];
    expect([saved.unrestricted, saved.hosts, saved.needs]).toEqual([false, ['api.example.com'], ['NOTES_API_KEY']]);
  } finally { await app.close(); }
});
