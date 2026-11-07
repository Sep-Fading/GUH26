// Automated accessibility checks (axe) on the screens people use most, in both themes.
import { test, expect, type Page } from '@playwright/test';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';
import { openSettings, newChatButton } from './settings.js';

// Reports what a person would be blocked by. Minor advice is left to manual review.
async function blockers(page: Page, where: string) {
  // A colour read halfway through a fade, or under the pointer, is not what people see at rest.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.mouse.move(0, 0);
  const { violations } = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  return violations.filter(item => ['serious', 'critical'].includes(item.impact ?? ''))
    .map(item => `${where}: ${item.id} (${item.nodes.length}) ${item.nodes[0].target.join(' ')} :: ${item.nodes[0].failureSummary?.split('\n')[1]?.trim()}`);
}

for (const theme of ['light', 'dark'] as const) {
  test(`no serious accessibility problem on the main screens (${theme})`, async ({ page }) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    try {
      await app.request('/api/settings/appearance', { method: 'POST',
        body: { theme, textSize: 'default', accent: 'blue', compact: false } });
      await page.goto(app.url);
      await (await newChatButton(page)).click();
      const found = await blockers(page, 'chat');
      for (const category of ['Appearance', 'Providers', 'Agents', 'Skills', 'MCP servers', 'Schedules', 'Virtualisation'] as const) {
        await openSettings(page, category);
        found.push(...await blockers(page, `settings/${category}`));
      }
      await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
      await page.getByRole('button', { name: 'Notifications', exact: true }).click();
      found.push(...await blockers(page, 'notifications'));
      expect(found).toEqual([]);
    } finally { await app.close(); }
  });
}

test('the white accent passes the same contrast checks', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    for (const theme of ['light', 'dark']) {
      await app.request('/api/settings/appearance', { method: 'POST', body: { theme, textSize: 'default', accent: 'white', compact: false } });
      await page.goto(app.url);
      await (await newChatButton(page)).click();
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hello');
      expect(await blockers(page, `white/${theme}`)).toEqual([]);
    }
  } finally { await app.close(); }
});

test('reduced motion switches the fades and slides off', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(app.url);
    const seconds = await (await newChatButton(page))
      .evaluate(el => parseFloat(getComputedStyle(el).transitionDuration));
    expect(seconds).toBeLessThan(0.001);
  } finally { await app.close(); }
});

test('at 200% zoom nothing scrolls sideways and the main controls stay reachable by keyboard', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop', 'zoom is a desktop concern');
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    // A 1440 px window at 200% zoom lays out like a 720 px one.
    await page.setViewportSize({ width: 720, height: 450 });
    await page.goto(app.url);
    const fits = () => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1);
    expect(await fits()).toBe(true);
    await openSettings(page, 'Virtualisation');
    expect(await fits()).toBe(true);
    await page.keyboard.press('Escape');
    // Keyboard only: reach the composer, type, and find Send focusable.
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.focus();
    await page.keyboard.type('Hello');
    const names: string[] = [];
    for (let step = 0; step < 12; step++) {
      await page.keyboard.press('Tab');
      names.push(await page.evaluate(() => document.activeElement?.getAttribute('aria-label') ?? document.activeElement?.textContent?.trim().slice(0, 20) ?? ''));
    }
    expect(names).toContain('Send');
    // Focus is always visible: the focused control draws an outline or a ring.
    const ring = await page.evaluate(() => { const style = getComputedStyle(document.activeElement!);
      return style.outlineStyle !== 'none' || style.boxShadow !== 'none'; });
    expect(ring).toBe(true);
  } finally { await app.close(); }
});
