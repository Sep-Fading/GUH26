// Exercises persisted sidebar sizing through browser input.
import { test, expect } from '@playwright/test';
// Starts an isolated backend without real conversations.
import { launchApp } from '../support/harness.mjs';

test('sidebar resizes, remembers its width and fits smaller windows', async ({ page, isMobile }) => {
  test.skip(isMobile, 'The persistent sidebar appears at desktop widths.');
  const app = await launchApp();
  try {
    await page.goto(app.url);
    const sidebar = page.getByRole('complementary', { name: 'Task navigation' });
    const handle = sidebar.getByRole('separator', { name: 'Resize sidebar' });
    await expect(handle).toBeVisible();
    const initial = (await sidebar.boundingBox())!.width;
    const edge = (await handle.boundingBox())!;
    await page.mouse.move(edge.x + edge.width / 2, edge.y + 160);
    await page.mouse.down();
    await page.mouse.move(edge.x + 140, edge.y + 160, { steps: 8 });
    await page.mouse.up();
    const wider = (await sidebar.boundingBox())!.width;
    expect(wider).toBeGreaterThan(initial + 100);
    await page.reload();
    await expect.poll(async () => (await sidebar.boundingBox())!.width).toBe(wider);
    await handle.focus();
    await page.keyboard.press('ArrowLeft');
    await expect.poll(async () => (await sidebar.boundingBox())!.width).toBe(wider - 16);
    await page.keyboard.press('End');
    expect((await sidebar.boundingBox())!.width).toBe(560);
    await page.setViewportSize({ width: 800, height: 900 });
    await expect.poll(async () => (await sidebar.boundingBox())!.width).toBeLessThanOrEqual(380);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(800);
    await page.setViewportSize({ width: 1440, height: 900 });
    await handle.dblclick();
    await expect.poll(async () => (await sidebar.boundingBox())!.width).toBe(initial);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(handle).toBeHidden();
    await page.getByRole('button', { name: 'Open navigation' }).click();
    await expect(page.getByRole('dialog', { name: 'MiniClaw', exact: true }).getByRole('button', { name: 'Search', exact: true })).toBeVisible();
  } finally { await app.close(); }
});

test('long navigation scrolls inside the sidebar', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { project } = await (await app.request('/api/projects', { method: 'POST', body: {
      name: 'Long project', environment: { kind: 'host' },
    } })).json();
    const threads = Array.from({ length: 80 }, (_, index) => ({
      id: crypto.randomUUID(), name: `Chat ${String(index + 1).padStart(2, '0')}`, model: null,
      ...(index < 40 ? { projectId: project.id, agentId: 'editor', agentName: 'Editor' } : {}),
      updatedAt: 80 - index,
    }));
    await page.route('**/api/threads', route => route.fulfill({ json: { threads, nextCursor: null } }));
    await page.goto(app.url);
    const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
    if (await toggle.isVisible()) await toggle.click();
    const nav = page.getByRole('navigation', { name: 'Tasks', exact: true });
    await expect(nav.getByRole('button', { name: 'Chat 80', exact: true })).toBeAttached();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollHeight - innerHeight)).toBeLessThanOrEqual(1);
    await expect.poll(() => nav.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(500);
    const settings = page.getByRole('button', { name: 'Settings', exact: true }).filter({ visible: true });
    await expect(settings).toBeInViewport();
    const before = await settings.boundingBox();
    await nav.hover();
    await page.mouse.wheel(0, 10000);
    await expect.poll(() => nav.evaluate(el => el.scrollTop)).toBeGreaterThan(500);
    await expect(nav.getByRole('button', { name: 'Chat 80', exact: true })).toBeInViewport();
    expect(await page.evaluate(() => window.scrollY)).toBe(0);
    expect((await settings.boundingBox())!.y).toBe(before!.y);
    await page.screenshot({ path: info.outputPath('long-sidebar.png') });
  } finally { await app.close(); }
});
