// Drives the global notice flow at desktop and mobile sizes.
import { test, expect } from '@playwright/test';
// Starts isolated backends without real account access.
import { launchApp, eventually } from '../support/harness.mjs';
// Emits protocol-shaped synthetic response failure events.
import { THREAD, turnDone } from '../support/fixtures.mjs';

test('notifications preserve read state and return to conversation', async ({ page }) => {
  const app = await launchApp();
  try {
    expect((await app.request(`/api/threads/${THREAD}/environment`, {
      method: 'POST', body: { kind: 'host' },
    })).status).toBe(200);
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(input).toBeEnabled();
    await input.fill('Synthetic failure test');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), state => state.requests.some(r => r.method === 'turn/start'));
    await app.emit(turnDone('failed'));
    await expect(input).toBeEnabled();
    await input.fill('Keep this unsent draft');
    // The unread badge sits inside its button, so nothing in the header can clip it.
    const bell = page.getByRole('button', { name: 'Notifications', exact: true });
    const [button, badge] = await Promise.all([bell.boundingBox(), bell.locator('span').boundingBox()]);
    expect(badge!.x).toBeGreaterThanOrEqual(button!.x);
    expect(badge!.y).toBeGreaterThanOrEqual(button!.y);
    expect(badge!.x + badge!.width).toBeLessThanOrEqual(button!.x + button!.width);
    await page.getByRole('button', { name: 'Notifications', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Notifications', exact: true });
    await expect(dialog.getByText('Codex could not complete this response. Open the conversation to review it.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Mark as read' }).click();
    await expect(dialog.getByText('Read', { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Open conversation' }).click();
    await expect(dialog).not.toBeVisible();
    await expect(input).toHaveValue('Keep this unsent draft');
    await page.reload();
    await expect(input).toBeEnabled();
    await page.getByRole('button', { name: 'Notifications', exact: true }).click();
    await expect(dialog.getByText('Read', { exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog).not.toBeVisible();
    await expect(page.getByRole('button', { name: 'Notifications', exact: true })).toBeFocused();
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(1);
  } finally { await app.close(); }
});

test('mark all and clear read persist after reload', async ({ page }) => {
  const app = await launchApp();
  try {
    expect((await app.request(`/api/threads/${THREAD}/environment`, { method: 'POST', body: { kind: 'host' } })).status).toBe(200);
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(input).toBeEnabled();
    await input.fill('Synthetic failure test');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), state => state.requests.some(r => r.method === 'turn/start'));
    await app.emit(turnDone('failed'));
    await expect(input).toBeEnabled();
    const bell = page.getByRole('button', { name: 'Notifications', exact: true });
    // The bell polls every ten seconds.
    await expect(bell.locator('span')).toHaveText('1', { timeout: 15000 });
    await bell.click();
    const dialog = page.getByRole('dialog', { name: 'Notifications', exact: true });
    await expect(dialog.getByRole('button', { name: 'Clear read' })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Mark all as read' }).click();
    await expect(dialog.getByText('0 unread.', { exact: false })).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Mark all as read' })).toBeDisabled();
    await expect(dialog.getByRole('button', { name: 'Mark as read' })).toHaveCount(0);
    expect((await (await app.request('/api/notifications')).json()).notifications.every((n: { read: boolean }) => n.read)).toBe(true);
    await dialog.getByRole('button', { name: 'Clear read' }).click();
    await expect(dialog.getByText('No saved notifications.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Clear read' })).toBeDisabled();
    expect((await (await app.request('/api/notifications')).json()).notifications).toEqual([]);
    await page.reload();
    await expect(input).toBeEnabled();
    await bell.click();
    await expect(dialog.getByText('No saved notifications.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Clear read' })).toBeDisabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally { await app.close(); }
});
