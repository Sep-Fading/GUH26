// Checks the Settings sheet in a real browser.
import { test, expect } from '@playwright/test';
// Starts a disposable backend.
import { launchApp, json } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings, newChatButton } from './settings.js';

test('one gear opens Settings, and the header keeps only what belongs to the chat', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    for (const gone of ['Manage agents', 'Manage skills', 'Manage providers']) {
      await expect(page.getByRole('button', { name: gone })).toHaveCount(0);
    }
    const dialog = await openSettings(page, 'Appearance');
    await expect(dialog.getByRole('tab')).toHaveText(['Appearance', 'Providers', 'Agents', 'Skills', 'MCP servers', 'Schedules', 'Analytics', 'Virtualisation']);
    await expect(dialog.getByRole('tab', { name: 'Appearance' })).toHaveAttribute('aria-selected', 'true');
    for (const category of ['Providers', 'Agents', 'Skills', 'MCP servers', 'Schedules', 'Virtualisation'] as const) {
      await openSettings(page, category);
      await expect(dialog.getByRole('heading', { name: category, exact: true })).toBeVisible();
    }
    // The sheet's close button sits on the centre line of every panel's header, beside its own buttons.
    // On a phone the categories sit in a row above the panel, so there is no shared header line.
    for (const [category, button] of (page.viewportSize()!.width >= 640 ? [['MCP servers', 'Reload servers'], ['Schedules', 'Reload schedules']] as const : [])) {
      await openSettings(page, category);
      const close = (await dialog.getByRole('button', { name: 'Close', exact: true }).boundingBox())!;
      const reload = (await dialog.getByRole('button', { name: button, exact: true }).boundingBox())!;
      expect(Math.abs((close.y + close.height / 2) - (reload.y + reload.height / 2))).toBeLessThanOrEqual(1);
      expect(close.x).toBeGreaterThanOrEqual(reload.x + reload.width);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('settings.png') });
  } finally { await app.close(); }
});

test('appearance changes apply at once, are saved on the Mac and survive a reload', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.emulateMedia({ colorScheme: 'light' });
    await page.goto(app.url);
    const root = page.locator('html');
    const dialog = await openSettings(page, 'Appearance');
    await dialog.getByRole('radio', { name: 'Dark', exact: true }).click();
    await expect(root).toHaveClass(/dark/);
    await dialog.getByRole('radio', { name: 'Green', exact: true }).click();
    await expect(root).toHaveAttribute('data-accent', 'green');
    await dialog.getByRole('radio', { name: 'Large', exact: true }).click();
    await expect(root).toHaveCSS('font-size', '17.5px');
    await dialog.getByRole('switch', { name: 'Compact layout' }).click();
    await expect(root).toHaveAttribute('data-compact', '');
    await expect.poll(async () => json(await app.request('/api/settings/appearance')))
      .toEqual({ theme: 'dark', textSize: 'large', accent: 'green', compact: true });
    // A reload starts from the saved choices, and a browser without a local copy gets them too.
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(root).toHaveClass(/dark/);
    await expect(root).toHaveAttribute('data-accent', 'green');
    // "Match macOS" follows the system again.
    await (await openSettings(page, 'Appearance')).getByRole('radio', { name: 'Match macOS' }).click();
    await expect(root).not.toHaveClass(/dark/);
  } finally { await app.close(); }
});

test('Virtualisation shows the budget, saves limits, repairs the ledger and flags an old VM worker',
  async ({ page }) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    const GiB = 1024 ** 3;
    const view = { available: true, workerCurrent: false,
      master: { state: 'running', ready: true, signedIn: true, computerReady: true },
      ledger: { vms: [{ id: '/Users/me/MiniClaw/master/Guest', state: 'stopped', cpuCount: 2, memoryBytes: 8 * GiB },
        { id: '/Users/me/old/Guest', state: 'unknown', cpuCount: 2, memoryBytes: 8 * GiB }],
      limits: { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 },
      capacity: { cpuCount: 5, memoryBytes: 12 * GiB, diskBytes: 231 * GiB },
      used: { cpuCount: 2, memoryBytes: 8 * GiB, diskBytes: 64 * GiB }, exceeded: [] as string[] } };
    const posted: Record<string, unknown> = {};
    try {
      await page.route('**/api/virtualisation', route => route.fulfill({ json: view }));
      await page.route('**/api/virtualisation/*', route => {
        const action = route.request().url().split('/').pop()!;
        posted[action] = route.request().postDataJSON();
        const ledger = { ...view.ledger, released: ['/Users/me/old/Guest'], refused: [], forgotten: [] as string[] };
        if (action === 'limits') ledger.limits = posted.limits as typeof view.ledger.limits;
        return route.fulfill({ json: { ...view, ledger } });
      });
      await page.goto(app.url);
      const dialog = await openSettings(page, 'Virtualisation');
      await expect(dialog.getByText("This VM's worker is older than this MiniClaw.")).toBeVisible();
      await expect(dialog.getByText('miniclaw update-vm')).toBeVisible();
      await expect(dialog.getByText('8.0 GiB of 12.0 GiB')).toBeVisible();
      await expect(dialog.getByRole('row')).toHaveCount(3);
      await dialog.getByRole('spinbutton', { name: 'CPU percent' }).fill('40');
      await dialog.getByRole('button', { name: 'Save limits' }).click();
      await expect(dialog.getByText('Limits saved.')).toBeVisible();
      expect(posted.limits).toEqual({ cpuPercent: 40, memoryPercent: 50, diskPercent: 25 });
      await dialog.getByRole('button', { name: 'Release idle VMs (1)' }).click();
      await expect(dialog.getByText('Released 1.')).toBeVisible();
      await dialog.getByRole('button', { name: 'Forget deleted VMs' }).click();
      await expect(dialog.getByText('No stopped VM has been deleted from disk.')).toBeVisible();
    } finally { await app.close(); }
  });

