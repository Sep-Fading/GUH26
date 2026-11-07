// Verifies live formatting through ordinary keyboard input.
import { test, expect } from '@playwright/test';
// Starts isolated editors without real model calls.
import { launchApp } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

test('one editor formats typing, preserves cursor, and saves Markdown', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    await openSettings(page, 'Skills');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(dialog.getByRole('tabpanel').getByRole('tab')).toHaveCount(0);
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Live formatting');
    const editor = dialog.getByRole('textbox', { name: 'Instructions', exact: true });
    await editor.click();
    await editor.pressSequentially('*test*');
    await expect(editor.locator('em')).toHaveText('test');
    await expect(editor).toHaveText('test');
    await editor.pressSequentially(' after');
    await expect(editor).toHaveText('test after');
    await editor.press('ArrowLeft');
    await editor.press('Backspace');
    await expect(editor).toHaveText('test aftr');
    await editor.press('ControlOrMeta+z');
    await expect(editor).toHaveText('test after');
    await editor.press('ControlOrMeta+Shift+z');
    await expect(editor).toHaveText('test aftr');
    await dialog.getByRole('button', { name: 'Save skill', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect((await (await app.request('/api/skills')).json()).skills[0].instructions).toBe('*test* aftr');
    await page.reload();
    await openSettings(page, 'Skills');
    await dialog.getByRole('button', { name: 'Live formatting', exact: true }).click();
    await expect(editor.locator('em')).toHaveText('test');
    await expect(editor).toHaveText('test aftr');
    await page.screenshot({ path: info.outputPath('live-markdown.png') });
    await page.setViewportSize({ width: 320, height: 640 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.screenshot({ path: info.outputPath('live-markdown-narrow.png') });
  } finally { await app.close(); }
});

test('literal code, escaped markers, and hostile text survive editing', async ({ page }) => {
  const app = await launchApp();
  try {
    const source = '---\nname: exact\n---\n\n# Rules\n\n' +
      '`* test *` and \\*literal\\*\n\n```html\n<img src=x onerror=alert(1)>\n```\n\n' +
      '![<script>alert(1)</script>](https://invalid.example/pixel)\n\n[unsafe](javascript:alert(1))';
    const response = await app.request('/api/skills', { method: 'POST', body: {
      name: 'Exact source', instructions: source,
    } });
    expect(response.status).toBe(201);
    let remote = 0;
    page.on('request', request => { if (request.url().startsWith('https://invalid.example')) remote++; });
    await page.goto(app.url);
    await openSettings(page, 'Skills');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await dialog.getByRole('button', { name: 'Exact source', exact: true }).click();
    const editor = dialog.getByRole('textbox', { name: 'Instructions', exact: true });
    await expect(editor.locator('code').first()).toHaveText('* test *');
    await expect(editor.locator('em,img[src],img[srcset],script,iframe,[onerror],a')).toHaveCount(0);
    await editor.focus();
    await editor.press('ControlOrMeta+End');
    await editor.pressSequentially(' end');
    await dialog.getByRole('button', { name: 'Save skill', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect((await (await app.request('/api/skills')).json()).skills[0].instructions).toBe(source + ' end');
    expect(remote).toBe(0);
  } finally { await app.close(); }
});

test('spaced italic shortcut and bold formatting work in agent instructions', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    await openSettings(page, 'Agents');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    const editor = dialog.getByRole('textbox', { name: 'Instructions', exact: true });
    await editor.click();
    await editor.pressSequentially('* test * and **bold**');
    await expect(editor.locator('em')).toHaveText('test');
    await expect(editor.locator('strong')).toHaveText('bold');
    await expect(editor).toHaveText('test and bold');
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Editor');
    await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect((await (await app.request('/api/agents')).json()).agents[0].instructions)
      .toBe('*test* and **bold**');
  } finally { await app.close(); }
});

test('pending saves lock editing and failure keeps source and undo', async ({ page }) => {
  const app = await launchApp();
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  try {
    await page.route('**/api/skills', async route => {
      if (route.request().method() !== 'POST') return route.continue();
      await held;
      await route.fulfill({ status: 503, contentType: 'application/json',
        body: JSON.stringify({ error: { message: 'Storage unavailable' } }) });
    });
    await page.goto(app.url);
    await openSettings(page, 'Skills');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    const editor = dialog.getByRole('textbox', { name: 'Instructions', exact: true });
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Draft');
    await editor.fill('*keep*');
    await editor.press('ControlOrMeta+End');
    await editor.pressSequentially(' draft');
    await dialog.getByRole('button', { name: 'Save skill', exact: true }).click();
    await expect(editor).toHaveAttribute('aria-readonly', 'true');
    await expect(editor).toHaveAttribute('contenteditable', 'false');
    release();
    await expect(dialog.getByRole('alert')).toHaveText('Storage unavailable');
    await expect(editor).toHaveAttribute('contenteditable', 'true');
    await expect(editor).toHaveText('keep draft');
    await editor.focus();
    await editor.press('ControlOrMeta+z');
    await expect(editor).not.toHaveText('keep draft');
  } finally { release(); await app.close(); }
});
