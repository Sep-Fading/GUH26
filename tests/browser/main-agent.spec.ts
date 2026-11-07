// Checks agent configuration in Settings: the main agent's defaults and capabilities, and a
// named agent's Capabilities section in place of the old read-only list.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json, eventually } from '../support/harness.mjs';
// Names the fake conversation, its turn and the delegated run's thread.
import { THREAD, TURN } from '../support/fixtures.mjs';
// Opens Settings and the composer's Tools dropdown.
import { openSettings, openTools } from './settings.js';

const MAIN = '00000000-0000-0000-0000-000000000000';

test('the main agent is configured in Settings: a default model and what plain conversations may use', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await json(await app.request('/api/mcp/servers', { method: 'POST', body: { name: 'notes', kind: 'stdio', command: '/usr/bin/true' } }), 201);
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Main agent', exact: true }).click();
    await expect(dialog.getByText('The agent of every conversation that has no other agent.')).toBeVisible();
    // It has capabilities and a default model, and none of a named agent's other fields or rules.
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Save permission' })).toHaveCount(0);
    await dialog.getByRole('combobox', { name: 'Default model' }).click();
    await page.getByRole('option', { name: 'Model A' }).click();
    await expect.poll(async () => (await json(await app.request('/api/main-agent'))).model).toBe('fixture-model');

    const capabilities = dialog.getByRole('group', { name: 'Capabilities' });
    await expect(capabilities.getByRole('switch', { name: 'Web search', exact: true })).toBeChecked();
    await capabilities.getByRole('switch', { name: 'Network for commands' }).click();
    await capabilities.getByRole('switch', { name: 'MCP server notes' }).click();
    await expect.poll(async () => (await json(await app.request(`/api/permissions/agent/${MAIN}`))).rules.length).toBe(2);
    await expect(capabilities).toContainText('Allowed for the main agent');
    await page.screenshot({ path: info.outputPath('main-agent.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    // What was allowed here is what a plain conversation's Tools dropdown offers.
    await page.keyboard.press('Escape');
    const tools = await openTools(page);
    // Requirement change 2026-09-22: allowed here means available there, off until the conversation switches it on.
    for (const name of ['Network for commands', 'notes']) {
      await expect(tools.getByRole('switch', { name, exact: true })).not.toBeChecked();
      // Requirement change: tools can be chosen before the first message.
      await expect(tools.getByRole('switch', { name, exact: true })).toBeEnabled();
    }
  } finally { await app.close(); }
});

test('a named agent has a Capabilities section, and the old read-only list is gone', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const { agent } = await json(await app.request('/api/agents', { method: 'POST', body: { name: 'Scout', instructions: 'Look things up.', active: true } }), 201);
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Scout', exact: true }).click();
    await expect(dialog.getByText('Application access')).toHaveCount(0);
    await expect(dialog.getByText('Tool network access')).toHaveCount(0);
    const capabilities = dialog.getByRole('group', { name: 'Capabilities' });
    await capabilities.getByRole('switch', { name: 'Web search', exact: true }).click();
    await expect.poll(async () => (await json(await app.request(`/api/permissions/agent/${agent.id}`))).rules
      .some((rule: { target: { name?: string } }) => rule.target.name === 'no_web_search')).toBe(true);
    await expect(capabilities).toContainText('Not allowed for this agent');
    // A named agent still has its resource rules.
    await expect(dialog.getByRole('button', { name: 'Save permission' })).toBeVisible();
  } finally { await app.close(); }
});

test('a conversation\'s permissions dialog keeps its rules and sends switching to Tools', async ({ page }) => {
  const app = await launchApp();
  try {
    await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
    await page.goto(app.url);
    await page.getByRole('button', { name: 'Conversation permissions' }).click();
    const settings = page.getByRole('dialog', { name: 'Conversation permissions' });
    await expect(settings.getByText('Capabilities are switched from Tools, beside the message box.')).toBeVisible();
    await expect(settings.getByRole('switch')).toHaveCount(0);
  } finally { await app.close(); }
});

test('skills and skill groups are ticked on the main agent and saved at once', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const post = async (path: string, body: unknown) => json(await app.request(path, { method: 'POST', body }), 201);
    const { skill } = await post('/api/skills', { name: 'British English', instructions: 'Write British English.', active: true });
    const { group } = await post('/api/skill-groups', { name: 'Universal', active: true, members: [{ skillId: skill.id, enabled: true }] });
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Main agent', exact: true }).click();
    await expect(dialog.getByText('Used in every conversation that has no other agent')).toBeVisible();
    await dialog.getByRole('group', { name: 'Always-on skill groups' }).getByRole('checkbox', { name: 'Universal' }).check();
    await dialog.getByRole('group', { name: 'Always-on skills' }).getByRole('checkbox', { name: 'British English' }).check();
    await expect.poll(async () => json(await app.request('/api/main-agent'))).toMatchObject({ skillIds: [skill.id], skillGroupIds: [group.id] });
    // Choosing a model afterwards leaves the ticks as they are.
    await dialog.getByRole('combobox', { name: 'Default model' }).click();
    await page.getByRole('option', { name: 'Model A' }).click();
    await expect.poll(async () => json(await app.request('/api/main-agent'))).toMatchObject({ model: 'fixture-model', skillIds: [skill.id], skillGroupIds: [group.id] });
    await expect(dialog.getByRole('checkbox', { name: 'Universal' })).toBeChecked();
  } finally { await app.close(); }
});

