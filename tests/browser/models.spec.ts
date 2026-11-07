import { test as base, expect } from '@playwright/test';
import { launchApp, eventually } from '../support/harness.mjs';
import { itemDone, turnDone, models } from '../support/fixtures.mjs';
// Finds the New chat control at either width.
import { newChatButton } from './settings.js';

const test = base.extend<{ app: Awaited<ReturnType<typeof launchApp>> }>({
  app: async ({}, use) => {
    const app = await launchApp({
      'thread/list': { result: { data: [], nextCursor: null } },
    });
    try { await use(app); } finally { await app.close(); }
  },
});

test('models determine effort choices and the next send', async ({ page, app }) => {
  await page.goto(app.url);
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  const effort = page.getByRole('combobox', { name: 'Reasoning effort', exact: true });
  await expect(model).toContainText('Model A');
  await model.click();
  await page.getByRole('option', { name: 'Model B', exact: true }).click();
  await expect(effort).toContainText('none');
  await effort.click();
  await expect(page.getByRole('option', { name: 'high', exact: true })).toHaveCount(0);
  await page.getByRole('option', { name: 'ultra', exact: true }).click();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hello');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  const state = await eventually(() => app.inspect(), s =>
    s.requests.some(r => r.method === 'turn/start'));
  const request = state.requests.find(r => r.method === 'turn/start');
  expect(request.params.model).toBe('other-model');
  expect(request.params.effort).toBe('ultra');
  await expect(model).toBeDisabled();
  await expect(effort).toBeDisabled();
  await app.emit(itemDone('Done'), turnDone());
  await expect(model).toBeEnabled();
  await page.reload();
  await expect(model).toContainText('Model B');
  await expect(effort).toContainText('ultra');
});

test('the speed choice offers the tiers the model advertises, in Codex\'s words, and the footer names the one in use', async ({ page, app }) => {
  await page.goto(app.url);
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  const speed = page.getByRole('combobox', { name: 'Speed', exact: true });
  await expect(model).toContainText('Model A');
  await expect(speed).toContainText('Standard');
  await speed.click();
  await page.getByRole('option', { name: 'Fast: 1.5x speed, increased usage', exact: true }).click();
  await expect(speed).toContainText('Fast');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hello');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  const state = await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/start'));
  expect(state.requests.find(r => r.method === 'turn/start').params.serviceTier).toBe('priority');
  await expect(page.getByRole('status').filter({ hasText: 'Fast' })).toBeVisible();
  await app.emit(itemDone('Done'), turnDone());
  await expect(model).toBeEnabled();
  await page.reload();
  await expect(speed).toContainText('Fast');
  // A model with no tiers has no speed to choose.
  await (await newChatButton(page)).click();
  await model.click();
  await page.getByRole('option', { name: 'Model B', exact: true }).click();
  await expect(speed).toBeDisabled();
  await expect(speed).toContainText('Standard');
});

test('model menu works with keyboard and fits viewport', async ({ page, app }) => {
  await page.goto(app.url);
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  await expect(model).toBeEnabled();
  await model.focus();
  await model.press('Enter');
  await expect(page.getByRole('option', { name: 'Model B', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(model).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
    .toBe(true);
});

test('catalog failure has retry without losing the draft', async ({ page, app }) => {
  await app.set('model/list', { error: { code: -32000, message: 'Unavailable' } });
  await page.goto(app.url);
  await expect(page.getByRole('combobox', { name: 'Model', exact: true })).toBeDisabled();
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('Keep this');
  await app.set('model/list', { result: { data: models, nextCursor: null } });
  await page.getByRole('button', { name: 'Retry models', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Model', exact: true })).toBeEnabled();
  await expect(composer).toHaveValue('Keep this');
});
