// Verifies explicit recovery and responsive nested dialogs.
import { test, expect } from '@playwright/test';
// Seeds only private synthetic creation metadata.
import { DatabaseSync } from 'node:sqlite';
// Uses valid identifiers for disposable creation intents.
import { randomUUID } from 'node:crypto';
// Resolves metadata paths inside the test fixture.
import { join } from 'node:path';
// Launches isolated backends without real account access.
import { launchApp } from '../support/harness.mjs';
// Keeps the existing synthetic chat admitted by baseline.
import { THREAD } from '../support/fixtures.mjs';

test('explicit recovery preserves draft and denial after restart', async ({ page }) => {
  const app = await launchApp();
  const db = new DatabaseSync(join(app.peer.dir, 'agents.sqlite3'));
  try {
    for (let i = 0; i < 2; i++) {
      const id = randomUUID();
      db.prepare('INSERT INTO chat_creations(id, baseline) VALUES(?, ?)').run(id, JSON.stringify([THREAD]));
      db.prepare('INSERT INTO chat_admissions(thread_id, required) VALUES(?, 0)').run(id);
    }
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(input).toBeEnabled();
    await input.fill('Keep this unsent draft');
    await page.getByRole('button', { name: 'Notifications', exact: true }).click();
    await page.getByRole('button', { name: 'Creation recovery', exact: true }).click();
    const recovery = page.getByRole('dialog', { name: 'Creation recovery', exact: true });
    await expect(recovery.getByText('2 unconfirmed creations. 0 known receipts are preserved.')).toBeVisible();
    const checkbox = recovery.getByRole('checkbox');
    const action = recovery.getByRole('button', { name: 'Free creation capacity' });
    await expect(checkbox).not.toBeChecked();
    await expect(action).toBeDisabled();
    await page.setViewportSize({ width: 320, height: 640 });
    await checkbox.check();
    await action.click();
    await expect(recovery.getByRole('status')).toContainText('Creation capacity was freed.');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.keyboard.press('Escape');
    await expect(recovery).not.toBeVisible();
    await page.keyboard.press('Escape');
    await expect(input).toHaveValue('Keep this unsent draft');
    expect((await app.inspect()).requests.some(r => r.method === 'thread/start' || r.method === 'turn/start')).toBe(false);
    await app.restart();
    await page.reload();
    await expect(input).toBeEnabled();
    await page.getByRole('button', { name: 'Notifications', exact: true }).click();
    await page.getByRole('button', { name: 'Creation recovery', exact: true }).click();
    await expect(recovery.getByText('There are no unconfirmed creations to recover.')).toBeVisible();
    expect(db.prepare('SELECT revision FROM creation_quarantine').get()?.revision).toBe(1);
  } finally { db.close(); await app.close(); }
});
