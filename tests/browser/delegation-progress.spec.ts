// Verifies live delegated messages and opening the same running conversation.
import { test, expect } from '@playwright/test';
// Runs disposable backends with observable provider requests.
import { launchApp, json, eventually } from '../support/harness.mjs';
// Supplies parent and child provider events.
import { THREAD, OTHER, TURN, CLIENT, started, thread, message } from '../support/fixtures.mjs';

test('follow a live run and open its conversation without restarting it', async ({ page }, info) => {
  const app = await launchApp();
  const post = (path: string, body: unknown = {}) => app.request(path, { method: 'POST', body });
  try {
    await json(await post('/api/agents', { name: 'Reviewer', instructions: 'Review.', active: true }), 201);
    await json(await post('/api/threads', { environment: { kind: 'host' } }), 201);
    await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
    await page.goto(app.url);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Review notes');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await app.emit({ id: 'delegate-live', method: 'item/tool/call', params: { tool: 'miniclaw_delegate',
      threadId: THREAD, turnId: TURN, callId: 'delegate-live', arguments: { agent: 'Reviewer', task: 'Check release notes.', summary: 'Check the release notes' } } });
    await page.getByRole('alertdialog', { name: 'Delegate to Reviewer?' }).getByRole('button', { name: 'Run in background' }).click();
    await eventually(() => app.inspect(), value => value.requests.some(r => r.method === 'turn/start' && r.params.threadId === OTHER));
    const card = page.getByRole('group', { name: 'Delegated to Reviewer' });
    await card.getByRole('button', { name: 'Show the full run' }).click();
    await app.emit({ method: 'item/agentMessage/delta', params: { threadId: OTHER, turnId: TURN,
      itemId: 'live-progress', delta: 'Checking the headings.' } });
    await expect(card).toContainText('Checking the headings.');
    await app.emit({ method: 'item/agentMessage/delta', params: { threadId: OTHER, turnId: TURN,
      itemId: 'live-progress', delta: ' Now checking examples.' } });
    await expect(card).toContainText('Now checking examples.');
    await page.screenshot({ path: info.outputPath('delegation-live.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await app.set('thread/list', { result: { data: [thread(app.peer.cwd), thread(app.peer.cwd, { id: OTHER, sessionId: OTHER })], nextCursor: null } });
    await card.getByRole('button', { name: 'Open conversation with Reviewer' }).click();
    await expect(page.getByRole('combobox', { name: 'Agent', exact: true })).toContainText('Reviewer');
    await expect(page.getByRole('article', { name: 'Assistant message' })).toContainText('Now checking examples.');
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    expect((await app.inspect()).requests.filter(r => r.method === 'thread/start').length).toBe(2);
    await app.emit({ method: 'item/completed', params: { threadId: OTHER, turnId: TURN, item: message('Review finished.', 'live-progress') } },
      { method: 'turn/completed', params: { threadId: OTHER, turn: { id: TURN, status: 'completed', items: [] } } });
    await expect(page.getByRole('article', { name: 'Assistant message' })).toContainText('Review finished.');
  } finally { await app.close(); }
});
