// Exercises visible controls in real browsers.
import { test, expect } from '@playwright/test';
// Starts isolated backends with controlled replies.
import { launchApp, eventually } from '../support/harness.mjs';
// Supplies consistent task and notification fixtures.
import { THREAD, TURN, turn, turnDone, message } from '../support/fixtures.mjs';
// Opens the composer's Tools dropdown.
import { openTools } from './settings.js';

// The fake Codex peer never executes this helper.
process.env.MINICLAW_HELPER_BIN = '/usr/bin/true';

for (const action of ['accept', 'decline']) {
  test(`computer approval: ${action}`, async ({ page }, info) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    try {
      await page.goto(app.url);
      await (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }).click();
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Use Calculator');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
      await app.emit({ id: 'computer-approval', method: 'mcpServer/elicitation/request',
        params: { threadId: THREAD, turnId: TURN, serverName: 'miniclaw_computer',
          mode: 'form', message: 'Allow Calculator? ' + 'Long approval detail. '.repeat(150),
          requestedSchema: { type: 'object', properties: {} } } });
      const approval = page.getByRole('region', { name: 'Computer approval', exact: true });
      await expect(approval).toBeVisible();
      await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeInViewport();
      const button = approval.getByRole('button', {
        name: action === 'accept' ? 'Allow once' : 'Decline', exact: true,
      });
      await expect(button).toBeInViewport();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath('approval.png'), fullPage: true });
      await button.click();
      const state = await eventually(() => app.inspect(), value => value.responses.length > 0);
      expect(state.responses.find(reply => reply.id === 'computer-approval')?.result)
        .toEqual({ action, content: action === 'accept' ? {} : null });
      await expect(approval).toHaveCount(0);
    } finally { await app.close(); }
  });
}

// Keeps running status until Codex confirms interruption, then clears it live.
test('Stop clears unfinished computer activity', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    await (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Use Calculator');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await app.emit({ method: 'item/started', params: { threadId: THREAD, turnId: TURN,
      item: { id: 'pending-tool', type: 'mcpToolCall', server: 'miniclaw_computer',
        tool: 'get_app_state', arguments: { app: 'Calculator' }, status: 'inProgress' } } });
    await page.getByText('Computer activity', { exact: true }).click();
    await expect(page.getByText('Running', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await eventually(() => app.inspect(), state =>
      state.requests.some(request => request.method === 'turn/interrupt'));
    await expect(page.getByText('Running', { exact: true })).toBeVisible();
    await app.emit(turnDone('interrupted'));
    await expect(page.getByText('interrupted', { exact: true })).toBeVisible();
    await expect(page.getByText('Running', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  } finally { await app.close(); }
});

// Verifies a late click reports expiry without approving or resending work.
test('expired approval shows its actual error', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    await (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Use Calculator');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await app.emit({ id: 'expired-approval', method: 'mcpServer/elicitation/request',
      params: { threadId: THREAD, turnId: TURN, serverName: 'miniclaw_computer',
        mode: 'form', message: 'Allow Calculator?',
        requestedSchema: { type: 'object', properties: {} } } });
    // Finish the turn before forwarding the user's already-submitted decision.
    await page.route('**/api/threads/*/approvals/*', async route => {
      await app.emit(turnDone());
      await eventually(() => app.inspect(), state =>
        state.responses.some(reply => reply.id === 'expired-approval'));
      await route.continue();
    });
    await page.getByRole('button', { name: 'Allow once', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText(
      'This approval is no longer active. Retry the request to get a new prompt.');
    await expect(page.getByRole('region', { name: 'Computer approval', exact: true })).toHaveCount(0);
    const state = await app.inspect();
    expect(state.responses.find(reply => reply.id === 'expired-approval')?.result.action)
      .toBe('decline');
    expect(state.requests.filter(request => request.method === 'turn/start')).toHaveLength(1);
  } finally { await app.close(); }
});

