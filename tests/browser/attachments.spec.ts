// Checks files and pictures in a real browser: choosing, pasting, removing, sending a message of
// files alone, and what the transcript shows afterwards.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, eventually } from '../support/harness.mjs';
// Opens an empty composer.
import { newChatButton } from './settings.js';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');

test('files are attached, removed, sent and shown in the conversation', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    await (await newChatButton(page)).click();
    await expect(page.getByRole('button', { name: 'Attach files' })).toBeEnabled();
    await page.locator('[data-attach-input]').setInputFiles([
      { name: 'tree.png', mimeType: 'image/png', buffer: PNG },
      { name: 'notes.csv', mimeType: 'text/csv', buffer: Buffer.from('alpha,beta\n1,2\n') },
      { name: 'wrong.txt', mimeType: 'text/plain', buffer: Buffer.from('not this one') }]);
    const waiting = page.getByRole('list', { name: 'Files to send' });
    await expect(waiting.getByRole('listitem')).toHaveCount(3);
    // The draft's thumbnail is really drawn, which the page's content policy has to allow.
    await expect.poll(() => waiting.locator('img').first().evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    await waiting.getByRole('button', { name: 'Remove wrong.txt' }).click();
    await expect(waiting.getByRole('listitem')).toHaveCount(2);
    expect((await new AxeBuilder({ page }).include('.composer').analyze()).violations).toEqual([]);
    // Files alone are a message.
    const send = page.getByRole('button', { name: 'Send', exact: true });
    await expect(send).toBeEnabled();
    await send.click();
    const started = await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/start'), 'turn');
    const turn = started.requests.find(request => request.method === 'turn/start').params;
    expect(turn.input.map((part: { type: string }) => part.type)).toEqual(['localImage', 'text']);
    expect(turn.additionalContext.miniclaw_attachments.value).toContain('alpha,beta');
    await expect(waiting).toHaveCount(0);
    const sent = page.getByRole('list', { name: 'Attached files' });
    await expect(sent.getByRole('img', { name: 'tree.png' })).toBeVisible();
    await expect(sent.getByRole('link', { name: 'Download notes.csv' })).toBeVisible();
    // The picture is really served: a broken image has no natural width.
    await expect.poll(() => sent.getByRole('img', { name: 'tree.png' }).evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    expect((await new AxeBuilder({ page }).include('[data-conversation-content]').analyze()).violations).toEqual([]);
  } finally { await app.close(); }
});

test('a pasted picture joins the draft', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    await (await newChatButton(page)).click();
    const box = page.getByRole('textbox', { name: 'Message', exact: true });
    await box.focus();
    await page.evaluate(bytes => {
      const data = new DataTransfer();
      data.items.add(new File([new Uint8Array(bytes)], 'shot.png', { type: 'image/png' }));
      document.activeElement!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
    }, [...PNG]);
    await expect(page.getByRole('list', { name: 'Files to send' }).getByRole('listitem')).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Remove shot.png' })).toBeVisible();
  } finally { await app.close(); }
});
