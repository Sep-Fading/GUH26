// Checks the Analytics tab in a real browser: the figures it draws, the range control, the empty
// state, and that resizing the window leaves no chart wider than its column and no page scrollbar.
import { test, expect, type Page } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Opens the Settings sheet.
import { openSettings } from './settings.js';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

type App = Awaited<ReturnType<typeof launchApp>>;

/** Answers the analytics route with a fixed report, so the tab draws the same picture every run. */
async function withReport(page: Page, report: unknown) {
  await page.route(/\/api\/analytics(\?|$)/, route => route.fulfill({ json: report }));
}

/** Builds a report of `days` days with a busy last day. */
function report(days = 30) {
  const daily = Array.from({ length: days }, (_, index) => {
    const date = new Date(Date.now() - (days - 1 - index) * 86400000).toISOString().slice(0, 10);
    const busy = index > days - 6;
    return { date, turns: busy ? 4 : 0, tokens: busy ? 42_000 : 0, inputTokens: busy ? 38_000 : 0,
      outputTokens: busy ? 4000 : 0, cachedTokens: busy ? 12_000 : 0, steps: busy ? 9 : 0 };
  });
  return { days, from: 0,
    totals: { turns: 20, tokens: 210_000, inputTokens: 190_000, outputTokens: 20_000, cachedTokens: 60_000,
      reasoningTokens: 5000, seconds: 742, steps: 45, conversations: 6 },
    daily,
    agents: [{ name: 'Builder', turns: 14, tokens: 160_000 }, { name: 'No agent', turns: 6, tokens: 50_000 }],
    projects: [{ name: 'Game', turns: 14, tokens: 160_000 }],
    providers: [{ name: 'codex', turns: 15, tokens: 180_000 }, { name: 'claude', turns: 5, tokens: 30_000 }],
    models: [{ name: 'gpt-5.6-luna', provider: 'codex', turns: 15, tokens: 180_000 },
      { name: 'claude-haiku-4-5-20251001', provider: 'claude', turns: 5, tokens: 30_000 }],
    efforts: [{ name: 'low', turns: 20, tokens: 210_000 }],
    steps: [{ kind: 'command', calls: 20, ok: 18, failed: 1, refused: 1, averageMs: 1400 },
      { kind: 'mcp', calls: 15, ok: 14, failed: 1, refused: 0, averageMs: 320 },
      { kind: 'read', calls: 10, ok: 10, failed: 0, refused: 0, averageMs: 40 }],
    mcpServers: [{ name: 'blender', calls: 12, failures: 1, tokens: 90_000, turns: 8 },
      { name: 'notes', calls: 3, failures: 0, tokens: 12_000, turns: 3 }],
    mcpTools: [{ name: 'blender: get_scene_info', calls: 7, failures: 0 },
      { name: 'blender: execute_code', calls: 5, failures: 1 }],
    skills: [{ name: 'Universal', source: 'agent', invocations: 14, tokens: 160_000 },
      { name: 'Release Notes', source: 'slash', invocations: 3, tokens: 21_000 }] };
}

/** Opens Settings at the Analytics tab. */
async function analytics(page: Page, app: App) {
  await page.goto(app.url);
  const settings = await openSettings(page, 'Analytics');
  const panel = settings.getByRole('region', { name: 'Analytics' });
  await expect(panel.getByRole('heading', { name: 'Analytics', exact: true })).toBeVisible();
  return panel;
}

