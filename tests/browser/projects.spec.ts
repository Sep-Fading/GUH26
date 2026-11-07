// Exercises project controls through accessible names.
import { test as base, expect } from '@playwright/test';
// Starts isolated backends without real credentials.
import { launchApp, eventually } from '../support/harness.mjs';
// Supplies distinct synthetic chats and notifications.
import { THREAD, OTHER, started, thread, delta } from '../support/fixtures.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

const test = base.extend<{ app: Awaited<ReturnType<typeof launchApp>> }>({
  // Keeps project UI cases independent of real saved chats.
  app: async ({}, use) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    try { await use(app); } finally { await app.close(); }
  },
});

// Opens navigation on narrow layouts while leaving desktop sidebar unchanged.
async function navigation(page: import('@playwright/test').Page) {
  const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await toggle.isVisible()) await toggle.click();
  return page.getByRole('navigation', { name: 'Tasks', exact: true });
}

test('projects group independent chats and persist collapse state', async ({ page, app }, info) => {
  await page.goto(app.url);
  let nav = await navigation(page);
  await nav.getByRole('button', { name: 'New project', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Project settings', exact: true });
  await settings.getByRole('textbox', { name: 'Name', exact: true }).fill('Game');
  await settings.getByRole('button', { name: 'Save project', exact: true }).click();
  await settings.getByRole('button', { name: 'Close project settings', exact: true }).click();
  await nav.getByRole('button', { name: 'New chat in Game', exact: true }).click();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('First project chat');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.some(r => r.method === 'turn/start'));
  await app.set('thread/start', { result: started(app.peer.cwd, { id: OTHER }) });
  await app.set('turn/start', { result: { turn: { id: 'second-turn', status: 'inProgress', items: [] } } });
  nav = await navigation(page);
  await nav.getByRole('button', { name: 'New chat in Game', exact: true }).click();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Second project chat');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await eventually(() => app.inspect(), s => s.requests.filter(r => r.method === 'turn/start').length === 2);
  await app.emit(delta('First still running'));
  nav = await navigation(page);
  await nav.getByRole('button', { name: 'First project chat', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Assistant message' })).toHaveText('First still running');
  nav = await navigation(page);
  await page.screenshot({ path: info.outputPath('projects.png') });
  await nav.getByRole('button', { name: 'Game', exact: true }).click();
  await expect(nav.getByRole('button', { name: 'First project chat', exact: true })).toBeHidden();
  await page.reload();
  nav = await navigation(page);
  await expect(nav.getByRole('button', { name: 'Game', exact: true })).toHaveAttribute('aria-expanded', 'false');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
});

test('project settings select agents and default skills', async ({ page, app }) => {
  const a = await (await app.request('/api/agents', { method: 'POST', body: {
    name: 'Editor', instructions: 'Review.', active: true,
  } })).json();
  const s = await (await app.request('/api/skills', { method: 'POST', body: {
    name: 'Clarity', instructions: 'Be concise.', active: true,
  } })).json();
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'New project', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Project settings', exact: true });
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Work');
  await dialog.getByRole('checkbox', { name: 'Editor', exact: true }).check();
  await dialog.getByRole('checkbox', { name: 'Clarity', exact: true }).check();
  await dialog.getByRole('button', { name: 'Save project', exact: true }).click();
  await expect(dialog.getByRole('status')).toHaveText('Saved');
  const { projects } = await (await app.request('/api/projects')).json();
  expect(projects[0].agentIds).toEqual([a.agent.id]);
  expect(projects[0].skillIds).toEqual([s.skill.id]);
});

test('inactive project skills require activation consent', async ({ page, app }, info) => {
  const { skill } = await (await app.request('/api/skills', { method: 'POST', body: {
    name: 'Clarity', instructions: 'Keep exact identifiers.',
  } })).json();
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'New project', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Project settings', exact: true });
  await settings.getByRole('textbox', { name: 'Name', exact: true }).fill('Work');
  const checkbox = settings.getByRole('checkbox', { name: /Clarity/ });
  await expect(checkbox).toBeEnabled();
  await checkbox.click();
  const consent = page.getByRole('alertdialog', { name: 'Activate skill?', exact: true });
  await expect(consent).toContainText('Other chats');
  expect((await (await app.request('/api/skills')).json()).skills[0].active).toBe(false);
  await consent.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(checkbox).not.toBeChecked();
  await checkbox.focus();
  await page.keyboard.press('Space');
  await page.screenshot({ path: info.outputPath('skill-activation.png') });
  await consent.getByRole('button', { name: 'Activate and add', exact: true }).click();
  await expect(consent).toBeHidden();
  await expect(checkbox).toBeChecked();
  const current = (await (await app.request('/api/skills')).json()).skills[0];
  expect(current.active).toBe(true);
  expect(current.instructions).toBe(skill.instructions);
  expect(current.revision).toBe(skill.revision + 1);
  await settings.getByRole('button', { name: 'Save project', exact: true }).click();
  await expect(settings.getByRole('status')).toHaveText('Saved');
  expect((await (await app.request('/api/projects')).json()).projects[0].skillIds).toEqual([skill.id]);
  await settings.getByRole('button', { name: 'Close project settings', exact: true }).click();
  await nav.getByRole('button', { name: 'Settings for Work', exact: true }).click();
  await expect(checkbox).toBeChecked();
  await checkbox.uncheck();
  await settings.getByRole('button', { name: 'Save project', exact: true }).click();
  await expect(settings.getByRole('status')).toHaveText('Saved');
  expect((await (await app.request('/api/skills')).json()).skills[0].active).toBe(true);
});

test('failed skill activation preserves the project draft', async ({ page, app }) => {
  const { skill } = await (await app.request('/api/skills', { method: 'POST', body: {
    name: 'Clarity', instructions: 'Keep exact identifiers.',
  } })).json();
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'New project', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Project settings', exact: true });
  await settings.getByRole('textbox', { name: 'Name', exact: true }).fill('Unsaved project');
  const checkbox = settings.getByRole('checkbox', { name: /Clarity/ });
  await checkbox.click();
  const consent = page.getByRole('alertdialog', { name: 'Activate skill?', exact: true });
  let release!: () => void;
  const held = new Promise<void>(resolve => release = resolve);
  await page.route(`**/api/skills/${skill.id}`, async route => {
    await held;
    await route.fulfill({ status: 409, contentType: 'application/json',
      body: JSON.stringify({ error: { message: 'Skill changed. Reload.' } }),
    });
  });
  await consent.getByRole('button', { name: 'Activate and add', exact: true }).click();
  await expect(consent.getByRole('button', { name: 'Activate and add', exact: true })).toBeDisabled();
  await expect(consent.getByRole('button', { name: 'Cancel', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(consent).toBeVisible();
  release();
  await expect(consent.getByRole('alert')).toHaveText('Skill changed. Reload.');
  await consent.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(checkbox).not.toBeChecked();
  await expect(settings.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('Unsaved project');
  expect((await (await app.request('/api/skills')).json()).skills[0].active).toBe(false);
});

test('project skill limit still allows removal', async ({ page, app }) => {
  const ids = [];
  for (let i = 0; i < 9; i++) {
    const { skill } = await (await app.request('/api/skills', { method: 'POST', body: {
      name: `Skill ${i}`, instructions: 'Fixture instructions.', active: i < 8,
    } })).json();
    ids.push(skill.id);
  }
  await app.request('/api/projects', { method: 'POST', body: { name: 'Full',
    skillIds: ids.slice(0, 8), environment: { kind: 'host' } } });
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'Settings for Full', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Project settings', exact: true });
  await expect(settings.getByRole('checkbox', { name: /Skill 8/ })).toBeDisabled();
  await settings.getByRole('checkbox', { name: 'Skill 0', exact: true }).uncheck();
  await expect(settings.getByRole('checkbox', { name: /Skill 8/ })).toBeEnabled();
});

test('chat deletion confirms, reports failure, and removes only after success', async ({ page, app }) => {
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null }, cases: [
    { params: { ancestorThreadId: THREAD }, handler: { result: { data: [], nextCursor: null } } },
  ] });
  await page.goto(app.url);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'Delete chat First task', exact: true }).click();
  const confirm = page.getByRole('alertdialog');
  await confirm.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect((await app.inspect()).requests.some(r => r.method === 'thread/delete')).toBe(false);
  await nav.getByRole('button', { name: 'Delete chat First task', exact: true }).click();
  await app.set('thread/delete', { error: { code: -32600, message: 'Failure' } });
  await confirm.getByRole('button', { name: 'Delete chat', exact: true }).click();
  await expect(confirm.getByRole('alert')).toContainText('Codex rejected');
  await app.set('thread/delete', { result: {} });
  await confirm.getByRole('button', { name: 'Delete chat', exact: true }).click();
  await expect(confirm).toBeHidden();
  await expect(nav.getByRole('button', { name: 'First task', exact: true })).toHaveCount(0);
});

test('uncertain creation stays visible and can be deleted without resuming', async ({ page, app }) => {
  await app.set('thread/start', { result: {} });
  expect((await app.request('/api/threads', { method: 'POST',
    body: { environment: { kind: 'host' } } })).status).toBe(503);
  await app.set('thread/list', { result: { data: [thread(app.peer.cwd)], nextCursor: null }, cases: [
    { params: { ancestorThreadId: THREAD }, handler: { result: { data: [], nextCursor: null } } },
  ] });
  await app.set('thread/delete', { result: {} });
  await page.goto(app.url);
  await expect(page.getByRole('alert')).toContainText("This chat's creation was not confirmed");
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'Delete chat First task', exact: true }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete chat', exact: true }).click();
  await expect(page.getByRole('alertdialog')).toBeHidden();
  await expect(nav.getByRole('button', { name: 'First task', exact: true })).toHaveCount(0);
  expect((await app.inspect()).requests.some(r => r.method === 'thread/resume' || r.method === 'turn/start'))
    .toBe(false);
});

for (const kind of ['agent', 'skill']) {
  test(`${kind} deletion requires confirmation`, async ({ page, app }, info) => {
    await app.request(`/api/${kind}s`, { method: 'POST', body: {
      name: 'Disposable', instructions: 'Fixture only.',
    } });
    await page.goto(app.url);
    await openSettings(page, kind === 'agent' ? 'Agents' : 'Skills');
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Disposable', exact: true }).click();
    const remove = dialog.getByRole('button', { name: `Delete ${kind}`, exact: true });
    await expect(remove).toHaveText('Delete');
    await expect(remove).toHaveCSS('background-color', 'rgb(255, 255, 255)');
    await expect(remove).toHaveCSS('color', 'rgb(201, 37, 45)');
    await expect(remove).toHaveCSS('border-top-color', 'rgb(201, 37, 45)');
    await expect(remove).toHaveCSS('border-top-width', '1px');
    await expect(remove.locator('svg')).toHaveCSS('stroke', 'rgb(201, 37, 45)');
    if (info.project.name === 'mobile') await page.setViewportSize({ width: 320, height: 844 });
    await remove.scrollIntoViewIfNeeded();
    const bounds = await remove.boundingBox();
    const save = await dialog.getByRole('button', { name: `Save ${kind}`, exact: true }).boundingBox();
    expect(Math.abs(bounds!.height - save!.height)).toBeLessThanOrEqual(2);
    expect(save!.x + save!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    await page.screenshot({ path: info.outputPath(`${kind}-delete-button.png`) });
    if (info.project.name === 'desktop') {
      await remove.hover();
      await expect(remove).toHaveCSS('background-color', 'rgb(201, 37, 45)');
      await expect(remove).toHaveCSS('color', 'rgb(255, 255, 255)');
      await expect(remove.locator('svg')).toHaveCSS('stroke', 'rgb(255, 255, 255)');
      await expect(remove).toHaveCSS('border-top-color', 'rgb(201, 37, 45)');
      expect(await remove.boundingBox()).toEqual(bounds);
      await remove.screenshot({ path: info.outputPath(`${kind}-delete-hover.png`) });
      await page.mouse.move(0, 0);
      await expect(remove).toHaveCSS('background-color', 'rgb(255, 255, 255)');
      await expect(remove).toHaveCSS('color', 'rgb(201, 37, 45)');
      await expect(remove.locator('svg')).toHaveCSS('stroke', 'rgb(201, 37, 45)');
    }
    await remove.click();
    const confirm = page.getByRole('alertdialog');
    await confirm.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Disposable', exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: `Delete ${kind}`, exact: true }).click();
    await confirm.getByRole('button', { name: `Delete ${kind}`, exact: true }).click();
    await expect(confirm).toBeHidden();
    await expect(dialog.getByRole('button', { name: 'Disposable', exact: true })).toHaveCount(0);
  });
}

test('project settings hold the project\'s instructions and name the rule files of its folder', async ({ page, app }) => {
  const { mkdtemp, realpath, rm, writeFile } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-project-rules-')));
  try {
    await writeFile(join(root, 'AGENTS.md'), 'Run the tests before you finish.');
    const { project } = await (await app.request('/api/projects', { method: 'POST', body: { name: 'Site', environment: { kind: 'host' }, workspace: root } })).json();
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'Settings for Site', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings', exact: true });
    await expect(dialog).toContainText('Also read from the folder: AGENTS.md (32 B)');
    const editor = dialog.getByRole('textbox', { name: 'Project instructions', exact: true });
    await editor.click();
    await page.keyboard.type('Answer in British English.');
    await dialog.getByRole('button', { name: 'Save project', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    const { projects } = await (await app.request('/api/projects')).json();
    expect(projects.find((item: { id: string }) => item.id === project.id).instructions).toBe('Answer in British English.');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('project settings list the shared memory, and notes are added, changed and deleted there', async ({ page, app }) => {
  const { project } = await (await app.request('/api/projects', { method: 'POST', body: { name: 'Game', environment: { kind: 'host' } } })).json();
  await app.request(`/api/projects/${project.id}/memory`, { method: 'POST', body: { name: 'Engine', description: 'Roblox Studio.', text: 'Luau.' } });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'Settings for Game', exact: true }).click();
  const memory = page.getByRole('dialog', { name: 'Project settings', exact: true }).getByRole('region', { name: 'Project memory' });
  await expect(memory.getByRole('listitem')).toContainText(['Engine']);
  await memory.getByRole('button', { name: 'Add note' }).click();
  const form = memory.getByRole('group', { name: 'New note' });
  await form.getByRole('textbox', { name: 'Note name' }).fill('Art style');
  await form.getByRole('textbox', { name: 'Note description' }).fill('Low-poly, flat colours.');
  await form.getByRole('textbox', { name: 'Note text' }).fill('password: hunter2hunter2hunter2');
  await form.getByRole('button', { name: 'Save note' }).click();
  await expect(memory.getByRole('alert')).toContainText('Secrets are never saved');
  await form.getByRole('textbox', { name: 'Note text' }).fill('Trees under 300 triangles.');
  await form.getByRole('button', { name: 'Save note' }).click();
  await expect(memory.getByRole('listitem')).toContainText(['Art style', 'Engine']);
  const { AxeBuilder } = await import('@axe-core/playwright');
  expect((await new AxeBuilder({ page }).include('[aria-label="Project memory"]').analyze()).violations).toEqual([]);
  await memory.getByRole('button', { name: 'Edit note Engine' }).click();
  await memory.getByRole('group', { name: 'Edit note Engine' }).getByRole('textbox', { name: 'Note name' }).fill('Game engine');
  await memory.getByRole('button', { name: 'Save note' }).click();
  await memory.getByRole('button', { name: 'Delete note Art style' }).click();
  await expect(memory.getByRole('listitem')).toContainText(['Game engine']);
  const { notes } = await (await app.request(`/api/projects/${project.id}/memory`)).json();
  expect(notes.map((note: { name: string }) => note.name)).toEqual(['Game engine']);
});

test('project settings offer the issue review level, saved at once', async ({ page, app }) => {
  const { project } = await (await app.request('/api/projects', { method: 'POST', body: { name: 'Game', environment: { kind: 'host' } } })).json();
  await page.goto(app.url);
  const nav = await navigation(page);
  await nav.getByRole('button', { name: 'Settings for Game', exact: true }).click();
  const issues = page.getByRole('dialog', { name: 'Project settings', exact: true }).getByRole('region', { name: 'Issues' });
  const level = issues.getByRole('group', { name: 'Agents may close without review' });
  await expect(level.getByRole('radio', { name: /^Low \(default\)/ })).toBeChecked();
  await level.getByRole('radio', { name: /^Everything/ }).check();
  await expect.poll(async () => (await (await app.request(`/api/projects/${project.id}/issue-settings`)).json()).closeWithoutReview).toBe('high');
});
