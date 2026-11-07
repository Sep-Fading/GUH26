// Runs the finder through the shipped page.
import { test, expect } from '@playwright/test';
// Starts the real backend with isolated history.
import { launchApp } from '../support/harness.mjs';
// Gives results stable message and conversation identities.
import { thread, turn, message } from '../support/fixtures.mjs';

test('search finds a typo and jumps to its message', async ({ page, isMobile }) => {
  const app = await launchApp();
  try {
    await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null } });
    await app.set('thread/turns/list', { cases: [
      { params: { cursor: null }, handler: { result: { data: [turn('completed',
        Array.from({ length: 25 }, (_, i) => message(`Unrelated message ${i}.`, `message-${i}`)))], nextCursor: 'older' } } },
      { params: { cursor: 'older' }, handler: { result: { data: [{ ...turn('completed', [
        message('Authentication uses the local session.', 'target'),
      ]), id: 'older-turn' }], nextCursor: null } } },
    ] });
    await page.goto(app.url);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await expect(page.locator('header').getByRole('button', { name: 'Search', exact: true })).toHaveCount(0);
    if (isMobile) await page.getByRole('button', { name: 'Open navigation', exact: true }).click();
    const navigation = isMobile ? page.getByRole('dialog', { name: 'MiniClaw', exact: true })
      : page.getByRole('complementary', { name: 'Task navigation' });
    const search = navigation.getByRole('button', { name: 'Search', exact: true });
    await expect(search).toBeVisible();
    const newChatBox = await navigation.getByRole('button', { name: 'New chat', exact: true }).boundingBox();
    const searchBox = await search.boundingBox();
    expect(searchBox!.y).toBeGreaterThanOrEqual(newChatBox!.y + newChatBox!.height);
    await search.click();
    if (isMobile) await expect(navigation).toHaveCount(0);
    await page.getByRole('textbox', { name: 'Search all conversations' }).fill('authetication');
    await expect(page.getByRole('button', { name: /First task.*Authentication uses/ })).toBeVisible();
    await page.screenshot({ path: `/tmp/miniclaw-search-${test.info().project.name}.png` });
    await page.getByRole('textbox', { name: 'Search all conversations' }).press('Enter');
    const target = page.locator('[data-message-id="target"]');
    await expect(target).toBeInViewport();
    await expect(target).toHaveAttribute('data-search-match', 'true');
    await expect(page.getByRole('dialog', { name: 'Search conversations', exact: true })).toHaveCount(0);
    await page.keyboard.press('Control+k');
    await expect(page.getByRole('dialog', { name: 'Search conversations', exact: true })).toBeVisible();
    const refreshed = page.waitForResponse(response => response.url().endsWith('/api/search/refresh'));
    await page.getByRole('button', { name: 'Refresh search', exact: true }).click();
    expect((await refreshed).status()).toBe(200);
    await page.keyboard.press('Escape');
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  } finally { await app.close(); }
});
