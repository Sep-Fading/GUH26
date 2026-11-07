// Checks the file transfer dialog through visible, named controls.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';

type Page = import('@playwright/test').Page;
const job = '019a9999-9999-7999-8999-999999999999';

// Opens a chat on the running Master VM and then its transfer dialog.
async function openTransfers(page: Page, url: string) {
  await page.route('**/api/vms/master/status', route => route.fulfill({ json: { state: 'running', ready: true } }));
  await page.route('**/api/vms/master/desktop?*', route => route.fulfill({
    body: Buffer.from('/9j/2Q==', 'base64'), contentType: 'image/jpeg' }));
  await page.goto(url);
  await page.getByRole('combobox', { name: 'Machine' }).click();
  await page.getByRole('option', { name: 'Master VM' }).click();
  await page.getByRole('complementary', { name: 'Guest desktop' }).getByRole('button', { name: 'Transfer files' }).click();
  return page.getByRole('dialog', { name: 'Transfer files' });
}

test('files go to the VM from the macOS panel, and a name conflict asks before anything is replaced',
  async ({ page }, info) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    const posted: unknown[] = [];
    const views = [
      { state: 'running', totalFiles: 2, doneFiles: 0, totalBytes: 2000, doneBytes: 500, skipped: 0, current: 'site/index.html', conflict: null, error: null },
      { state: 'conflict', totalFiles: 2, doneFiles: 1, totalBytes: 2000, doneBytes: 1000, skipped: 0, current: 'notes.txt', conflict: 'notes.txt', error: null },
      { state: 'done', totalFiles: 2, doneFiles: 2, totalBytes: 2000, doneBytes: 2000, skipped: 1, current: 'notes.txt', conflict: null, error: null },
    ];
    let step = 0;
    try {
      await page.route('**/api/vms/master/transfers/pick', route => {
        posted.push(route.request().postDataJSON());
        return route.fulfill({ json: { selectionId: 'sel-1', items: [
          { name: 'site', kind: 'folder', size: null }, { name: 'notes.txt', kind: 'file', size: 1000 }] } });
      });
      await page.route('**/api/vms/master/transfers', route => {
        posted.push(route.request().postDataJSON());
        return route.fulfill({ status: 202, json: { jobId: job } });
      });
      await page.route(`**/api/transfers/${job}`, route => route.fulfill({ json: views[step] }));
      await page.route(`**/api/transfers/${job}/resolve`, route => {
        posted.push(route.request().postDataJSON()); step = 2;
        return route.fulfill({ json: {} });
      });
      const dialog = await openTransfers(page, app.url);
      await expect(dialog.getByText('/Users/Shared/MiniClaw/Transfers')).toBeVisible();
      await dialog.getByRole('button', { name: 'Choose files on this Mac' }).click();
      await expect(dialog.getByRole('listitem').filter({ hasText: 'notes.txt' })).toBeVisible();
      await expect(dialog.getByRole('listitem').filter({ hasText: 'site' })).toContainText('folder');
      await dialog.getByRole('button', { name: 'Send to VM' }).click();
      await expect(dialog.getByRole('progressbar', { name: 'Transfer progress' })).toBeVisible();
      step = 1;
      const conflict = dialog.getByRole('group', { name: 'Name conflict' });
      await expect(conflict).toContainText('notes.txt already exists');
      await page.screenshot({ path: info.outputPath('transfer-conflict.png') });
      await conflict.getByRole('checkbox', { name: 'Apply to all' }).check();
      await conflict.getByRole('button', { name: 'Keep both' }).click();
      await expect(dialog.getByRole('status')).toContainText('2 files transferred. 1 skipped.');
      expect(posted).toEqual([{ kind: 'files' }, { direction: 'to_vm', selectionId: 'sel-1' },
        { choice: 'keep_both', all: true }]);
    } finally { await app.close(); }
  });