test('the Analytics tab shows what was used, by agent, model, skill and MCP server', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await withReport(page, report());
    const panel = await analytics(page, app);
    await expect(panel).toContainText('210,000');
    await expect(panel).toContainText('20 responses across 6 conversations');
    // Every ranked list is drawn, with its own rows.
    for (const heading of ['Agents', 'Models', 'Skills', 'MCP servers', 'MCP tools', 'Projects']) {
      await expect(panel.getByRole('heading', { name: heading, exact: true })).toBeVisible();
    }
    await expect(panel).toContainText('Builder');
    await expect(panel).toContainText('blender: get_scene_info');
    await expect(panel).toContainText('Release Notes');
    // A skill's source is said in words, and every ranked list names its unit once, in its heading.
    await expect(panel).toContainText('called with /');
    await expect(panel.getByRole('region', { name: 'Skills' })).toContainText('responses');
    await expect(panel.getByRole('region', { name: 'MCP servers' })).toContainText('90k tokens, 1 failed');
    // The table of tool kinds carries its counts.
    const row = panel.getByRole('row', { name: /command/ });
    await expect(row).toContainText('20');
    await expect(row).toContainText('1.4 s');
    // Charts really draw: each one has an SVG with marks in it.
    await expect.poll(() => panel.locator('[data-slot="chart"] svg').count()).toBeGreaterThanOrEqual(3);
    await page.screenshot({ path: info.outputPath('analytics.png'), fullPage: false });
    const { violations } = await new AxeBuilder({ page }).include('[aria-label="Analytics"]')
      .withTags(['wcag2a', 'wcag2aa']).analyze();
    expect(violations.filter(item => ['serious', 'critical'].includes(item.impact ?? ''))).toEqual([]);
  } finally { await app.close(); }
});

test('the range control asks the backend for that many days', async ({ page }) => {
  const app = await launchApp();
  try {
    const asked: string[] = [];
    await page.route(/\/api\/analytics(\?|$)/, route => {
      const days = new URL(route.request().url()).searchParams.get('days') ?? '';
      asked.push(days);
      return route.fulfill({ json: report(Number(days) || 30) });
    });
    const panel = await analytics(page, app);
    await expect.poll(() => asked).toEqual(['30']);
    await panel.getByRole('combobox', { name: 'Range' }).click();
    await page.getByRole('option', { name: 'Last 7 days' }).click();
    await expect.poll(() => asked).toEqual(['30', '7']);
    await expect(panel).toContainText('Tokens in the last 7 days');
  } finally { await app.close(); }
});

test('an empty range says so instead of drawing empty charts', async ({ page }) => {
  const app = await launchApp();
  try {
    const empty = report(30);
    empty.totals = { turns: 0, tokens: 0, inputTokens: 0, outputTokens: 0, cachedTokens: 0, reasoningTokens: 0, seconds: 0, steps: 0, conversations: 0 };
    await withReport(page, empty);
    const panel = await analytics(page, app);
    await expect(panel).toContainText('Nothing was recorded in this range');
    await expect(panel.locator('[data-slot="chart"]')).toHaveCount(0);
  } finally { await app.close(); }
});

test('resizing the window leaves no chart wider than its column and no sideways scrollbar', async ({ page }) => {
  const app = await launchApp();
  try {
    await withReport(page, report());
    const panel = await analytics(page, app);
    await expect.poll(() => panel.locator('[data-slot="chart"] svg').count()).toBeGreaterThanOrEqual(3);
    // The sheet is wide, then narrow, then wide again: a chart that only grows would be caught here.
    for (const [width, height] of [[1440, 900], [1100, 800], [760, 700], [390, 844], [900, 800], [1440, 900]] as const) {
      await page.setViewportSize({ width, height });
      // One frame to lay out, one for the charts' own resize observers.
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await expect.poll(async () => page.evaluate(() =>
        document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      const overflowing = await panel.evaluate(element => [...element.querySelectorAll('[data-slot="chart"]')]
        .filter(chart => chart.scrollWidth > Math.ceil(chart.getBoundingClientRect().width) + 1).length);
      expect(overflowing, `charts wider than their column at ${width}px`).toBe(0);
      // Nothing inside the panel may push the panel itself sideways.
      expect(await panel.evaluate(element => element.scrollWidth - Math.ceil(element.clientWidth)),
        `panel scrolls sideways at ${width}px`).toBeLessThanOrEqual(1);
    }
  } finally { await app.close(); }
});
