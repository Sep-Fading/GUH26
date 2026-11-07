// Verifies provider limits in conversations and the Analytics settings panel.
import { test, expect } from '@playwright/test';
import { fileURLToPath } from 'node:url';
import { AxeBuilder } from '@axe-core/playwright';
import { launchApp } from '../support/harness.mjs';
import { openSettings } from './settings.js';

test('Claude has its own footer and Analytics shows every provider limit', async ({ page }, info) => {
  const app = await launchApp({
    'thread/list': { result: { data: [], nextCursor: null } },
    'account/rateLimits/read': { result: { ordinaryUsageAllowed: false,
      rateLimits: { limitId: 'codex', primary: { usedPercent: 100, windowDurationMins: 300, resetsAt: 2000000000 } } } },
  }, { env: { CLAUDE_BIN: fileURLToPath(new URL('../support/fake-claude.mjs', import.meta.url)), MC_CLAUDE_LOG: '/dev/null' } });
  try {
    await page.goto(app.url);
    await page.getByRole('combobox', { name: 'Model', exact: true }).click();
    await page.getByRole('option', { name: 'Claude Sonnet 5', exact: true }).click();
    const footer = page.getByRole('region', { name: 'Claude usage', exact: true });
    await expect(footer).toContainText('10% used');
    await expect(footer).toContainText('51% used');
    await expect(footer).not.toContainText('100%');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hello');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('article', { name: 'Assistant message' })).toContainText('Hello from Claude');
    await page.screenshot({ path: info.outputPath('usage-footer.png') });
    const settings = await openSettings(page, 'Analytics');
    const limits = settings.getByRole('region', { name: 'Provider usage limits', exact: true });
    await expect(limits.getByRole('heading', { name: 'Codex', exact: true })).toBeVisible();
    await expect(limits.getByRole('heading', { name: 'Claude Code', exact: true })).toBeVisible();
    await expect(limits).toContainText('100% used');
    await expect(limits).toContainText('10% used');
    await expect(limits).toContainText('51% used');
    await expect(limits).toContainText('98% used');
    await expect(limits).toContainText('Sonnet');
    await expect(limits).toContainText('Resets');
    await page.screenshot({ path: info.outputPath('usage-limits.png') });
    const { violations } = await new AxeBuilder({ page }).include('[aria-label="Provider usage limits"]')
      .withTags(['wcag2a', 'wcag2aa']).analyze();
    expect(violations).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // A failed refresh leaves the previous figures and reset times visible.
    await page.route('**/api/usage?provider=claude*', route => route.fulfill({ json: {
      status: 'stale', provider: 'claude', ordinaryUsageAllowed: null, fetchedAt: new Date().toISOString(),
      buckets: [{ limitId: 'claude', primary: { usedPercent: 10, resetsAt: 2000000000, windowDurationMins: 300 }, secondary: null }],
    } }));
    await limits.getByRole('button', { name: 'Refresh usage limits' }).click();
    await expect(limits).toContainText('Out of date');
    await expect(limits).toContainText('10% used');
  } finally { await app.close(); }
});

test('provider limits remain visible with no recorded analytics', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    const settings = await openSettings(page, 'Analytics');
    await expect(settings.getByText(/Nothing was recorded in this range/)).toBeVisible();
    const limits = settings.getByRole('region', { name: 'Provider usage limits', exact: true });
    await expect(limits).toContainText('25% used');
    await expect(limits.getByRole('heading', { name: 'Claude Code', exact: true })).toHaveCount(0);
  } finally { await app.close(); }
});
