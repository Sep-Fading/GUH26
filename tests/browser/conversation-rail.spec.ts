// Verifies exchange navigation in actual browser layouts.
import { test, expect } from '@playwright/test';
// Runs isolated chats without real account access.
import { launchApp } from '../support/harness.mjs';
// Supplies saved messages and synthetic streaming updates.
import { turn, message, delta, itemDone } from '../support/fixtures.mjs';
// Finds the new conversation button in the sidebar, opening the drawer on a narrow window.
import { newChatButton } from './settings.js';

// Returns a saved exchange with enough text to require scrolling.
function exchange(index: number) {
  return { ...turn('completed', [
    { id: `prompt-${index}`, type: 'userMessage', content: [
      { type: 'text', text: `Question ${index} about the project`, text_elements: [] },
    ] },
    message(`**Answer ${index}** with *details*.\n\n` + 'Response paragraph.\n'.repeat(12), `answer-${index}`),
  ]), id: `turn-${index}` };
}

test('rail previews exchanges and jumps without moving the composer', async ({ page }, info) => {
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: Array.from({ length: 12 }, (_, i) => exchange(12 - i)), nextCursor: null,
  } } });
  try {
    await page.goto(app.url);
    const rail = page.getByRole('navigation', { name: 'Conversation timeline' });
    await expect(rail.getByRole('button')).toHaveCount(12);
    expect((await rail.boundingBox())!.width).toBeLessThanOrEqual(40);
    const target = rail.getByRole('button', { name: 'Jump to prompt 4: Question 4 about the project', exact: true });
    await expect(target.locator('span')).toHaveCSS('height', '2px');
    expect((await target.boundingBox())!.width).toBeGreaterThanOrEqual(24);
    expect((await target.boundingBox())!.height).toBeGreaterThanOrEqual(24);
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Keep my draft');
    const before = await composer.boundingBox();
    if (info.project.name === 'desktop') {
      const neighbor = rail.getByRole('button').nth(4).locator('span');
      const width = (await neighbor.boundingBox())!.width;
      await target.hover();
      await expect.poll(async () => (await neighbor.boundingBox())!.width).toBeGreaterThan(width);
      const tooltip = page.getByRole('tooltip');
      await expect(tooltip).toHaveCSS('font-size', '14px');
      await expect(tooltip).toHaveCSS('width', '320px');
      await expect(tooltip.locator('.response-preview')).toHaveCSS('-webkit-line-clamp', '4');
      await expect(tooltip.locator('strong').first()).toHaveText('Question 4 about the project');
      await expect(tooltip.locator('strong').nth(1)).toHaveText('Answer 4');
      await expect(tooltip.locator('em')).toHaveText('details');
      await expect(tooltip.locator('.response-preview')).toHaveCSS('font-weight', '400');
      // The preview is readable secondary text, not a background grey.
      await expect(tooltip.locator('.response-preview')).toHaveCSS('color', 'rgb(110, 110, 115)');
      await page.screenshot({ path: info.outputPath('rail-preview.png') });
    }
    await target.click();
    await expect(target).toHaveAttribute('aria-current', 'location');
    // The current mark is drawn in the text colour; the others stay a quiet grey.
    await expect(target.locator('span')).toHaveCSS('background-color', 'rgb(29, 29, 31)');
    await expect.poll(async () => {
      const region = await page.getByRole('region', { name: 'Conversation', exact: true }).boundingBox();
      const prompt = await page.getByRole('article', { name: 'Your message' }).nth(3).boundingBox();
      return Math.abs(prompt!.y - region!.y - 48);
    }).toBeLessThan(40);
    await expect(composer).toHaveValue('Keep my draft');
    expect(await composer.boundingBox()).toEqual(before);
    await target.focus();
    await page.keyboard.press('ArrowDown');
    await expect(rail.getByRole('button').nth(4)).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(rail.getByRole('button').nth(4)).toHaveAttribute('aria-current', 'location');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(target.locator('span')).toHaveCSS('transition-duration', '0s');
    await page.setViewportSize({ width: 320, height: 844 });
    await page.screenshot({ path: info.outputPath('rail-narrow.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  } finally { await app.close(); }
});

test('older history extends the rail and empty chats clear it', async ({ page }) => {
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: [exchange(2)], nextCursor: 'older',
  } } });
  try {
    await page.goto(app.url);
    const rail = page.getByRole('navigation', { name: 'Conversation timeline' });
    await expect(rail.getByRole('button')).toHaveCount(1);
    await app.set('thread/turns/list', { result: { data: [exchange(1)], nextCursor: null } });
    await page.getByRole('button', { name: 'Load older messages', exact: true }).click();
    await expect(rail.getByRole('button')).toHaveCount(2);
    await expect(rail.getByRole('button').first()).toHaveAccessibleName('Jump to prompt 1: Question 1 about the project');
    await (await newChatButton(page)).click();
    await expect(rail).toHaveCount(0);
  } finally { await app.close(); }
});

