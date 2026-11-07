// Exercises skill editing through accessible controls.
import { test, expect } from '@playwright/test';
// Starts isolated applications without real inference.
import { launchApp } from '../support/harness.mjs';
// Creates disposable installed skill import candidates.
import { mkdir, writeFile, readFile } from 'node:fs/promises';
// Resolves source paths within fixture homes.
import { join } from 'node:path';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

test('live Markdown preserves source in agents and skills', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    await openSettings(page, 'Agents');
    const agents = page.getByRole('dialog', { name: 'Settings', exact: true });
    const source = '# Rules\n\n- Keep **exact** terms.\n\n```ts\nconst limit = 4;\n```\n\n' +
      '<img src="https://invalid.example/pixel" onerror="alert(1)">\n\n' +
      '![remote](https://invalid.example/pixel)\n\n[unsafe](javascript:alert(1))';
    await agents.getByRole('textbox', { name: 'Instructions', exact: true }).fill(source);
    await agents.getByRole('textbox', { name: 'Instructions', exact: true }).press('ControlOrMeta+Home');
    await expect(agents.getByRole('tabpanel').getByRole('tab')).toHaveCount(0);
    await expect(agents.getByRole('heading', { name: 'Rules', exact: true })).toBeVisible();
    await expect(agents.locator('code')).toHaveText('const limit = 4;');
    // CodeMirror uses source-free img elements as caret buffers around hidden marks.
    await expect(agents.locator('img[src],img[srcset],script,[onerror],a[href^="javascript:"]')).toHaveCount(0);
    await agents.getByRole('button', { name: 'Close', exact: true }).click();
    await openSettings(page, 'Skills');
    const skills = page.getByRole('dialog', { name: 'Settings', exact: true });
    await skills.getByRole('textbox', { name: 'Name', exact: true }).fill('Clarity');
    await skills.getByRole('textbox', { name: 'Instructions', exact: true }).fill(source);
    await skills.getByRole('checkbox', { name: 'Active', exact: true }).check();
    await expect(skills.getByRole('heading', { name: 'Rules', exact: true })).toBeVisible();
    await skills.getByRole('button', { name: 'Save skill', exact: true }).click();
    await expect(skills.getByRole('status')).toHaveText('Saved');
    await page.screenshot({ path: info.outputPath('skills-live-markdown.png') });
    await skills.getByRole('button', { name: 'Close', exact: true }).click();
    await openSettings(page, 'Agents');
    await agents.getByRole('textbox', { name: 'Name', exact: true }).fill('Editor');
    await agents.getByRole('textbox', { name: 'Instructions', exact: true }).fill('Review text.');
    await agents.getByRole('checkbox', { name: 'Clarity', exact: true }).check();
    await agents.getByRole('button', { name: 'Save agent', exact: true }).click();
    await expect(agents.getByRole('status')).toHaveText('Saved');
    const result = await (await app.request('/api/agents')).json();
    const saved = await (await app.request('/api/skills')).json();
    expect(result.agents[0].skillIds).toEqual([saved.skills[0].id]);
    expect(saved.skills[0].instructions).toBe(source);
    await page.setViewportSize({ width: 320, height: 640 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.screenshot({ path: info.outputPath('agent-skills-narrow.png') });
  } finally { await app.close(); }
});

test('Import skills offers Codex or Claude Code as the source, needs a selection, and leaves the original unchanged', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const root = join(app.peer.codexHome, 'skills', 'fixture-skill');
    await mkdir(root, { recursive: true });
    const source = '# Imported instructions\n\nKeep `flags` unchanged.\n';
    await writeFile(join(root, 'SKILL.md'), source);
    await page.goto(app.url);
    await openSettings(page, 'Skills');
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    // One entry for importing; the source is chosen on its page.
    await expect(dialog.getByRole('button', { name: 'Import from Codex', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Import skills', exact: true }).click();
    const sources = dialog.getByRole('radiogroup', { name: 'Import from' });
    await expect(sources.getByRole('radio', { name: 'Codex' })).toBeChecked();
    await sources.getByRole('radio', { name: 'Claude Code' }).click();
    await expect(dialog.getByText('No importable skills found.')).toBeVisible();
    await sources.getByRole('radio', { name: 'Codex' }).click();
    const picker = dialog.getByRole('combobox', { name: 'Skill to import', exact: true });
    await expect(picker).toBeEnabled();
    await expect(dialog.getByRole('button', { name: 'Import skill', exact: true })).toBeDisabled();
    await picker.click();
    await page.getByRole('option', { name: 'fixture-skill', exact: true }).click();
    expect((await (await app.request('/api/skills')).json()).skills).toEqual([]);
    await page.screenshot({ path: info.outputPath('skills-import.png') });
    await dialog.getByRole('button', { name: 'Import skill', exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'Imported instructions', exact: true })).toBeVisible();
    expect((await (await app.request('/api/skills')).json()).skills[0].instructions).toBe(source);
    await expect(dialog.getByRole('checkbox', { name: 'Active', exact: true })).not.toBeChecked();
    await dialog.getByRole('textbox', { name: 'Instructions', exact: true }).fill('# Local edit');
    await dialog.getByRole('button', { name: 'Save skill', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    expect(await readFile(join(root, 'SKILL.md'), 'utf8')).toBe(source);
    await page.reload();
    await openSettings(page, 'Skills');
    await dialog.getByRole('button', { name: 'fixture-skill', exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'Local edit', exact: true })).toBeVisible();
  } finally { await app.close(); }
});