test('files come back from the VM: browse the two offered folders, then pick the Mac folder', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const posted: unknown[] = [];
  const listed: string[] = [];
  try {
    await page.route('**/api/vms/master/transfers/files?*', route => {
      const query = new URL(route.request().url()).searchParams;
      listed.push(`${query.get('folder')}:${query.get('path')}`);
      return route.fulfill({ json: { entries: query.get('path') === 'out'
        ? [{ name: 'result.bin', kind: 'file', size: 900000 }]
        : [{ name: 'out', kind: 'folder', size: 0 }, { name: 'readme.md', kind: 'file', size: 6 }] } });
    });
    await page.route('**/api/vms/master/transfers/pick', route => {
      posted.push(route.request().postDataJSON());
      return route.fulfill({ json: { selectionId: 'dest-1', items: [{ name: 'Downloads', kind: 'folder', size: null }] } });
    });
    await page.route('**/api/vms/master/transfers', route => {
      posted.push(route.request().postDataJSON());
      return route.fulfill({ status: 202, json: { jobId: job } });
    });
    await page.route(`**/api/transfers/${job}`, route => route.fulfill({ json: {
      state: 'failed', totalFiles: 2, doneFiles: 1, totalBytes: 10, doneBytes: 6, skipped: 0,
      current: 'out/result.bin', conflict: null, error: 'The VM disconnected during the transfer.' } }));
    const dialog = await openTransfers(page, app.url);
    await dialog.getByRole('tab', { name: 'From VM' }).click();
    await dialog.getByRole('checkbox', { name: 'readme.md' }).check();
    await dialog.getByRole('button', { name: 'Open folder out' }).click();
    await dialog.getByRole('checkbox', { name: 'result.bin' }).check();
    await dialog.getByRole('button', { name: 'Save to this Mac' }).click();
    await expect(dialog.getByRole('alert')).toContainText('The VM disconnected during the transfer.');
    await expect(dialog.getByRole('alert')).toContainText('1 of 2 files arrived whole');
    expect(listed.slice(0, 2)).toEqual(['workspace:', 'workspace:out']);
    expect(posted).toEqual([{ kind: 'destination' }, { direction: 'to_mac', selectionId: 'dest-1',
      sources: [{ folder: 'workspace', path: 'readme.md' }, { folder: 'workspace', path: 'out/result.bin' }] }]);
  } finally { await app.close(); }
});

test('cancelling the macOS panel changes nothing, and a running transfer can be cancelled', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  let picks = 0, cancelled = 0;
  try {
    await page.route('**/api/vms/master/transfers/pick', route => route.fulfill({ json: ++picks === 1
      ? { cancelled: true } : { selectionId: 'sel-2', items: [{ name: 'big.bin', kind: 'file', size: 6000000 }] } }));
    await page.route('**/api/vms/master/transfers', route => route.fulfill({ status: 202, json: { jobId: job } }));
    await page.route(`**/api/transfers/${job}`, route => route.fulfill({ json: {
      state: cancelled ? 'cancelled' : 'running', totalFiles: 1, doneFiles: 0, totalBytes: 6000000,
      doneBytes: 1000000, skipped: 0, current: 'big.bin', conflict: null, error: null } }));
    await page.route(`**/api/transfers/${job}/cancel`, route => { cancelled++; return route.fulfill({ json: {} }); });
    const dialog = await openTransfers(page, app.url);
    await dialog.getByRole('button', { name: 'Choose files on this Mac' }).click();
    await expect(dialog.getByRole('button', { name: 'Send to VM' })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Choose files on this Mac' }).click();
    await dialog.getByRole('button', { name: 'Send to VM' }).click();
    await dialog.getByRole('button', { name: 'Cancel transfer' }).click();
    await expect(dialog.getByRole('status')).toContainText('Cancelled. Files that arrived whole were kept');
    expect(cancelled).toBe(1);
  } finally { await app.close(); }
});
