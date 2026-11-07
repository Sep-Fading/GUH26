// Checks browser geometry and user interactions.
import { test as base, expect, type Locator } from '@playwright/test';
// Starts isolated application and Codex fixtures.
import { launchApp } from '../support/harness.mjs';
// Supplies model choices and completed response events.
import { models, itemDone, turnDone } from '../support/fixtures.mjs';

const test = base.extend<{ app: Awaited<ReturnType<typeof launchApp>> }>({
  // Runs each test with an empty saved-task list and disposes its processes afterward.
  app: async ({}, use) => {
    const app = await launchApp({
      'thread/list': { result: { data: [], nextCursor: null } },
    });
    try { await use(app); } finally { await app.close(); }
  },
});

// Returns the visible line count from the textarea's actual content box.
async function lines(input: Locator) {
  return input.evaluate(element => {
    const style = getComputedStyle(element);
    const padding = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    return (element.clientHeight - padding) / parseFloat(style.lineHeight);
  });
}

// Verifies the selectors use their labels' space and leave the remaining width before Send.
test('selectors stay compact on the left', async ({ page, app }) => {
  await page.goto(app.url);
  const machine = page.getByRole('combobox', { name: 'Machine', exact: true });
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  const effort = page.getByRole('combobox', { name: 'Reasoning effort', exact: true });
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(model).toContainText('Model A');
  const machineBox = (await machine.boundingBox())!;
  const modelBox = (await model.boundingBox())!;
  const effortBox = (await effort.boundingBox())!;
  const inputBox = (await input.boundingBox())!;
  const textWidth = await model.locator('span').evaluate(element => {
    const range = document.createRange();
    range.selectNodeContents(element);
    return range.getBoundingClientRect().width;
  });
  expect(modelBox.width - textWidth).toBeLessThanOrEqual(44);
  expect(modelBox.x - machineBox.x - machineBox.width).toBeLessThanOrEqual(8);
  expect(effortBox.x - modelBox.x - modelBox.width).toBeLessThanOrEqual(8);
  expect(Math.abs(modelBox.y - effortBox.y)).toBeLessThanOrEqual(1);
  expect(Math.abs(machineBox.x - inputBox.x)).toBeLessThanOrEqual(1);
  const sendBox = (await page.getByRole('button', { name: 'Send', exact: true })
    .boundingBox())!;
  expect(sendBox.x).toBeGreaterThanOrEqual(effortBox.x + effortBox.width);
  expect(Math.abs(sendBox.x + sendBox.width - inputBox.x - inputBox.width))
    .toBeLessThanOrEqual(1);
});

// Checks growth, the four-line cap, scrolling, and shrinkage after editing or sending.
test('input grows to four lines then scrolls', async ({ page, app }, info) => {
  await page.goto(app.url);
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(input).toBeEnabled();
  await expect.poll(() => lines(input)).toBeCloseTo(1, 1);
  for (const count of [2, 3, 4]) {
    await input.fill(Array.from({ length: count }, (_, i) => `Line ${i + 1}`).join('\n'));
    await expect.poll(() => lines(input)).toBeCloseTo(count, 1);
  }
  await input.fill('One\nTwo\nThree\nFour\nFive\nSix');
  await expect.poll(() => lines(input)).toBeCloseTo(4, 1);
  expect(await input.evaluate(element => element.scrollHeight > element.clientHeight))
    .toBe(true);
  await expect(input).toHaveCSS('overflow-y', 'auto');
  await input.press('ControlOrMeta+End');
  expect(await input.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeInViewport();
  await page.screenshot({ path: info.outputPath('composer-four-lines.png') });
  await input.fill('Short');
  await expect.poll(() => lines(input)).toBeCloseTo(1, 1);
  await input.fill('');
  await expect.poll(() => lines(input)).toBeCloseTo(1, 1);
  await page.screenshot({ path: info.outputPath('composer-empty.png') });
  await input.fill('First\nSecond\nThird');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(input).toHaveValue('');
  await expect.poll(() => lines(input)).toBeCloseTo(1, 1);
  await app.emit(itemDone('Done'), turnDone());
  await expect(input).toBeEnabled();
});

// Checks automatic reflow after viewport changes and containment of long selector labels.
test('wrapped drafts and long choices fit narrow screens', async ({ page, app }, info) => {
  await app.set('model/list', { result: {
    data: models.map(model => ({ ...model,
      displayName: 'An unusually long model name for layout verification' })),
    nextCursor: null,
  } });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(app.url);
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(input).toBeEnabled();
  await input.fill('A wrapped draft with enough words to span a few lines. '.repeat(3));
  const wide = await lines(input);
  expect(wide).toBeGreaterThanOrEqual(1);
  expect(wide).toBeLessThan(4);
  await page.setViewportSize({ width: 320, height: 640 });
  await expect.poll(() => lines(input)).toBeCloseTo(4, 1);
  expect(await input.evaluate(element => element.scrollHeight > element.clientHeight))
    .toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
    .toBe(true);
  const effort = page.getByRole('combobox', { name: 'Reasoning effort', exact: true });
  const model = page.getByRole('combobox', { name: 'Model', exact: true });
  await expect(effort).toBeInViewport();
  const modelBox = (await model.boundingBox())!;
  const effortBox = (await effort.boundingBox())!;
  expect(modelBox.x + modelBox.width).toBeLessThanOrEqual(effortBox.x);
  await model.click();
  await expect(page.getByRole('listbox')).toBeInViewport();
  await page.keyboard.press('Escape');
  await page.screenshot({ path: info.outputPath('composer-narrow.png') });
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect.poll(() => lines(input)).toBeCloseTo(wide, 1);
});
