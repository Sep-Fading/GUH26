// Checks naming a conversation from its row in the sidebar, and taking the name back.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, eventually } from '../support/harness.mjs';
// Opens an empty composer; the sidebar is a drawer on a narrow window.
import { newChatButton } from './settings.js';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

test('a conversation is renamed from its row and can get its original name back', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    await (await newChatButton(page)).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Create a low-poly tree');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/start'), 'turn');
    // Reaching the row opens the drawer on a narrow window.
    await newChatButton(page);
    const tasks = page.getByRole('navigation', { name: 'Tasks' });
    await tasks.getByRole('button', { name: 'Rename chat Create a low-poly tree' }).click();
    const dialog = page.getByRole('dialog', { name: 'Rename chat' });
    const field = dialog.getByRole('textbox', { name: 'Chat name' });
    await expect(field).toHaveValue('Create a low-poly tree');
    await expect(dialog.getByRole('button', { name: 'Save' })).toBeDisabled();
    expect((await new AxeBuilder({ page }).include('[role="dialog"]').analyze()).violations).toEqual([]);
    await field.fill('Roblox tree');
    await field.press('Enter');
    await expect(dialog).toHaveCount(0);
    await expect(tasks.getByRole('button', { name: 'Roblox tree', exact: true })).toBeVisible();
    await page.reload();
    await newChatButton(page);
    await tasks.getByRole('button', { name: 'Rename chat Roblox tree' }).click();
    await page.getByRole('dialog', { name: 'Rename chat' }).getByRole('button', { name: 'Use original name' }).click();
    await expect(tasks.getByRole('button', { name: 'Rename chat Roblox tree' })).toHaveCount(0);
    await expect(tasks.getByRole('button', { name: /^Rename chat / })).toHaveCount(1);
  } finally { await app.close(); }
});
