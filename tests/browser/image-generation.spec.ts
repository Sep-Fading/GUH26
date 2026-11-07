// Checks the signed-in Codex image control and its returned picture in the chat.
import { test, expect } from '@playwright/test';
import { launchApp, eventually, json } from '../support/harness.mjs';
import { THREAD, TURN, turnDone } from '../support/fixtures.mjs';
import { openSettings, openTools } from './settings.js';

const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScL/nwAAAABJRU5ErkJggg==';

test('main agent can allow and default generated images', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Main agent', exact: true }).click();
    const allowance = dialog.getByRole('group', { name: 'Capabilities' })
      .getByRole('switch', { name: 'Image generation', exact: true });
    await allowance.click();
    await expect(allowance).toBeChecked();
    await expect.poll(async () => (await json(await app.request('/api/agents/00000000-0000-0000-0000-000000000000/capability-defaults')))
      .capabilities.find((capability: { key: string }) => capability.key === 'image_generation')?.allowed).toBe(true);
    await dialog.getByRole('tab', { name: 'Enabled by default' }).click();
    const initial = dialog.getByRole('tabpanel', { name: 'Enabled by default' })
      .getByRole('switch', { name: 'Image generation', exact: true });
    await initial.click();
    await expect(initial).toBeChecked();
    await page.keyboard.press('Escape');
    const tools = await openTools(page);
    await expect(tools.getByRole('switch', { name: 'Image generation', exact: true })).toBeChecked();
    await page.getByRole('button', { name: 'Tools', exact: true }).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Draw a red dot');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const started = await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/start'), 'turn');
    expect(started.requests.find(request => request.method === 'thread/start')?.params.config['features.image_generation']).toBe(true);
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN,
      item: { type: 'imageGeneration', id: 'generated-1', status: 'completed', result: PNG } } }, turnDone());
    const generated = page.getByRole('list', { name: 'Generated images' });
    await expect(generated.getByRole('img', { name: 'generated-image.png' })).toBeVisible();
    await expect.poll(() => generated.getByRole('img').evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
  } finally { await app.close(); }
});
