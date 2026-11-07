// Checks the hand-over of /skill-create and /agent-create in a real browser: the card, the form
// it fills in, that nothing is saved before Save, and that each suggested permission of an agent
// is approved or refused on its own.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Builds a finished turn the fake Codex returns as history.
import { message, turn } from '../support/fixtures.mjs';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

const post = (app: Awaited<ReturnType<typeof launchApp>>, path: string, body: unknown) => app.request(path, { method: 'POST', body });
const history = (text: string, answer: string) => ({ 'thread/turns/list': { result: { data: [turn('completed', [
  { id: 'user-1', type: 'userMessage', content: [{ type: 'text', text, text_elements: [] }] }, message(answer, 'reply-1')])], nextCursor: null } } });
const block = (kind: string, value: unknown) => `Here it is.\n\n\`\`\`${kind}\n${JSON.stringify(value)}\n\`\`\``;

test('a proposed skill opens the skill form filled in, inactive, and is saved only by the user', async ({ page }) => {
  const app = await launchApp(history('/skill-create a review skill', block('miniclaw-skill', { name: 'Pull request review',
    description: 'Reviews a pull request.', instructions: 'When asked to review, read the diff first.', slash: true, replaces: null, active: true })));
  try {
    await page.goto(app.url);
    const card = page.getByRole('group', { name: 'Skill Pull request review' });
    await expect(card).toContainText('Reviews a pull request.');
    await card.getByRole('button', { name: 'Review this skill' }).click();
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(dialog.getByRole('heading', { name: 'Skills', exact: true })).toBeVisible();
    await expect(dialog.getByRole('note')).toContainText('Written by the agent');
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('Pull request review');
    await expect(dialog.getByRole('checkbox', { name: 'Active', exact: true })).not.toBeChecked();
    expect((await json(await app.request('/api/skills'))).skills).toEqual([]);
    await dialog.getByRole('button', { name: 'Save skill' }).click();
    await expect.poll(async () => (await json(await app.request('/api/skills'))).skills.map((skill: { name: string; active: boolean; instructions: string }) =>
      [skill.name, skill.active, skill.instructions])).toEqual([['Pull request review', false, 'When asked to review, read the diff first.']]);
  } finally { await app.close(); }
});

test('a new version of a skill opens that skill, and saving replaces its text', async ({ page }) => {
  const app = await launchApp(history('/skill-create improve release notes', block('miniclaw-skill', { name: 'ignored', description: 'Shorter notes.',
    instructions: 'Write five bullets at most.', slash: false, replaces: 'release notes' })));
  try {
    await json(await post(app, '/api/skills', { name: 'Release Notes', description: 'Notes.', instructions: 'Write release notes.', active: true }), 201);
    await page.goto(app.url);
    await page.getByRole('group', { name: 'Skill ignored' }).getByRole('button', { name: 'Review this skill' }).click();
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(dialog.getByRole('note')).toContainText('new version of Release Notes');
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('Release Notes');
    await dialog.getByRole('button', { name: 'Save skill' }).click();
    await expect.poll(async () => (await json(await app.request('/api/skills'))).skills.map((skill: { name: string; active: boolean; instructions: string }) =>
      [skill.name, skill.active, skill.instructions])).toEqual([['Release Notes', true, 'Write five bullets at most.']]);
  } finally { await app.close(); }
});

test('a proposed agent is saved without permissions, and each suggestion is approved or refused on its own', async ({ page }) => {
  const app = await launchApp(history('/agent-create a reviewer', block('miniclaw-agent', { name: 'Reviewer', instructions: 'You review code.',
    model: 'no-such-model', effort: 'low', skills: ['Release Notes', 'Missing skill'], replaces: null,
    permissions: [{ kind: 'folder', path: '/private/tmp/site', action: 'read', reason: 'It reads the code it reviews.' },
      { kind: 'feature', name: 'network', reason: 'It installs packages.' },
      { kind: 'mcp_server', name: 'ghost', reason: 'Looks things up.' }] })));
  try {
    await json(await post(app, '/api/skills', { name: 'Release Notes', description: 'Notes.', instructions: 'Write release notes.', active: true }), 201);
    // Buttons fade from disabled to enabled; the contrast check must not catch them halfway.
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(app.url);
    const card = page.getByRole('group', { name: 'Agent Reviewer' });
    await expect(card).toContainText('Read folder /private/tmp/site');
    await expect(card).toContainText('You approve each one yourself after saving');
    await card.getByRole('button', { name: 'Review this agent' }).click();
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(dialog.getByRole('heading', { name: 'Agents', exact: true })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('Reviewer');
    await expect(dialog.getByRole('note')).toContainText('no-such-model is not available');
    await expect(dialog.getByRole('note')).toContainText('Missing skill');
    await expect(dialog.getByRole('checkbox', { name: 'Release Notes' })).toBeChecked();
    const suggested = dialog.getByRole('region', { name: 'Suggested permissions' });
    const allowFolder = suggested.getByRole('button', { name: 'Allow: Read folder /private/tmp/site', exact: true });
    await expect(allowFolder).toBeDisabled();
    await expect(suggested).toContainText('Save the agent first');
    expect((await json(await app.request('/api/agents'))).agents).toEqual([]);

    await dialog.getByRole('button', { name: 'Save agent' }).click();
    await expect(allowFolder).toBeEnabled();
    const [agent] = (await json(await app.request('/api/agents'))).agents;
    expect([agent.name, agent.active]).toEqual(['Reviewer', false]);
    const rules = async () => (await json(await app.request(`/api/permissions/agent/${agent.id}`))).rules.map((rule: { target: unknown; effect: string }) => [rule.target, rule.effect]);
    expect(await rules()).toEqual([]);
    expect((await new AxeBuilder({ page }).include('[aria-label="Suggested permissions"]').analyze()).violations).toEqual([]);

    await allowFolder.click();
    await expect(suggested.getByText('Allowed', { exact: true })).toHaveCount(1);
    await suggested.getByRole('button', { name: 'Do not allow: Network for commands' }).click();
    await suggested.getByRole('button', { name: 'Allow: MCP server ghost', exact: true }).click();
    await expect(suggested.getByRole('alert')).toContainText('No MCP server is named ghost');
    // One click, one rule; nothing else was granted.
    expect(await rules()).toEqual([[{ target_type: 'folder', path: '/private/tmp/site', action: 'read' }, 'allow']]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); }
});