test('agent full access default appears in Tools and can be overridden before sending', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await json(await app.request('/api/agents', { method: 'POST', body: {
      name: 'Builder', instructions: 'Build.', active: true } }), 201);
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Builder', exact: true }).click();
    const setting = dialog.getByRole('switch', { name: 'Full access by default', exact: true });
    await expect(setting).not.toBeChecked();
    await setting.click();
    await expect(setting).toBeEnabled();
    await page.screenshot({ path: info.outputPath('agent-full-access.png') });
    await page.keyboard.press('Escape');
    await page.getByRole('combobox', { name: 'Agent', exact: true }).click();
    await page.getByRole('option', { name: 'Builder', exact: true }).click();
    let tools = await openTools(page);
    const full = tools.getByRole('switch', { name: 'Full access', exact: true });
    await expect(full).toBeChecked();
    await full.click();
    await expect(full).not.toBeChecked();
    await page.keyboard.press('Escape');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Work.');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    tools = await openTools(page);
    await expect(tools.getByRole('switch', { name: 'Full access', exact: true })).not.toBeChecked();
    await page.keyboard.press('Escape');
    await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Builder', exact: true }).click();
    await expect(setting).toBeChecked();
    await dialog.getByRole('button', { name: 'Main agent', exact: true }).click();
    await expect(setting).not.toBeChecked();
    await setting.click();
    await expect(setting).toBeEnabled();
    await expect.poll(async () => (await json(await app.request(`/api/permissions/agent/${MAIN}`))).rules
      .some((rule: { target: { name?: string } }) => rule.target.name === 'default_full_access')).toBe(true);
  } finally { await app.close(); }
});

// Requirement 2026-10-11 (user decision): "add the ability to configure delegation settings on main agent so we are
// not locked into a value for the time limit". The same controls as a named agent, up to 1440 minutes.
test("the main agent's delegation limits are set in Settings, and a hand-off says how long it may take", async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await json(await app.request('/api/agents', { method: 'POST', body: { name: 'Blender', instructions: 'Model in Blender.', active: true } }), 201);
    await page.goto(app.url);
    const dialog = await openSettings(page, 'Agents');
    await dialog.getByRole('button', { name: 'Main agent', exact: true }).click();
    const minutes = dialog.getByRole('spinbutton', { name: 'Time limit per run' });
    await expect(minutes).toHaveValue('10');
    await expect(minutes).toHaveAttribute('max', '1440');
    await minutes.fill('1441');
    await minutes.press('Enter');
    await expect(dialog.getByRole('alert')).toHaveText('Enter a whole number of minutes from 1 to 1440.');
    await minutes.fill('120');
    await minutes.press('Enter');
    await expect.poll(async () => (await json(await app.request('/api/main-agent'))).delegation).toEqual({ maxPerTurn: 3, minutes: 120 });
    await expect(dialog.getByText('2 h', { exact: true })).toBeVisible();
    await expect(dialog.getByText(/the conversation waits for at most 1 h/)).toBeVisible();
    await dialog.getByRole('combobox', { name: 'Delegations per message' }).selectOption('5');
    await expect.poll(async () => (await json(await app.request('/api/main-agent'))).delegation).toEqual({ maxPerTurn: 5, minutes: 120 });
    await page.screenshot({ path: info.outputPath('main-agent-delegation.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    // A plain conversation's hand-off prompt states both limits before the user chooses.
    await page.keyboard.press('Escape');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Make the tiles');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await eventually(() => app.inspect(), (value: any) => value.requests.some((r: any) => r.method === 'turn/start'), 'turn start');
    await app.emit({ id: 'hand', method: 'item/tool/call', params: { tool: 'miniclaw_delegate', threadId: THREAD, turnId: TURN, callId: 'hand',
      arguments: { agent: 'Blender', summary: 'Build the tile set', task: 'Build six tiles.' } } });
    const popup = page.getByRole('alertdialog', { name: 'Delegate to Blender?' });
    // Requirement change 2026-10-11 (lead decision): a waited run is handed to the background at the hour, not stopped.
    await expect(popup).toContainText('In the background it may take up to 2 h. If you wait, after 1 h it continues in the background.');
    await popup.getByRole('button', { name: 'Decline' }).click();
    await expect(popup).toHaveCount(0);
  } finally { await app.close(); }
});
