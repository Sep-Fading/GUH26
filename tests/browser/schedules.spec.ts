// Checks the Schedules settings through visible, named controls.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

test('a schedule is created switched off, shows its last run, and can be run, switched on and deleted',
  async ({ page }, info) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    try {
      const agent = await (await app.request('/api/agents', { method: 'POST',
        body: { name: 'Reporter', instructions: 'Report.', active: true } })).json();
      await page.goto(app.url);
      const dialog = await openSettings(page, 'Schedules');
      await expect(dialog.getByText('No schedules yet.')).toBeVisible();
      await dialog.getByRole('button', { name: 'New schedule' }).click();
      await dialog.getByRole('textbox', { name: 'Name' }).fill('Morning report');
      await dialog.getByRole('combobox', { name: 'Agent' }).selectOption(agent.agent.id);
      await dialog.getByRole('textbox', { name: 'Prompt' }).fill('Summarise the open issues.');
      await dialog.getByRole('combobox', { name: 'Runs' }).selectOption('weekly');
      await dialog.getByRole('combobox', { name: 'Approval wait' }).selectOption('15');
      await dialog.getByRole('button', { name: 'Save schedule' }).click();
      const row = dialog.getByRole('group', { name: 'Morning report' });
      await expect(row).toContainText('Off');
      await expect(row).toContainText('Never run');
      const saved = (await (await app.request('/api/schedules')).json()).schedules[0];
      expect([saved.enabled, saved.every, saved.approvalMinutes]).toEqual([false, 'weekly', 15]);

      // The last run's outcome is shown in words, with the reason when it was terminated.
      await page.route('**/api/schedules', route => route.request().method() === 'GET'
        ? route.fulfill({ json: { schedules: [{ ...saved, lastStatus: 'terminated', lastRun: 1790000000,
          lastThread: '019a1111-1111-7111-8111-111111111111',
          lastResult: 'It needed an approval and nobody answered within 15 min.' }] } }) : route.continue());
      await dialog.getByRole('button', { name: 'Reload schedules' }).click();
      await expect(row).toContainText('Terminated');
      await expect(row).toContainText('nobody answered within 15 min');
      await page.screenshot({ path: info.outputPath('schedules.png') });
      await page.unroute('**/api/schedules');

      await row.getByRole('switch', { name: 'Run Morning report on schedule' }).click();
      await expect.poll(async () => (await (await app.request('/api/schedules')).json()).schedules[0].enabled).toBe(true);
      await row.getByRole('button', { name: 'Delete schedule' }).click();
      await page.getByRole('alertdialog').getByRole('button', { name: 'Delete schedule' }).click();
      await expect(dialog.getByText('No schedules yet.')).toBeVisible();
    } finally { await app.close(); }
  });
