// Checks the terminal tab in a real browser: the shell's prompt appears in xterm, typing runs
// in the working folder, the panel's width sets the terminal's width, and Restart clears it.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp, json } from '../support/harness.mjs';
// Prepares a working folder.
import { mkdtemp, realpath, rm, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Runs the axe rules inside the page.
import { AxeBuilder } from '@axe-core/playwright';

test('the terminal tab shows the shell, runs what is typed, and restarts clean', async ({ page }, info) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-terminal-ui-')));
  const site = join(root, 'site');
  await mkdir(site);
  await writeFile(join(site, 'hello.txt'), 'from the folder\n');
  const app = await launchApp();
  try {
    const post = (path: string, body: unknown) => app.request(path, { method: 'POST', body });
    const { project } = await json(await post('/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site }), 201);
    await json(await post('/api/threads', { environment: { kind: 'host' }, projectId: project.id }), 201);
    await page.goto(app.url);
    const mobile = info.project.name === 'mobile';
    if (mobile) await page.getByRole('button', { name: 'Open workspace' }).click();
    const panel = mobile ? page.getByRole('dialog', { name: 'Workspace' }) : page.getByRole('complementary', { name: 'Workspace' });
    if (!mobile) await panel.getByRole('button', { name: 'Open Terminal' }).click();
    else await panel.getByRole('tab', { name: 'Terminal' }).click();
    const terminal = panel.getByRole('region', { name: 'Terminal' });
    await expect(terminal.locator('.xterm')).toBeVisible();
    await expect(terminal).toContainText('%', { timeout: 15000 });
    await terminal.locator('.xterm-helper-textarea').focus();
    await page.keyboard.type('cat hello.txt');
    await page.keyboard.press('Enter');
    await expect(terminal).toContainText('from the folder');
    expect((await new AxeBuilder({ page }).include('[aria-label="Workspace"]').analyze()).violations.filter(item => ['serious', 'critical'].includes(item.impact ?? ''))).toEqual([]);
    await page.screenshot({ path: info.outputPath('terminal.png') });
    // The shell has the terminal's real width, so a long line wraps where the page does.
    const cols = await terminal.locator('.xterm').evaluate(() => (document.querySelector('.xterm-screen') as HTMLElement).clientWidth);
    expect(cols).toBeGreaterThan(200);
    await panel.getByRole('button', { name: 'Restart shell' }).click();
    await expect(terminal).not.toContainText('from the folder', { timeout: 15000 });
    await expect(terminal).toContainText('%', { timeout: 15000 });
  } finally { await app.close(); await rm(root, { recursive: true, force: true }); }
});
