// Checks sidebar names through browser animations.
import { test as base, expect, type Locator, type Page } from '@playwright/test';
// Starts disposable backends with synthetic conversations only.
import { launchApp, json } from '../support/harness.mjs';
// Supplies distinct provider records for sidebar rows.
import { THREAD, OTHER, message, started, thread } from '../support/fixtures.mjs';

const projectName = 'A project with a long name that needs its final words to be readable';
const groupedName = 'A conversation inside the project with important details at the very end';
const looseName = 'An independent conversation with a long descriptive name and a readable ending';
const shortId = '019a5555-5555-7555-8555-555555555555';

const test = base.extend<{ app: Awaited<ReturnType<typeof launchApp>> }>({
  // Creates named rows in both sidebar sections without model calls.
  app: async ({}, use) => {
    const app = await launchApp();
    try {
      const { project } = await json(await app.request('/api/projects', { method: 'POST',
        body: { name: projectName, environment: { kind: 'host' } } }), 201);
      const rows = [[THREAD, groupedName], [OTHER, looseName], [shortId, 'Short']];
      for (const [id, name] of rows) {
        await app.set('thread/start', { result: started(app.peer.cwd, { id, name }) });
        await json(await app.request('/api/threads', { method: 'POST', body: {
          environment: { kind: 'host' }, ...(id === THREAD ? { projectId: project.id } : {}),
        } }), 201);
        await json(await app.request(`/api/threads/${id}/name`, { method: 'POST', body: { name } }));
      }
      await app.set('thread/list', { result: { data: rows.map(([id, name]) => thread(app.peer.cwd, { id, name })), nextCursor: null } });
      await use(app);
    } finally { await app.close(); }
  },
});

// Opens the shared navigation, including its narrow-screen drawer.
async function navigation(page: Page, url: string) {
  await page.goto(url);
  const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await toggle.isVisible()) await toggle.click();
  return page.getByRole('navigation', { name: 'Tasks', exact: true });
}

// Counts running animations on the name and its enclosing button.
const animations = (button: Locator) => button.evaluate(el => el.getAnimations({ subtree: true }).length);

test('long sidebar names scroll and reset', async ({ page, app }) => {
  const nav = await navigation(page, app.url);
  for (const name of [projectName, groupedName, looseName]) {
    const button = nav.getByRole('button', { name, exact: true });
    await expect(button).toBeVisible();
    await button.hover();
    await expect.poll(() => animations(button)).toBe(1);
    await expect.poll(() => button.evaluate(el => {
      const target = (el.getAnimations({ subtree: true })[0]?.effect as KeyframeEffect)?.target;
      return target ? new DOMMatrix(getComputedStyle(target).transform).m41 : 0;
    })).toBeLessThan(0);
    const end = await button.evaluate(el => {
      const animation = el.getAnimations({ subtree: true })[0];
      animation.pause();
      animation.currentTime = Number(animation.effect!.getTiming().duration) * 0.98;
      const text = (animation.effect as KeyframeEffect).target as HTMLElement;
      return { right: text.getBoundingClientRect().right,
        edge: text.parentElement!.getBoundingClientRect().right };
    });
    expect(Math.abs(end.right - end.edge)).toBeLessThanOrEqual(2);
    await expect(button).toHaveAccessibleName(name);
    await page.mouse.move(0, 0);
    await expect.poll(() => animations(button)).toBe(0);
  }
  const short = nav.getByRole('button', { name: 'Short', exact: true });
  await short.hover();
  expect(await animations(short)).toBe(0);
  await page.mouse.move(0, 0);
  await page.keyboard.press('Tab');
  const focused = nav.getByRole('button', { name: looseName, exact: true });
  await focused.focus();
  await expect.poll(() => animations(focused)).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('reduced motion reveals names without scrolling', async ({ page, app }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const nav = await navigation(page, app.url);
  for (const name of [projectName, groupedName, looseName]) {
    const button = nav.getByRole('button', { name, exact: true });
    await button.hover();
    expect(await animations(button)).toBe(0);
    const fits = await button.evaluate((el, name) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        const node = walker.currentNode;
        if (node.textContent !== name || node.parentElement?.closest('[aria-hidden="true"]')) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        const bounds = el.getBoundingClientRect();
        return [...range.getClientRects()].every(rect => rect.left >= bounds.left && rect.right <= bounds.right + 1);
      }
      return false;
    }, name);
    expect(fits).toBe(true);
  }
});

test('working chats show circles in both sections', async ({ page, app }) => {
  const nav = await navigation(page, app.url);
  const indicator = (name: string) => nav.getByRole('status', { name: `Working on ${name}`, exact: true });
  await expect(nav.getByRole('status')).toHaveCount(0);
  await app.set('turn/start', { result: { turn: { id: 'busy-turn', status: 'inProgress', items: [] } } });
  for (const id of [THREAD, OTHER]) {
    await json(await app.request(`/api/threads/${id}/messages`, { method: 'POST', body: {
      text: 'Work', clientMessageId: crypto.randomUUID(),
    } }), 202);
  }
  for (const name of [groupedName, looseName]) {
    await expect(indicator(name)).toBeVisible({ timeout: 8000 });
    await expect.poll(() => animations(indicator(name))).toBe(1);
    await expect(nav.getByRole('button', { name, exact: true })).toBeVisible();
  }
  await expect(indicator('Short')).toHaveCount(0);
  await page.screenshot({ path: `/tmp/miniclaw-working-${test.info().project.name}.png` });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect.poll(() => animations(indicator(looseName))).toBe(0);
  await app.emit(...[THREAD, OTHER].map(threadId => ({ method: 'turn/completed', params: {
    threadId, turn: { id: 'busy-turn', status: 'completed', items: [] },
  } })));
  await expect(nav.getByRole('status')).toHaveCount(0, { timeout: 8000 });
});

test('background replies show a dot until opened', async ({ page, app }) => {
  const nav = await navigation(page, app.url);
  await nav.getByRole('button', { name: 'Short', exact: true }).click();
  const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await toggle.isVisible()) await toggle.click();
  const ready = (name: string) => nav.getByRole('status', { name: `Response ready in ${name}`, exact: true });
  await app.set('turn/start', { result: { turn: { id: 'busy-turn', status: 'inProgress', items: [] } } });
  for (const id of [THREAD, OTHER]) {
    await json(await app.request(`/api/threads/${id}/messages`, { method: 'POST', body: {
      text: 'Work', clientMessageId: crypto.randomUUID(),
    } }), 202);
  }
  await expect(nav.getByRole('status', { name: `Working on ${groupedName}` })).toBeVisible({ timeout: 8000 });
  await expect(nav.getByRole('status', { name: `Working on ${looseName}` })).toBeVisible({ timeout: 8000 });
  await app.emit(...[THREAD, OTHER].flatMap((threadId, index) => [
    { method: 'item/completed', params: { threadId, turnId: 'busy-turn',
      item: message('The response is ready.', `ready-${index}`) } },
    { method: 'turn/completed', params: { threadId,
      turn: { id: 'busy-turn', status: 'completed', items: [] } } },
  ]));
  await expect(ready(groupedName)).toBeVisible({ timeout: 8000 });
  await expect(ready(looseName)).toBeVisible({ timeout: 8000 });
  await page.screenshot({ path: `/tmp/miniclaw-ready-${test.info().project.name}.png` });
  await nav.getByRole('button', { name: groupedName, exact: true }).click();
  if (await toggle.isVisible()) await toggle.click();
  await expect(ready(groupedName)).toHaveCount(0);
  await expect(ready(looseName)).toBeVisible();
});
