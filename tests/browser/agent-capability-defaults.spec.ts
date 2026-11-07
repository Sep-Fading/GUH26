// Checks saved agent defaults separately from permission allowances.
import { test, expect } from '@playwright/test';
// Starts an isolated application and checks API replies.
import { launchApp, json } from '../support/harness.mjs';
// Navigates settings and the conversation tools menu.
import { openSettings, openTools } from './settings.js';

test('agent defaults page saves Blender activation for new conversations', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const post = (path: string, body: unknown) => app.request(path, { method: 'POST', body });
    const { agent } = await json(await post('/api/agents', { name: 'Artist', instructions: 'Draw.', active: true }), 201);
    const { server } = await json(await post('/api/mcp/servers', { name: 'blender', kind: 'stdio', command: '/usr/bin/true' }), 201);
    await json(await post(`/api/permissions/agent/${agent.id}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
    await page.goto(app.url);
    await openSettings(page, 'Agents');
    await page.getByRole('navigation', { name: 'Agent profiles' }).getByRole('button', { name: 'Artist', exact: true }).click();
    await page.getByRole('tab', { name: 'Enabled by default', exact: true }).click();
    const panel = page.getByRole('tabpanel', { name: 'Enabled by default', exact: true });
    const blender = panel.getByRole('switch', { name: 'blender', exact: true });
    await expect(blender).not.toBeChecked();
    await page.route('**/capability-defaults', route => route.request().method() === 'POST'
      ? route.fulfill({ status: 503, json: { error: { message: 'Default unavailable' } } }) : route.continue(), { times: 1 });
    await blender.click();
    await expect(panel.getByRole('alert')).toContainText('Default unavailable');
    await expect(blender).not.toBeChecked();
    await blender.click();
    await expect(blender).toBeChecked();
    await expect(blender).toBeEnabled();
    await expect(panel.getByRole('switch', { name: 'Network for commands', exact: true })).toBeDisabled();
    await page.screenshot({ path: info.outputPath('agent-defaults.png'), animations: 'disabled' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.reload();
    await openSettings(page, 'Agents');
    await page.getByRole('navigation', { name: 'Agent profiles' }).getByRole('button', { name: 'Artist', exact: true }).click();
    await page.getByRole('tab', { name: 'Enabled by default', exact: true }).click();
    await expect(page.getByRole('tabpanel', { name: 'Enabled by default' }).getByRole('switch', { name: 'blender', exact: true })).toBeChecked();
    await page.keyboard.press('Escape');
    await page.getByRole('combobox', { name: 'Agent', exact: true }).click();
    await page.getByRole('option', { name: 'Artist', exact: true }).click();
    const tools = await openTools(page);
    await expect(tools.getByRole('switch', { name: 'blender', exact: true })).toBeChecked();
    await tools.getByRole('switch', { name: 'blender', exact: true }).click();
    await expect(tools.getByRole('switch', { name: 'blender', exact: true })).not.toBeChecked();
  } finally { await app.close(); }
});
