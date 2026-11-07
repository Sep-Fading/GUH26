// Checks profile editing and responsive browser behavior.
import { test as base, expect } from '@playwright/test';
// Runs disposable backends with inspectable protocol fixtures.
import { launchApp, eventually } from '../support/harness.mjs';
// Completes synthetic responses without calling real models.
import { itemDone, turnDone } from '../support/fixtures.mjs';
// Opens a category of the Settings sheet.
import { openSettings, openTools } from './settings.js';

const test = base.extend<{ app: Awaited<ReturnType<typeof launchApp>> }>({
  // Supplies an empty chat list and closes the fixture after each case.
  app: async ({}, use) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    try { await use(app); } finally { await app.close(); }
  },
});

test('define, activate, and chat with an agent', async ({ page, app }, info) => {
  await page.goto(app.url);
  await openSettings(page, 'Agents');
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Editor');
  await dialog.getByRole('textbox', { name: 'Instructions', exact: true })
    .fill('Review the text for clarity. Preserve code identifiers.');
  await expect(dialog.getByRole('checkbox', { name: 'Active', exact: true })).not.toBeChecked();
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Editor', exact: true })).toBeVisible();
  await dialog.getByRole('checkbox', { name: 'Active', exact: true }).check();
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Save agent', exact: true })).toBeEnabled();
  await page.screenshot({ path: info.outputPath('agent-manager.png') });
  await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  const picker = page.getByRole('combobox', { name: 'Agent', exact: true });
  await picker.click();
  await page.getByRole('option', { name: 'Editor', exact: true }).click();
  await expect((await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true })).toBeDisabled();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Review this text.');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  const state = await eventually(() => app.inspect(), s =>
    s.requests.some(r => r.method === 'turn/start'));
  expect(state.requests.find(r => r.method === 'thread/start').params.developerInstructions)
    .toBe('Review the text for clarity. Preserve code identifiers.');
  await app.emit(itemDone('Review complete.'), turnDone());
  await expect(page.getByRole('article', { name: 'Assistant message' }))
    .toHaveText('Review complete.');
  await expect(picker).toContainText('Editor');
  await expect(picker).toBeDisabled();
  await page.screenshot({ path: info.outputPath('agent-chat.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
    .toBe(true);
  await page.setViewportSize({ width: 320, height: 640 });
  await expect(picker).toBeInViewport();
  await openSettings(page, 'Agents');
  await dialog.getByRole('button', { name: 'Editor', exact: true }).click();
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).scrollIntoViewIfNeeded();
  await expect(dialog.getByRole('button', { name: 'Save agent', exact: true })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
    .toBe(true);
  await page.screenshot({ path: info.outputPath('agent-narrow.png') });
});

test('unsupported access remains inactive and does not disappear on edit', async ({ page, app }) => {
  const response = await app.request('/api/agents', { method: 'POST', body: {
    name: 'Scoped agent', instructions: 'Work only within granted access.',
    capabilities: ['computer_use'], access: {
      applications: [{ id: 'org.blenderfoundation.blender', actions: ['observe', 'control'] }],
      files: [], network: [],
    },
  } });
  expect(response.status).toBe(201);
  await page.goto(app.url);
  await openSettings(page, 'Agents');
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await dialog.getByRole('button', { name: 'Scoped agent', exact: true }).click();
  await expect(dialog.getByRole('checkbox', { name: 'Active', exact: true })).toBeDisabled();
  await expect(dialog).toContainText('Computer Use');
  await expect(dialog).toContainText('org.blenderfoundation.blender');
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Renamed agent');
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Renamed agent', exact: true })).toBeVisible();
  const saved = await (await app.request('/api/agents')).json();
  expect(saved.agents[0].access.applications[0].id).toBe('org.blenderfoundation.blender');
  expect(saved.agents[0].active).toBe(false);
});

test('select skill groups when creating and editing agents', async ({ page, app }) => {
  const response = await app.request('/api/skill-groups', { method: 'POST', body: {
    name: 'Writing group', active: true, members: [] } });
  const { group } = await response.json();
  await page.goto(app.url);
  await openSettings(page, 'Agents');
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Writer');
  await dialog.getByRole('textbox', { name: 'Instructions', exact: true }).fill('Write clearly.');
  const choice = dialog.getByRole('group', { name: 'Skill groups', exact: true })
    .getByRole('checkbox', { name: 'Writing group', exact: true });
  await choice.check();
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Writer', exact: true })).toBeVisible();
  expect((await (await app.request('/api/agents')).json()).agents[0].skillGroupIds).toEqual([group.id]);
  await page.reload();
  await openSettings(page, 'Agents');
  await dialog.getByRole('button', { name: 'Writer', exact: true }).click();
  await expect(choice).toBeChecked();
  await choice.uncheck();
  await dialog.getByRole('button', { name: 'Save agent', exact: true }).click();
  await expect.poll(async () => (await (await app.request('/api/agents')).json()).agents[0].skillGroupIds).toEqual([]);
});
