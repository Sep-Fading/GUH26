// Checks the message queue in a real browser: the composer stays open while a response runs,
// queued messages are shown and can be changed, and a paused queue says why.
import { test, expect, type Page } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, eventually, json } from '../support/harness.mjs';
// Names the fake conversation and ends its turn.
import { THREAD, TURN, itemDone, turnDone, message } from '../support/fixtures.mjs';
// Finds the new conversation button in the sidebar, opening the drawer on a narrow window.
import { newChatButton } from './settings.js';

type App = Awaited<ReturnType<typeof launchApp>>;

/** Opens a new conversation and sends its first message, whose response keeps running. */
async function responding(page: Page, app: App) {
  await page.goto(app.url);
  await (await newChatButton(page)).click();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('First');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/start'), 'first turn');
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
}

/** Makes the fake answer the next turn under its own id. */
async function nextTurn(app: App, id: string) {
  const turn = { id, status: 'inProgress', items: [] };
  await app.set('turn/start', { result: { turn }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn } }] });
}

test('messages typed during a response are queued, shown, and sent when it ends', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await responding(page, app);
    const box = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(box).toBeEnabled();
    await box.fill('Second');
    // Stop stays within reach while the draft turns the main button into Queue.
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Queue message', exact: true }).click();
    await expect(box).toHaveValue('');
    await box.fill('Third');
    await box.press('Enter');
    const queue = page.getByRole('region', { name: 'Queued messages' });
    await expect(queue.getByRole('listitem')).toHaveText([/Second/, /Third/]);
    await expect(queue).toContainText('2 queued');
    await page.screenshot({ path: info.outputPath('queue.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    await nextTurn(app, 'turn-2');
    await app.emit(itemDone('Answer one'), turnDone());
    await expect(queue.getByRole('listitem')).toHaveText([/Third/]);
    const conversation = page.getByRole('region', { name: 'Conversation', exact: true });
    await expect(conversation.getByRole('article', { name: 'Your message', exact: true })).toHaveText(['First', 'Second']);
  } finally { await app.close(); }
});

test('a queued message is edited, removed and sent now', async ({ page }) => {
  const app = await launchApp();
  try {
    await responding(page, app);
    const box = page.getByRole('textbox', { name: 'Message', exact: true });
    for (const text of ['Draft', 'Drop me', 'Urgent']) { await box.fill(text); await box.press('Enter'); }
    const queue = page.getByRole('region', { name: 'Queued messages' });
    await expect(queue.getByRole('listitem')).toHaveCount(3);

    const first = queue.getByRole('listitem').nth(0);
    await first.getByRole('button', { name: 'Edit queued message' }).click();
    const editor = first.getByRole('textbox', { name: 'Queued message' });
    await editor.fill('Final words');
    await first.getByRole('button', { name: 'Save' }).click();
    await expect(first).toContainText('Final words');

    await queue.getByRole('listitem').nth(1).getByRole('button', { name: 'Remove queued message' }).click();
    await expect(queue.getByRole('listitem')).toHaveText([/Final words/, /Urgent/]);

    await nextTurn(app, 'turn-2');
    await queue.getByRole('listitem').nth(1).getByRole('button', { name: 'Send now' }).click();
    await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
    await app.emit(turnDone('interrupted'));
    await expect(queue.getByRole('listitem')).toHaveText([/Final words/]);
    await eventually(() => app.inspect(), state => state.requests.filter(request => request.method === 'turn/start')
      .some(request => request.params.input[0].text === 'Urgent'), 'urgent sent');
  } finally { await app.close(); }
});

test('a failed response pauses the queue, says why, and Resume sends on; Stop closes the box', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await responding(page, app);
    const box = page.getByRole('textbox', { name: 'Message', exact: true });
    await box.fill('After the failure');
    await box.press('Enter');
    await nextTurn(app, 'turn-2');
    await app.emit(turnDone('failed'));
    const queue = page.getByRole('region', { name: 'Queued messages' });
    await expect(queue).toContainText('Paused: the last response failed.');
    await page.screenshot({ path: info.outputPath('queue-paused.png') });
    expect((await app.inspect()).requests.filter(request => request.method === 'turn/start')).toHaveLength(1);
    await queue.getByRole('button', { name: 'Resume queue' }).click();
    await eventually(() => app.inspect(), state => state.requests.filter(request => request.method === 'turn/start').length === 2, 'resumed');
    await expect(queue).toHaveCount(0);

    // Once Stop is pressed, nothing more is queued until the response has really ended.
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await expect(box).toBeDisabled();
    // A queue that came back from a restart is shown paused, from the saved conversation.
    expect((await json(await app.request(`/api/threads/${THREAD}`))).queue.items).toEqual([]);
  } finally { await app.close(); }
});

// User report 2026-10-10: after a cancelled turn or a message pushed with "Send now", that turn's Activity
// stayed at the bottom and newer messages appeared above it. It stays with its own turn, live and after reload.
test('"Send now" during a step keeps the stopped turn\'s Activity above the message sent now, live and after reload', async ({ page }, info) => {
  const app = await launchApp();
  try {
    await responding(page, app);
    await app.emit({ id: 'ask-1', method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD, turnId: TURN,
      callId: 'ask-1', arguments: {} } });
    await expect(page.getByRole('group', { name: 'Activity' })).toBeVisible();
    const box = page.getByRole('textbox', { name: 'Message', exact: true });
    await box.fill('Urgent');
    await page.getByRole('button', { name: 'Queue message', exact: true }).click();
    await nextTurn(app, 'turn-2');
    await page.getByRole('region', { name: 'Queued messages' }).getByRole('button', { name: 'Send now' }).click();
    await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
    await app.emit(turnDone('interrupted'));
    await eventually(() => app.inspect(), state => state.requests.filter(request => request.method === 'turn/start').length === 2, 'urgent sent');
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: 'turn-2', item: message('Urgent answer', 'assistant-2') } },
      { method: 'turn/completed', params: { threadId: THREAD, turn: { id: 'turn-2', status: 'completed', items: [] } } });
    // Top to bottom: messages and the conversation's own Activity lists.
    const order = () => page.locator('[data-conversation-content]').evaluate(content =>
      [...content.querySelectorAll('article, [role="group"]')].map(node => node.getAttribute('aria-label') === 'Activity'
        ? 'Activity' : `${node.getAttribute('aria-label')}: ${node.textContent?.trim()}`));
    const expected = ['Your message: First', 'Activity', 'Your message: Urgent', 'Assistant message: Urgent answer'];
    await expect.poll(order).toEqual(expected);
    await page.screenshot({ path: info.outputPath('send-now-order.png') });
    await page.reload();
    await expect(page.getByRole('article', { name: 'Assistant message', exact: true })).toHaveText('Urgent answer');
    await expect.poll(order).toEqual(expected);
  } finally { await app.close(); }
});
