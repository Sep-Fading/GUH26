import { test, expect } from '@playwright/test';
import { launchApp } from '../support/harness.mjs';
import { THREAD, OTHER, thread, turn, message, started } from '../support/fixtures.mjs';

// A 503 ends native EventSource retries; periodic recovery must replace it.
test('closed SSE recovers without navigation or resending', async ({ page }) => {
  const app = await launchApp();
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let streams = 0;
  await page.route('**/events', async route => {
    streams++;
    if (streams === 1) {
      await gate;
      await route.fulfill({ status: 503, contentType: 'application/json', body: '{}' });
    } else { await route.continue(); }
  });
  try {
    await page.goto(app.url);
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(composer).toBeEnabled();
    await composer.fill('Keep this unsent draft');
    await expect.poll(() => streams).toBe(1);
    release();
    await expect.poll(() => streams, { timeout: 20000 }).toBeGreaterThan(1);
    await expect(composer).toBeEnabled();
    await expect(composer).toHaveValue('Keep this unsent draft');
    await expect(page.locator('select[aria-label="Chat"]')).toHaveValue(THREAD);
    expect(await page.evaluate(() => performance.getEntriesByType('navigation').length)).toBe(1);
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  } finally { release(); await page.unrouteAll({ behavior: 'wait' }); await app.close(); }
});

// Holds initial history loading so the startup composer race is reproducible.
test('composer waits for initial chat selection', async ({ page }) => {
  const app = await launchApp();
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let requested = false;
  await page.route('**/api/threads', async route => {
    requested = true;
    await gate;
    await route.continue();
  });
  try {
    await page.goto(app.url);
    await expect.poll(() => requested).toBe(true);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(input).toBeDisabled();
    release();
    await expect(input).toBeEnabled();
    await expect(page.locator('select[aria-label="Chat"]')).toHaveValue(THREAD);
    await input.fill('Keep this draft after startup');
    await expect(input).toHaveValue('Keep this draft after startup');
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
  } finally { release(); await page.unrouteAll({ behavior: 'wait' }); await app.close(); }
});

test('saved history reopens and loads older messages', async ({ page }) => {
  const app = await launchApp();
  try {
    await app.set('thread/turns/list', { result: {
      data: [turn('completed', [message('Saved answer')])], nextCursor: 'older',
    } });
    await page.goto(app.url);
    await expect(page.getByRole('article', { name: 'Assistant message' })).toHaveText('Saved answer');
    await app.set('thread/turns/list', { result: { data: [
      { ...turn('completed', [message('Older answer', 'older-item')]), id: 'older-turn' },
    ], nextCursor: null } });
    await page.getByRole('button', { name: 'Load older messages', exact: true }).click();
    await expect(page.getByRole('article', { name: 'Assistant message' })).toHaveText([
      'Older answer', 'Saved answer',
    ]);
    expect((await app.inspect()).requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
    await page.reload();
    await expect(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    await expect(page.getByRole('article', { name: 'Assistant message' })).toContainText('Saved answer');
  } finally { await app.close(); }
});

test('selected saved chat survives reload', async ({ page, isMobile }) => {
  const app = await launchApp();
  try {
    await app.set('thread/list', { result: { data: [thread(app.peer.cwd),
      thread(app.peer.cwd, { id: OTHER, name: 'Second task' })], nextCursor: null } });
    await page.goto(app.url);
    // Wait for restoration before replacing the shared RPC fixture.
    await expect.poll(async () => (await app.inspect()).requests
      .filter(r => r.method === 'thread/resume').map(r => r.params.threadId))
      .toEqual([THREAD]);
    await expect(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    await app.set('thread/read', { result: { thread: thread(app.peer.cwd, { id: OTHER }) } });
    await app.set('thread/resume', { result: started(app.peer.cwd, { id: OTHER }) });
    if (isMobile) {
      await page.getByRole('combobox', { name: 'Chat', exact: true }).selectOption(OTHER);
    } else {
      await page.getByRole('button', { name: 'Second task', exact: true }).click();
    }
    await expect(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    await page.reload();
    await expect(page.locator('select[aria-label="Chat"]')).toHaveValue(OTHER);
    expect((await app.inspect()).requests.filter(r => r.method === 'thread/resume')
      .map(r => r.params.threadId)).toEqual([THREAD, OTHER]);
  } finally { await app.close(); }
});