test('an installation without a VM says so instead of showing empty controls', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Virtualisation');
    await expect(dialog.getByText('This installation has no VM.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Save limits' })).toHaveCount(0);
  } finally { await app.close(); }
});

test('Settings opens from the foot of the sidebar, which no longer shows the account email', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    if (info.project.name === 'desktop') {
      const sidebar = page.getByRole('complementary', { name: 'Task navigation' });
      const row = sidebar.getByRole('button', { name: 'Settings', exact: true });
      const [rowBox, sideBox] = [await row.boundingBox(), await sidebar.boundingBox()];
      expect(rowBox!.y + rowBox!.height).toBeGreaterThan(sideBox!.y + sideBox!.height - 24);
      await expect(page.getByRole('banner').getByRole('button', { name: 'Settings', exact: true })).toHaveCount(0);
    }
    await expect(page.getByText('reader@example.test', { exact: true })).toHaveCount(0);
    const dialog = await openSettings(page, 'Appearance');
    await expect(dialog).toBeVisible();
    // On a phone the drawer closes behind the sheet, so the page is usable afterwards.
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  } finally { await app.close(); }
});

test('the white accent keeps everything legible: dark text on white, never white on white', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Appearance');
    await dialog.getByRole('radio', { name: 'White' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-accent', 'white');
    await dialog.getByRole('radio', { name: 'Light', exact: true }).click();
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toBeHidden();
    await (await newChatButton(page)).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hello');
    const send = page.getByRole('button', { name: 'Send', exact: true });
    await expect(send).toBeEnabled();
    const look = (locator: import('@playwright/test').Locator) => locator.evaluate(el => {
      const style = getComputedStyle(el);
      return { text: style.color, background: style.backgroundColor, outline: style.boxShadow };
    });
    // The button fades from its disabled grey, so its colour is read once it has settled.
    await expect.poll(async () => (await look(send)).background).toBe('rgb(255, 255, 255)');
    const button = await look(send);
    expect(button.text).toBe('rgb(29, 29, 31)');
    expect(button.outline).not.toBe('none');
    // The accent used as text follows the page's text colour.
    const mark = await look(page.locator('.text-brand').first());
    expect(mark.text).not.toBe('rgb(255, 255, 255)');
    await page.screenshot({ path: info.outputPath('white-accent.png') });
    expect((await (await app.request('/api/settings/appearance')).json()).accent).toBe('white');
  } finally { await app.close(); }
});

test('the new conversation button is in the sidebar at every width, never in the top bar', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    const narrow = page.viewportSize()!.width < 768;
    const header = page.getByRole('banner');
    await expect(header.getByRole('button', { name: 'New chat', exact: true })).toHaveCount(0);
    if (narrow) {
      await expect(page.getByRole('button', { name: 'New chat', exact: true }).filter({ visible: true })).toHaveCount(0);
      await page.getByRole('button', { name: 'Open navigation', exact: true }).click();
    }
    const sidebar = narrow ? page.getByRole('dialog', { name: 'MiniClaw', exact: true }) : page.getByRole('complementary', { name: 'Task navigation' });
    await sidebar.getByRole('button', { name: 'New chat', exact: true }).click();
    // Pressing it on a narrow window closes the drawer, so the new conversation is in view.
    if (narrow) await expect(page.getByRole('dialog', { name: 'MiniClaw', exact: true })).toHaveCount(0);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  } finally { await app.close(); }
});

