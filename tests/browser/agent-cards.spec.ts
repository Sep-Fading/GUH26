// Checks, in a real browser, the Agents tab of the workspace: a card for each delegated agent with its
// name, the summary its request gave, and a live line that follows what it is doing; and that the
// parent carries on by itself when a background run ends, shown as MiniClaw's notice, not as the user's words.
import { test, expect, type Page } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json, eventually } from '../support/harness.mjs';
// Names the fake conversation, its turn and the delegated run's thread.
import { THREAD, TURN, OTHER, started, delta, message, turn } from '../support/fixtures.mjs';

const post = (app: any, path: string, body: unknown = {}) => app.request(path, { method: 'POST', body });

/** Opens the workspace's Agents tab at either width and returns the tab's panel. */
async function agentsTab(page: Page) {
  const rail = page.getByRole('button', { name: 'Open Agents', exact: true });
  if (await rail.isVisible()) await rail.click();
  else {
    await page.getByRole('button', { name: 'Open workspace', exact: true }).click();
    await page.getByRole('tab', { name: /^Agents/ }).click();
  }
  return page.getByRole('tabpanel');
}

test('a delegated agent gets a card with its name, summary and live line, and the parent carries on when it ends', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await json(await post(app, '/api/agents', { name: 'Blender', instructions: 'Model in Blender.', active: true }), 201);
    await page.goto(app.url);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Make the tiles');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start'), 'turn start');
    await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
    // The fake announces a started turn on the parent by default; the run's own turn must not look like the parent's.
    await app.set('turn/start', { result: { turn: turn() }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: turn() } }],
      cases: [{ params: { threadId: OTHER }, handler: { result: { turn: turn() } } }] });
    await app.emit({ id: 'hand', method: 'item/tool/call', params: { tool: 'miniclaw_delegate', threadId: THREAD, turnId: TURN, callId: 'hand',
      arguments: { agent: 'Blender', summary: 'Build the wasteland tile set', task: 'Build six low-poly wasteland tiles and export them as tiles.glb.' } } });
    const popup = page.getByRole('alertdialog', { name: 'Delegate to Blender?' });
    await expect(popup).toContainText('Build the wasteland tile set');
    await popup.getByRole('button', { name: 'Run in background' }).click();
    await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');

    const panel = await agentsTab(page);
    const card = panel.getByRole('article', { name: 'Blender: Build the wasteland tile set' });
    await expect(card).toContainText('Blender');
    await expect(card).toContainText('Running in the background');
    const line = card.getByRole('status');
    await expect(line).toHaveText('Starting…');
    // The line follows the agent: first what it writes, then its answer once it ends.
    await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } });
    await app.emit(delta('Opening the scene.\nModelling the first tile', 'child-words', OTHER));
    await expect(line).toHaveText('Modelling the first tile');
    await page.screenshot({ path: info.outputPath('agent-card-running.png') });

    // The parent's own response ends; then the run does, and the parent carries on by itself.
    await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
    await app.emit({ method: 'item/completed', params: { threadId: OTHER, turnId: TURN, completedAtMs: 1900000000000, item: message('Six tiles exported to tiles.glb.', 'child-words') } },
      { method: 'turn/completed', params: { threadId: OTHER, turn: { id: TURN, status: 'completed', items: [] } } });
    await expect(card).toContainText('Finished');
    await expect(line).toHaveText('Six tiles exported to tiles.glb.');
    const notice = page.getByRole('note').filter({ hasText: 'Blender finished "Build the wasteland tile set"' });
    await expect(notice).toBeVisible();
    await expect(page.getByRole('article', { name: 'Your message' })).toHaveCount(1);
    await expect(notice).not.toContainText('[MiniClaw notice]');
    await page.screenshot({ path: info.outputPath('agent-card-done.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  } finally { await app.close(); }
});

// Requirement 2026-10-11 (lead decision): a run the user waits for that is still going at the hour, with a longer
// limit, continues in the background. Its block and its card show the change; its answer comes back by itself.
// A test minute lasts 20 ms here, so the hour passes in 1.2 s.
test('a waited run still going at the hour continues in the background, and its block and card say so', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } }, { env: { MINICLAW_TEST_MINUTE_MS: '20' } });
  try {
    await json(await post(app, '/api/agents', { name: 'Blender', instructions: 'Model in Blender.', active: true }), 201);
    await json(await post(app, '/api/main-agent', { model: null, effort: null, delegation: { maxPerTurn: 3, minutes: 600 } }));
    await page.goto(app.url);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Make the tiles');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start'), 'turn start');
    await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
    await app.set('turn/start', { result: { turn: turn() }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: turn() } }],
      cases: [{ params: { threadId: OTHER }, handler: { result: { turn: turn() } } }] });
    await app.emit({ id: 'hand', method: 'item/tool/call', params: { tool: 'miniclaw_delegate', threadId: THREAD, turnId: TURN, callId: 'hand',
      arguments: { agent: 'Blender', summary: 'Build the wasteland tile set', task: 'Build six low-poly wasteland tiles.' } } });
    const popup = page.getByRole('alertdialog', { name: 'Delegate to Blender?' });
    await expect(popup).toContainText('If you wait, after 1 h it continues in the background.');
    await popup.getByRole('button', { name: 'Wait for it' }).click();
    const block = page.getByRole('group', { name: 'Delegated to Blender' });
    await expect(block).toContainText('Running, the conversation waits');
    // At the (test) hour the parent's call returns and the run carries on.
    await expect(block).toContainText('Continues in the background after the 1 h wait', { timeout: 10_000 });
    const handed = (await app.inspect()).responses.find((r: any) => r.id === 'hand');
    expect(handed?.result.success).toBe(true);
    expect(handed?.result.contentItems[0].text).toMatch(/continues in the background/);
    const panel = await agentsTab(page);
    const card = panel.getByRole('article', { name: 'Blender: Build the wasteland tile set' });
    await expect(card).toContainText('Continues in the background after the 1 h wait');
    await page.screenshot({ path: info.outputPath('agent-card-handed-over.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);

    // The parent's response ends, then the run does: the card shows its answer and the parent carries on by itself.
    await app.emit({ method: 'turn/completed', params: { threadId: THREAD, turn: { id: TURN, status: 'completed', items: [] } } });
    await app.emit({ method: 'turn/started', params: { threadId: OTHER, turn: { id: TURN, status: 'inProgress', items: [] } } },
      { method: 'item/completed', params: { threadId: OTHER, turnId: TURN, completedAtMs: 1900000000000, item: message('Six tiles exported.', 'child-words') } },
      { method: 'turn/completed', params: { threadId: OTHER, turn: { id: TURN, status: 'completed', items: [] } } });
    await expect(card).toContainText('Finished');
    await expect(card.getByRole('status')).toHaveText('Six tiles exported.');
    await expect(page.getByRole('note').filter({ hasText: 'Blender finished "Build the wasteland tile set"' })).toBeVisible();
  } finally { await app.close(); }
});
