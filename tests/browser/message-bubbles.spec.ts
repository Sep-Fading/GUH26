// Checks message alignment and readable prompt bubbles.
import { test, expect } from '@playwright/test';
// Runs disposable chats without touching real history.
import { launchApp } from '../support/harness.mjs';
// Builds saved exchanges using existing protocol fixtures.
import { turn, message } from '../support/fixtures.mjs';

// Supplies a user message with the same shape as saved history.
function prompt(id: string, text: string) {
  return { id, type: 'userMessage', content: [{ type: 'text', text, text_elements: [] }] };
}

test('prompts sit in right-aligned bubbles and replies stay unboxed', async ({ page }, info) => {
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: [turn('completed', [
      prompt('one', 'Make the rail smaller.'),
      message('The rail is narrower. The preview is unchanged.', 'reply-one'),
      prompt('two', 'Keep the preview as it is.'),
      message('Only the rail and its marks changed.', 'reply-two'),
    ])], nextCursor: null,
  } } });
  try {
    await page.goto(app.url);
    const region = page.getByRole('region', { name: 'Conversation', exact: true });
    const prompts = region.getByRole('article', { name: 'Your message', exact: true });
    const replies = region.getByRole('article', { name: 'Assistant message', exact: true });
    await expect(prompts).toHaveCount(2);
    await expect(prompts.first()).toHaveCSS('background-color', 'rgb(0, 113, 227)');
    await expect(prompts.first()).toHaveCSS('color', 'rgb(255, 255, 255)');
    await expect(prompts.first()).toHaveCSS('border-radius', '20px');
    await expect(replies.first()).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
    await expect(region.getByText('You', { exact: true })).toHaveCount(0);
    const bubble = (await prompts.first().boundingBox())!;
    const reply = (await replies.first().boundingBox())!;
    expect(bubble.x).toBeGreaterThan(reply.x);
    expect(Math.abs(bubble.x + bubble.width - reply.x - reply.width)).toBeLessThan(1);
    await page.screenshot({ path: info.outputPath('message-bubbles.png') });
  } finally { await app.close(); }
});

test('long multiline prompts wrap inside narrow bubbles', async ({ page }, info) => {
  const text = 'Keep these lines:\n  exact_key = 1\n' + 'long_identifier_'.repeat(50);
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: [turn('completed', [prompt('one', text), message('Kept unchanged.')])], nextCursor: null,
  } } });
  try {
    await page.goto(app.url);
    await page.setViewportSize({ width: 320, height: 844 });
    const bubble = page.getByRole('article', { name: 'Your message', exact: true });
    await expect(bubble).toHaveText(text);
    await expect(bubble).toHaveCSS('white-space', 'pre-wrap');
    expect(await bubble.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    const parent = (await bubble.locator('..').boundingBox())!;
    const bounds = (await bubble.boundingBox())!;
    expect(bounds.width).toBeLessThanOrEqual(parent.width * 0.93);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(320);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeInViewport();
    await bubble.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath('long-bubble-narrow.png') });
  } finally { await app.close(); }
});
