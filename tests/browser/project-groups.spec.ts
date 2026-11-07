// Checks agent sections and changing conversation order.
import { test, expect } from '@playwright/test';
// Runs the page against an isolated backend.
import { launchApp, json } from '../support/harness.mjs';
// Supplies valid synthetic provider conversation records.
import { THREAD, OTHER, thread } from '../support/fixtures.mjs';

test('project groups sort chats by recent activity', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { project } = await json(await app.request('/api/projects', { method: 'POST',
      body: { name: 'Game', environment: { kind: 'host' } } }), 201);
    const third = '019a5555-5555-7555-8555-555555555555';
    const fourth = '019a6666-6666-7666-8666-666666666666';
    const tasks = [
      { id: THREAD, name: 'Older editor chat', agentId: 'editor', agentName: 'Editor', updatedAt: 100 },
      { id: OTHER, name: 'Recent editor chat', agentId: 'editor', agentName: 'Editor', updatedAt: 300 },
      { id: third, name: 'Build chat', agentId: 'builder', agentName: 'Builder', updatedAt: 200 },
      { id: fourth, name: 'Main chat', updatedAt: 150 },
    ].map(t => ({ ...thread(app.peer.cwd), ...t, projectId: project.id, createdAt: 50 }));
    await page.route('**/api/threads', route => route.fulfill({ json: { threads: tasks, nextCursor: null } }));
    const snapshot = { thread: tasks[0], messages: [], activeTurn: null, pending: false };
    await page.route(`**/api/threads/${THREAD}`, route => route.fulfill({ json: snapshot }));
    await page.route(`**/api/threads/${THREAD}/events`, route => route.fulfill({
      contentType: 'text/event-stream', body: `event: snapshot\ndata: ${JSON.stringify(snapshot)}\n\n`,
    }));
    let updatedAt = 100;
    await page.route('**/api/threads/working', route => route.fulfill({ json: {
      threads: [], activity: { [THREAD]: updatedAt },
    } }));
    await page.goto(app.url);
    const open = page.getByRole('button', { name: 'Open navigation', exact: true });
    if (await open.isVisible()) await open.click();
    const nav = page.getByRole('navigation', { name: 'Tasks', exact: true });
    await expect(nav.getByRole('heading', { level: 3 })).toHaveText(['Builder', 'Editor', 'Main agent']);
    const editor = nav.getByRole('group', { name: 'Editor conversations', exact: true });
    // Chat controls keep their names; reading text by order ignores action buttons.
    const order = () => editor.locator('button').evaluateAll(buttons => buttons
      .map(b => b.querySelector('.rest')?.textContent?.trim())
      .filter(name => name === 'Older editor chat' || name === 'Recent editor chat'));
    await expect.poll(order).toEqual(['Recent editor chat', 'Older editor chat']);
    updatedAt = 400;
    await expect.poll(order, { timeout: 6000 }).toEqual(['Older editor chat', 'Recent editor chat']);
    await expect(nav.getByRole('group', { name: 'Builder conversations', exact: true })
      .getByRole('button', { name: 'Build chat', exact: true })).toBeVisible();
    const editorToggle = editor.getByRole('button', { name: 'Editor', exact: true });
    await expect(editorToggle).toBeVisible();
    await editorToggle.click();
    await expect(editorToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(editor.getByRole('button', { name: 'Older editor chat', exact: true })).toBeHidden();
    await expect(nav.getByRole('button', { name: 'Build chat', exact: true })).toBeVisible();
    await expect(nav.getByRole('button', { name: 'Main chat', exact: true })).toBeVisible();
    await page.reload();
    if (await open.isVisible()) await open.click();
    await expect(editorToggle).toHaveAttribute('aria-expanded', 'false');
    await editorToggle.focus();
    await page.keyboard.press('Enter');
    await expect(editorToggle).toHaveAttribute('aria-expanded', 'true');
    await expect(editor.getByRole('button', { name: 'Older editor chat', exact: true })).toBeVisible();
    const mainHandle = nav.getByRole('button', { name: 'Reorder Main agent', exact: true });
    await expect(mainHandle).toBeVisible();
    const from = (await mainHandle.boundingBox())!;
    const to = (await nav.getByRole('button', { name: 'Builder', exact: true }).boundingBox())!;
    if (info.project.name === 'mobile') {
      const touch = await page.context().newCDPSession(page);
      await touch.send('Input.dispatchTouchEvent', { type: 'touchStart',
        touchPoints: [{ x: from.x + from.width / 2, y: from.y + from.height / 2 }] });
      await touch.send('Input.dispatchTouchEvent', { type: 'touchMove',
        touchPoints: [{ x: to.x + 10, y: to.y + 2 }] });
      await touch.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
      await touch.detach();
    } else {
      await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
      await page.mouse.down();
      await page.mouse.move(to.x + 10, to.y + 2, { steps: 10 });
      await page.mouse.up();
    }
    await expect(nav.getByRole('heading', { level: 3 })).toHaveText(['Main agent', 'Builder', 'Editor']);
    await page.reload();
    if (await open.isVisible()) await open.click();
    await expect(nav.getByRole('heading', { level: 3 })).toHaveText(['Main agent', 'Builder', 'Editor']);
    await mainHandle.focus();
    await page.keyboard.press('ArrowDown');
    await expect(nav.getByRole('heading', { level: 3 })).toHaveText(['Builder', 'Main agent', 'Editor']);
    await page.screenshot({ path: info.outputPath('agent-groups.png') });
  } finally { await app.close(); }
});
