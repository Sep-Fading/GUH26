// Checks, in a real browser, the Roblox path end to end with the fake MCP server: a screenshot a tool
// returns shows in Activity and opens full size, and that same picture can go with a task handed to
// another agent, shown in the hand-off popup and on the run's card.
import { test, expect, type Page } from '@playwright/test';
// Prepares a folder that holds the fake MCP server.
import { copyFile, mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json, eventually } from '../support/harness.mjs';
// Names the fake conversation, its turn and the delegated run's thread.
import { THREAD, TURN, OTHER, started, turn } from '../support/fixtures.mjs';

const MAIN = '00000000-0000-0000-0000-000000000000';
const post = (app: any, path: string, body: unknown = {}) => app.request(path, { method: 'POST', body });

/** Starts a conversation from the page with a Studio-like server on, and has it take one screenshot. */
async function screenshot(page: Page, folder: string) {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  await copyFile(new URL('../support/fake-mcp.mjs', import.meta.url), join(folder, 'server.mjs'));
  const { server } = await json(await post(app, '/api/mcp/servers', { name: 'studio', kind: 'stdio', command: process.execPath,
    args: [join(folder, 'server.mjs')], folder }), 201);
  await json(await post(app, `/api/permissions/agent/${MAIN}`, { target: { target_type: 'mcp_server', server: server.id }, effect: 'allow' }));
  await json(await post(app, '/api/agents', { name: 'Blender', instructions: 'Model in Blender.', active: true }), 201);
  await page.goto(app.url);
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Take a screenshot');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start'), 'turn start');
  await json(await post(app, `/api/threads/${THREAD}/capabilities`, { key: `mcp:${server.id}`, on: true }));
  await app.emit({ id: 'shot', method: 'item/tool/call', params: { tool: 'miniclaw_mcp', threadId: THREAD, turnId: TURN, callId: 'shot',
    arguments: { action: 'call_tool', server: 'studio', tool: 'snapshot', arguments: {} } } });
  await page.getByRole('button', { name: 'Approve Once', exact: true }).click();
  const link = page.getByRole('link', { name: 'Open studio-snapshot-1.png' });
  await expect(link).toBeVisible();
  const entry = (await json(await app.request(`/api/threads/${THREAD}`))).activities.find((item: any) => item.id === 'shot');
  return { app, link, picture: entry.pictures[0] };
}

test('a screenshot an MCP tool returns shows in Activity and opens full size', async ({ page }, info) => {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-pictures-')));
  const { app, link } = await screenshot(page, folder);
  try {
    await expect.poll(() => link.locator('img').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth)).toBe(1);
    expect(await link.getAttribute('target')).toBe('_blank');
    await page.screenshot({ path: info.outputPath('activity-picture.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  } finally { await app.close(); await rm(folder, { recursive: true, force: true }); }
});

test('that screenshot goes with a task to another agent, shown in the popup and on the run card', async ({ page }, info) => {
  const folder = await realpath(await mkdtemp(join(tmpdir(), 'miniclaw-pictures-')));
  const { app, picture } = await screenshot(page, folder);
  try {
    await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER, sessionId: OTHER }) });
    // The fake announces a started turn on the parent by default; the run's own turn must not look like the parent's.
    await app.set('turn/start', { result: { turn: turn() }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: turn() } }],
      cases: [{ params: { threadId: OTHER }, handler: { result: { turn: turn() } } }] });
    await app.emit({ id: 'hand', method: 'item/tool/call', params: { tool: 'miniclaw_delegate', threadId: THREAD, turnId: TURN, callId: 'hand',
      arguments: { agent: 'Blender', task: 'Match this layout in Blender.', summary: 'Match the layout', attachments: [picture.id] } } });
    const popup = page.getByRole('alertdialog', { name: 'Delegate to Blender?' });
    await expect(popup.getByRole('list', { name: 'Files handed over' }).getByRole('link', { name: 'Open studio-snapshot-1.png' })).toBeVisible();
    await page.screenshot({ path: info.outputPath('handoff-popup.png') });
    await popup.getByRole('button', { name: 'Run in background' }).click();
    const card = page.getByRole('group', { name: 'Delegated to Blender' });
    await expect(card.getByRole('list', { name: 'Files handed over' }).getByRole('img', { name: 'studio-snapshot-1.png' })).toBeVisible();
    // The run's first message carried the picture as a picture.
    const state = await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start' && r.params.threadId === OTHER), 'run start');
    expect(state.requests.find((r: any) => r.method === 'turn/start' && r.params.threadId === OTHER).params.input.map((part: any) => part.type))
      .toEqual(['localImage', 'text']);
    await page.screenshot({ path: info.outputPath('handoff-card.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  } finally { await app.close(); await rm(folder, { recursive: true, force: true }); }
});
