// Checks provider settings in a real browser.
import { test as base, expect } from '@playwright/test';
// Resolves the fake Claude CLI path.
import { fileURLToPath } from 'node:url';
// Updates provider catalogs while the application stays running.
import { writeFile } from 'node:fs/promises';
// Starts isolated application, Codex and Claude fixtures.
import { launchApp } from '../support/harness.mjs';
// Supplies the signed-out ChatGPT account.
import { signedOut } from '../support/fixtures.mjs';
import { models } from '../support/fixtures.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

const claude = fileURLToPath(new URL('../support/fake-claude.mjs', import.meta.url));
type App = Awaited<ReturnType<typeof launchApp>>;
const test = base.extend<{ start: (handlers?: object, env?: object) => Promise<App> }>({
  // Launches apps with the fake Claude CLI and disposes them after the test.
  start: async ({}, use) => {
    const apps: App[] = [];
    await use(async (handlers = {}, env = {}) => {
      const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } },
        ...handlers }, { env: { CLAUDE_BIN: claude, MC_CLAUDE_LOG: '/dev/null', ...env } });
      apps.push(app);
      return app;
    });
    for (const app of apps) await app.close();
  },
});

test('Claude alone makes the app ready and offers only Claude models', async ({ page, start }) => {
  const app = await start({ 'account/read': { result: signedOut } });
  await page.goto(app.url);
  await expect(page.getByText('Ready when you are')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toHaveCount(0);
  await page.getByRole('combobox', { name: 'Model', exact: true }).click();
  await expect(page.getByRole('option', { name: 'Claude Sonnet 5' })).toBeVisible();
  await expect(page.getByRole('option', { name: 'Model A' })).toHaveCount(0);
});

test('both signed out: each provider offers its own sign-in', async ({ page, start }) => {
  const app = await start({ 'account/read': { result: signedOut } }, { MC_CLAUDE_SIGNED_OUT: '1' });
  await page.goto(app.url);
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in with Claude', exact: true })).toBeVisible();
});

test('the Providers panel disables Claude and its models disappear', async ({ page, start }) => {
  const app = await start();
  await page.goto(app.url);
  await openSettings(page, 'Providers');
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText('reader@example.test · plus')).toBeVisible();
  await expect(dialog.getByText('claude-reader@example.test · max')).toBeVisible();
  await dialog.getByRole('switch', { name: 'Enable Claude Code' }).click();
  await expect(dialog.getByText('Disabled')).toBeVisible();
  await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  // The sheet animates out. A click made while it is still closing lands on its overlay,
  // which is what failed now and then under load.
  await expect(dialog).toBeHidden();
  await page.getByRole('combobox', { name: 'Model', exact: true }).click();
  await expect(page.getByRole('option', { name: 'Model A' })).toBeVisible();
  await expect(page.getByRole('option', { name: 'Claude Sonnet 5' })).toHaveCount(0);
});

test('the existing provider refresh discovers models from both providers', async ({ page, start }, info) => {
  const file = info.outputPath('claude-models.json');
  const oldModel = { value: 'claude-original', displayName: 'Claude Original', description: '' };
  await writeFile(file, JSON.stringify({ models: [oldModel] }));
  const app = await start({}, { MC_CLAUDE_MODELS_FILE: file });
  await page.goto(app.url);
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  await expect(model).toContainText('Model A');
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('Keep my draft');
  await openSettings(page, 'Providers');
  const dialog = page.getByRole('dialog');
  // Finish the panel's initial refresh before changing the upstream catalogs.
  const refresh = dialog.getByRole('button', { name: 'Refresh providers', exact: true });
  await expect(refresh).toBeEnabled();
  await writeFile(file, JSON.stringify({ models: [oldModel,
    { value: 'claude-new-model', displayName: 'Claude New model', description: 'New release' }] }));
  await app.set('model/list', { result: { data: [...models,
    { ...models[0], model: 'new-codex-model', displayName: 'Codex New model', isDefault: false }], nextCursor: null } });
  await refresh.click();
  await expect(dialog.getByText('2 models available', { exact: true })).toBeVisible();
  await expect(dialog.getByText('3 models available', { exact: true })).toBeVisible();
  await expect(refresh).toBeEnabled();
  await page.screenshot({ path: info.outputPath('provider-model-refresh.png') });
  await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(composer).toHaveValue('Keep my draft');
  await expect(model).toContainText('Model A');
  await model.click();
  await expect(page.getByRole('option', { name: 'Claude New model', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: 'Codex New model', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  await openSettings(page, 'Providers');
  await expect(refresh).toBeEnabled();
  await writeFile(file, JSON.stringify({ error: 'private failure' }));
  await refresh.click();
  await expect(dialog.getByRole('alert')).toContainText('Models unavailable');
  await expect(dialog.getByText('2 models available', { exact: true })).toBeVisible();
});
