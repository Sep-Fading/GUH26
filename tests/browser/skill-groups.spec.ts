// Exercises group controls through accessible names.
import { test, expect } from '@playwright/test';
// Starts disposable backends without real account data.
import { launchApp, eventually } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

test('groups retain membership and member switches without global activation', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { skill } = await (await app.request('/api/skills', { method: 'POST', body: {
      name: 'Clarity', instructions: 'Keep exact names.',
    } })).json();
    await page.goto(app.url);
    await openSettings(page, 'Skills');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await dialog.getByRole('button', { name: 'New group', exact: true }).click();
    await dialog.getByRole('textbox', { name: 'Group name', exact: true }).fill('Writing');
    // One box per skill (user decision, 2026-09-21): ticked means the skill is in the group and on.
    await expect(dialog.getByRole('checkbox', { name: 'Include Clarity', exact: true })).toHaveCount(0);
    await expect(dialog.getByText('Skill', { exact: true })).toBeVisible();
    await expect(dialog.getByText('Enabled', { exact: true })).toBeVisible();
    await dialog.getByRole('checkbox', { name: 'Enable Clarity in this group', exact: true }).check();
    await dialog.getByRole('checkbox', { name: 'Group active', exact: true }).check();
    await dialog.getByRole('button', { name: 'Save group', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    let groups = (await (await app.request('/api/skill-groups')).json()).groups;
    expect(groups[0].members).toEqual([{ skillId: skill.id, enabled: true }]);
    expect((await (await app.request('/api/skills')).json()).skills[0].active).toBe(false);
    await dialog.getByRole('checkbox', { name: 'Group active', exact: true }).uncheck();
    await dialog.getByRole('button', { name: 'Save group', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    await dialog.getByRole('checkbox', { name: 'Group active', exact: true }).check();
    await expect(dialog.getByRole('checkbox', { name: 'Enable Clarity in this group', exact: true })).toBeChecked();
    // Unticking takes the skill out of the group.
    await dialog.getByRole('checkbox', { name: 'Enable Clarity in this group', exact: true }).uncheck();
    await dialog.getByRole('button', { name: 'Save group', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect((await (await app.request('/api/skill-groups')).json()).groups[0].members).toEqual([]);
    if (info.project.name === 'mobile') await page.setViewportSize({ width: 320, height: 844 });
    await page.screenshot({ path: info.outputPath('skill-group-editor.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await dialog.getByRole('button', { name: 'Delete group', exact: true }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    await dialog.getByRole('button', { name: 'Delete group', exact: true }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Delete group', exact: true }).click();
    await expect(page.getByRole('alertdialog')).toBeHidden();
    groups = (await (await app.request('/api/skill-groups')).json()).groups;
    expect(groups).toEqual([]);
    expect((await (await app.request('/api/skills')).json()).skills).toHaveLength(1);
  } finally { await app.close(); }
});

test('new conversations select groups before sending without changing the draft', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const { skill } = await (await app.request('/api/skills', { method: 'POST', body: {
      name: 'Clarity', instructions: 'GROUP_BROWSER_MARKER',
    } })).json();
    const { group } = await (await app.request('/api/skill-groups', { method: 'POST', body: {
      name: 'Writing', active: true, members: [{ skillId: skill.id, enabled: true }],
    } })).json();
    await page.goto(app.url);
    const message = page.getByRole('textbox', { name: 'Message', exact: true });
    await message.fill('Keep my draft');
    await page.getByRole('button', { name: 'Conversation skills', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Conversation skills', exact: true });
    await dialog.getByRole('checkbox', { name: 'Writing', exact: true }).check();
    await dialog.getByRole('button', { name: 'Save selection', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(message).toHaveValue('Keep my draft');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const state = await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/start'));
    expect(state.requests.find((r: any) => r.method === 'turn/start').params
      .additionalContext.miniclaw_skills.value).toContain('GROUP_BROWSER_MARKER');
    await page.getByRole('button', { name: 'Conversation skills', exact: true }).click();
    await expect(dialog.getByRole('checkbox', { name: 'Writing', exact: true })).toBeChecked();
    await page.screenshot({ path: info.outputPath('conversation-skills.png') });
    await dialog.getByRole('checkbox', { name: 'Writing', exact: true }).uncheck();
    await dialog.getByRole('button', { name: 'Save selection', exact: true }).click();
    await expect(dialog).toBeHidden();
    expect((await app.inspect()).requests.some((r: any) => r.method === 'turn/interrupt')).toBe(false);
    expect(group.id).toBeTruthy();
  } finally { await app.close(); }
});

test('projects select groups without activating individual skills', async ({ page }) => {
  const app = await launchApp();
  try {
    const { group } = await (await app.request('/api/skill-groups', { method: 'POST', body: {
      name: 'Writing', active: true, members: [],
    } })).json();
    await page.goto(app.url);
    const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
    if (await toggle.isVisible()) await toggle.click();
    await page.getByRole('button', { name: 'New project', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings', exact: true });
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Work');
    await dialog.getByRole('checkbox', { name: 'Writing', exact: true }).check();
    await dialog.getByRole('button', { name: 'Save project', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect((await (await app.request('/api/projects')).json()).projects[0].skillGroupIds).toEqual([group.id]);
  } finally { await app.close(); }
});

test('selection failures require reload and retain unsaved changes', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await app.request('/api/skill-groups', { method: 'POST', body: { name: 'Writing', active: true } });
    let failRead = true;
    await page.route('**/api/threads/*/skills', async route => {
      if (route.request().method() === 'GET' && !failRead) return route.continue();
      await route.fulfill({ status: route.request().method() === 'GET' ? 503 : 409,
        contentType: 'application/json', body: JSON.stringify({ error: {
          code: 'fixture_error', message: 'Settings unavailable.', retryable: false,
        } }) });
    });
    await page.goto(app.url);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Test selection errors');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Conversation skills', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Conversation skills', exact: true });
    await expect(dialog.getByRole('alert')).toHaveText('Settings unavailable.');
    await expect(dialog.getByRole('button', { name: 'Save selection', exact: true })).toBeDisabled();
    failRead = false;
    await dialog.getByRole('button', { name: 'Reload selection', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Save selection', exact: true })).toBeEnabled();
    await dialog.getByRole('checkbox', { name: 'Writing', exact: true }).check();
    await dialog.getByRole('button', { name: 'Save selection', exact: true }).click();
    await expect(dialog.getByRole('alert')).toHaveText('Settings unavailable.');
    await expect(dialog.getByRole('checkbox', { name: 'Writing', exact: true })).toBeChecked();
    await expect(dialog).toBeVisible();
  } finally { await app.close(); }
});
