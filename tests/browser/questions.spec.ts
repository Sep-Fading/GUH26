// Checks the question popup in a real browser: options, several picks, the user's own answer,
// "Answer in chat", and that it cannot be dismissed by accident.
import { test, expect, type Page } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, eventually, json } from '../support/harness.mjs';
// Names the fake conversation and its turn.
import { THREAD, TURN } from '../support/fixtures.mjs';
// Opens the composer's Tools dropdown.
import { openTools, newChatButton } from './settings.js';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

type App = Awaited<ReturnType<typeof launchApp>>;
const QUESTIONS = [
  { question: 'Which language should the server use?', header: 'Language', multiSelect: false,
    options: [{ label: 'TypeScript (Recommended)', description: 'The official SDK for Node.' }, { label: 'Python', description: 'The official mcp package.' }] },
  { question: 'Which tools should it have?', header: 'Tools', multiSelect: true,
    options: [{ label: 'Search', description: 'Find notes.' }, { label: 'Create', description: 'Add a note.' }, { label: 'Delete', description: 'Remove a note.' }] }];

/** Opens a conversation with a running response and makes the model ask. */
async function asking(page: Page, app: App, id: string) {
  await page.goto(app.url);
  await (await newChatButton(page)).click();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Build me a notes server');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/start'), 'turn');
  await app.emit({ id, method: 'item/tool/call', params: { tool: 'miniclaw_ask', threadId: THREAD, turnId: TURN, callId: id, arguments: { questions: QUESTIONS } } });
  const dialog = page.getByRole('dialog', { name: 'The agent has a question' });
  await expect(dialog).toBeVisible();
  return dialog;
}
const reply = async (app: App, id: string) => (await eventually(() => app.inspect(),
  state => state.responses.some((response: { id: string }) => response.id === id), 'tool reply')).responses.find((response: { id: string }) => response.id === id).result;

test('questions are answered with options, several picks and the user\'s own words', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const dialog = await asking(page, app, 'ask-1');
    const language = dialog.getByRole('group', { name: 'Which language should the server use?' });
    const tools = dialog.getByRole('group', { name: 'Which tools should it have?' });
    await expect(language.getByRole('radio')).toHaveCount(2);
    await expect(tools.getByRole('checkbox')).toHaveCount(3);
    await expect(language).toContainText('The official SDK for Node.');
    const send = dialog.getByRole('button', { name: 'Send answers' });
    await expect(send).toBeDisabled();
    // Neither Escape nor a click outside closes it.
    await page.keyboard.press('Escape');
    await page.mouse.click(5, 5);
    await expect(dialog).toBeVisible();
    await language.getByRole('radio', { name: /Python/ }).check();
    await expect(send).toBeDisabled();
    await tools.getByRole('checkbox', { name: /Search/ }).check();
    await tools.getByRole('checkbox', { name: /Create/ }).check();
    await tools.getByRole('textbox', { name: 'Your own answer' }).fill('Also export to Markdown');
    await page.screenshot({ path: info.outputPath('question.png') });
    // Nothing a person would be blocked by: names, roles and contrast of the popup.
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.mouse.move(0, 0);
    const { violations } = await new AxeBuilder({ page }).include('[role="dialog"]').withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
    expect(violations.filter(item => ['serious', 'critical'].includes(item.impact ?? '')).map(item => `${item.id}: ${item.nodes[0].target.join(' ')}`)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await send.click();
    await expect(dialog).toHaveCount(0);
    const result = await reply(app, 'ask-1');
    expect(result.success).toBe(true);
    expect(result.contentItems[0].text).toContain('Python');
    expect(result.contentItems[0].text).toContain('Search, Create. In their own words: Also export to Markdown');
    // Activity calls it what it was: a question that was asked, not a command that ran.
    const row = page.getByRole('button', { name: /Which language should the server use/ });
    await expect(row).toContainText('Asked');
    await expect(row).not.toContainText('Did');
  } finally { await app.close(); }
});

test('the user\'s own words alone are an answer, and "Answer in chat" stops the response', async ({ page }) => {
  const app = await launchApp();
  try {
    const dialog = await asking(page, app, 'ask-2');
    await dialog.getByRole('group', { name: 'Which language should the server use?' }).getByRole('textbox', { name: 'Your own answer' }).fill('Go');
    await dialog.getByRole('group', { name: 'Which tools should it have?' }).getByRole('textbox', { name: 'Your own answer' }).fill('Only search');
    await expect(dialog.getByRole('button', { name: 'Send answers' })).toBeEnabled();
    await dialog.getByRole('button', { name: 'Answer in chat' }).click();
    await expect(dialog).toHaveCount(0);
    await eventually(() => app.inspect(), state => state.requests.some(request => request.method === 'turn/interrupt'), 'interrupt');
    expect((await reply(app, 'ask-2')).success).toBe(false);
  } finally { await app.close(); }
});

test('asking is a switch in Tools, on by default', async ({ page }) => {
  const app = await launchApp();
  try {
    const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
    await page.goto(app.url);
    const toggle = (await openTools(page)).getByRole('switch', { name: 'Ask me questions', exact: true });
    await expect(toggle).toBeChecked();
    await toggle.click();
    await expect(toggle).not.toBeChecked();
    await expect.poll(async () => (await json(await app.request(`/api/capabilities?thread=${thread.id}`))).capabilities
      .find((item: { key: string }) => item.key === 'questions').on).toBe(false);
  } finally { await app.close(); }
});