test('streaming previews update without pulling a reader to the bottom', async ({ page }, info) => {
  const latest = exchange(3);
  latest.items = latest.items.slice(0, 1);
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: [latest, exchange(2), exchange(1)], nextCursor: null,
  } } });
  try {
    await page.goto(app.url);
    const rail = page.getByRole('navigation', { name: 'Conversation timeline' });
    await expect(rail.getByRole('button')).toHaveCount(3);
    await rail.getByRole('button').first().click();
    await expect(rail.getByRole('button').first()).toHaveAttribute('aria-current', 'location');
    const region = page.getByRole('region', { name: 'Conversation', exact: true });
    await expect.poll(() => region.evaluate(el => el.scrollTop)).toBeLessThan(40);
    await app.emit(delta('A **streamed**', 'live-answer'));
    // Assistant messages render Markdown since 2026-09-21, while they stream as well.
    await expect(page.getByRole('article', { name: 'Assistant message' }).last()).toHaveText('A streamed');
    await expect(page.getByRole('article', { name: 'Assistant message' }).last().locator('strong')).toHaveText('streamed');
    await app.emit(delta(' answer.', 'live-answer'));
    await expect(page.getByRole('article', { name: 'Assistant message' }).last()).toHaveText('A streamed answer.');
    expect(await region.evaluate(el => el.scrollTop)).toBeLessThan(40);
    if (info.project.name === 'desktop') {
      await rail.getByRole('button').last().hover();
      await expect(page.getByRole('tooltip').locator('strong').nth(1)).toHaveText('streamed');
      await app.emit(itemDone('Final *result*.', 'live-answer'));
      await expect(page.getByRole('tooltip').locator('em')).toHaveText('result');
    }
  } finally { await app.close(); }
});

test('long rails keep the current mark visible and previews stay inert', async ({ page }, info) => {
  const data = Array.from({ length: 40 }, (_, i) => exchange(40 - i));
  data[0].items[1] = message('<img src="https://example.test/x" onerror="alert(1)">\n\n'
    + '![Hidden](https://example.test/x) [Link](https://example.test)\n\n*Safe* text.', 'answer-40');
  const app = await launchApp({ 'thread/turns/list': { result: { data, nextCursor: null } } });
  try {
    const requests: string[] = [];
    await page.route('https://example.test/**', async route => {
      requests.push(route.request().url()); await route.abort();
    });
    await page.goto(app.url);
    const rail = page.getByRole('navigation', { name: 'Conversation timeline' });
    const last = rail.getByRole('button').last();
    await expect(last).toHaveAttribute('aria-current', 'location');
    await expect.poll(async () => {
      const region = (await rail.boundingBox())!;
      const mark = (await last.boundingBox())!;
      return mark.y >= region.y && mark.y + mark.height <= region.y + region.height + 1;
    }).toBe(true);
    if (info.project.name === 'desktop') {
      await last.hover();
      const tooltip = page.getByRole('tooltip');
      await expect(tooltip.locator('em')).toHaveText('Safe');
      await expect(tooltip.locator('img, a, script, [onerror]')).toHaveCount(0);
      expect(requests).toEqual([]);
    }
    await page.mouse.move(0, 0);
    await page.getByRole('region', { name: 'Conversation', exact: true })
      .evaluate(el => el.scrollTo({ top: 0, behavior: 'instant' }));
    await expect(rail.getByRole('button').first()).toHaveAttribute('aria-current', 'location');
    await expect.poll(() => rail.evaluate(el => el.scrollTop)).toBe(0);
  } finally { await app.close(); }
});

test('messages and the composer share one centre line, with the rail and a scrollbar present', async ({ page }, info) => {
  const app = await launchApp({ 'thread/turns/list': { result: {
    data: Array.from({ length: 12 }, (_, i) => exchange(12 - i)), nextCursor: null } } });
  try {
    await page.goto(app.url);
    await expect(page.getByRole('navigation', { name: 'Conversation timeline' }).getByRole('button')).toHaveCount(12);
    const centre = async (locator: import('@playwright/test').Locator) => {
      const box = (await locator.boundingBox())!;
      return box.x + box.width / 2;
    };
    const messages = await centre(page.locator('[data-conversation-content]'));
    const composer = await centre(page.locator('.composer'));
    expect(Math.abs(messages - composer)).toBeLessThanOrEqual(1);
    // The rail still sits at the left edge and does not cover the text.
    const rail = (await page.getByRole('navigation', { name: 'Conversation timeline' }).boundingBox())!;
    const text = (await page.locator('[data-conversation-content]').boundingBox())!;
    if (info.project.name === 'desktop') expect(rail.x + rail.width).toBeLessThanOrEqual(text.x);
  } finally { await app.close(); }
});