test('computer toggle preserves the conversation without a page reload', async ({ page }, info) => {
  const text = 'A saved answer stays here.\n'.repeat(90);
  const app = await launchApp({
    'thread/turns/list': { result: { data: [turn('completed', [message(text)])], nextCursor: null } },
    'thread/archive': { result: {} }, 'thread/unarchive': { hold: true },
  });
  try {
    expect((await app.request(`/api/threads/${THREAD}/environment`, {
      method: 'POST', body: { kind: 'host' },
    })).status).toBe(200);
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    const toggle = (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true });
    const conversation = page.getByRole('region', { name: 'Conversation', exact: true });
    await expect(toggle).toBeEnabled();
    await expect(conversation.getByRole('article')).toHaveText(text);
    await input.fill('Keep this draft');
    await conversation.evaluate(element => { element.scrollTop = 120; });
    const original = await conversation.elementHandle();
    let navigations = 0;
    page.on('framenavigated', frame => { if (frame === page.mainFrame()) navigations++; });
    for (const enabled of [true, false]) {
      await app.set('thread/unarchive', { hold: true });
      await toggle.click();
      const state = await eventually(() => app.inspect(), value => value.held.length > 0);
      await expect(input).toBeEnabled();
      await expect(input).toHaveValue('Keep this draft');
      await expect(page.getByText('Responding...', { exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      await app.peer.call({ op: 'release', id: state.held[0].id,
        reply: { result: { thread: { id: THREAD } } } });
      await expect(toggle).toHaveAttribute('aria-checked', String(enabled));
      await expect(toggle).toBeEnabled();
      expect(await conversation.evaluate(element => element.scrollTop)).toBe(120);
      expect(await original!.evaluate(element => element.isConnected)).toBe(true);
      await expect(input).toHaveValue('Keep this draft');
      await page.screenshot({ path: info.outputPath(`computer-${enabled}.png`), fullPage: true });
    }
    expect(navigations).toBe(0);
    const log = await app.inspect();
    expect(log.requests.filter(r => r.method === 'thread/start')).toHaveLength(0);
    expect(log.requests.filter(r => r.method === 'turn/start')).toHaveLength(0);
    await page.setViewportSize({ width: 320, height: 740 });
    // Requirement change 2026-09-22: Speed now follows effort, so Tools stays beside
    // that last choice on a narrow screen (PLAN.md, Codex service tiers).
    const trigger = page.getByRole('button', { name: 'Tools', exact: true });
    await expect(trigger).toBeInViewport();
    const speed = await page.getByRole('combobox', { name: 'Speed', exact: true }).boundingBox();
    const button = await trigger.boundingBox();
    expect(button!.x - speed!.x - speed!.width).toBeLessThanOrEqual(8);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('computer-narrow.png'), fullPage: true });
  } finally { await app.close(); }
});

test('failed computer switch shows an inline error and can retry', async ({ page }) => {
  const app = await launchApp({
    'thread/archive': { result: {} },
    'thread/unarchive': { error: { code: -32000, message: 'Fixture restore failure' } },
  });
  try {
    expect((await app.request(`/api/threads/${THREAD}/environment`, {
      method: 'POST', body: { kind: 'host' },
    })).status).toBe(200);
    await page.goto(app.url);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await input.fill('A draft worth keeping');
    await (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Could not restore');
    await expect(input).toHaveValue('A draft worth keeping');
    await expect(input).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
    await app.set('thread/unarchive', { result: { thread: { id: THREAD } } });
    // The dropdown is closed before reaching for a control behind it.
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Retry Computer Use', exact: true }).click();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect((await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }))
      .toHaveAttribute('aria-checked', 'true');
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
  } finally { await app.close(); }
});

test('a failed step opens Computer activity with the reason and what to do', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    await (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true }).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Look at Roblox');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await app.emit({ method: 'item/completed', params: { threadId: THREAD, turnId: TURN,
      item: { id: 'gone', type: 'mcpToolCall', server: 'miniclaw_computer', tool: 'list_apps', arguments: {}, status: 'failed',
        result: { isError: true, content: [{ type: 'text', text: 'Computer Use server error -10005: codex app-server exited before returning a response' }] } } } });
    // Nobody has to open the list to learn why it failed.
    await expect(page.getByText(/-10005/)).toBeVisible();
    await expect(page.getByText(/open the ChatGPT desktop app once/)).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); }
});
