// Drives the right-hand panel against a real git repository in a disposable folder.
import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';

const git = (cwd: string, ...args: string[]) =>
  execFileSync('git', ['-c', 'user.name=T', '-c', 'user.email=t@example.test', ...args], { cwd, stdio: 'pipe' });

test('the panel opens from its rail, browses files, reviews git changes, discards one and remembers its state',
  async ({ page }, info) => {
    const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-panel-')));
    const site = join(root, 'site');
    await mkdir(join(site, 'src'), { recursive: true });
    await writeFile(join(site, 'README.md'), '# Site\n');
    await writeFile(join(site, 'src/game.luau'), '--[[\nA multiline comment\n]]\nlocal count = 3\n');
    await writeFile(join(site, 'src/app.js'), 'const limit = 10;\nconsole.log(limit);\n');
    git(site, 'init', '-q'); git(site, 'add', '.'); git(site, 'commit', '-qm', 'first');
    const app = await launchApp({}, { env: { MINICLAW_TRASH_DIR: join(root, 'Trash') } });
    try {
      const post = (path: string, body: unknown) => app.request(path, { method: 'POST', body });
      const { project } = await (await post('/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site })).json();
      await post('/api/threads', { environment: { kind: 'host' }, projectId: project.id });
      await writeFile(join(site, 'src/app.js'), 'const limit = 25;\nconsole.log(limit);\n');
      await writeFile(join(site, 'notes.txt'), 'scratch\n');
      await page.goto(app.url);
      const mobile = info.project.name === 'mobile';
      if (mobile) await page.getByRole('button', { name: 'Open workspace' }).click();
      const panel = mobile ? page.getByRole('dialog', { name: 'Workspace' }) : page.getByRole('complementary', { name: 'Workspace' });
      // A Mac conversation starts with the panel folded to its rail.
      if (!mobile) {
        await expect(panel.getByRole('tab')).toHaveCount(0);
        await panel.getByRole('button', { name: 'Open Files' }).click();
      } else await panel.getByRole('tab', { name: 'Files' }).click();
      await expect(panel.getByRole('tab', { name: 'VM' })).toHaveCount(0);

      await panel.getByRole('button', { name: 'src', exact: true }).click();
      await panel.getByRole('button', { name: 'app.js' }).click();
      await expect(panel.getByRole('region', { name: 'src/app.js' })).toContainText('const limit = 25;');

      await panel.getByRole('button', { name: 'game.luau' }).click();
      const code = panel.getByRole('region', { name: 'src/game.luau' });
      await expect(code.locator('.hljs-comment').nth(1)).toHaveText('A multiline comment');
      await expect(code.locator('.hljs-keyword')).toHaveText('local');
      await expect(code.locator('.number')).toHaveText(['1', '2', '3', '4']);
      await page.screenshot({ path: info.outputPath('file-syntax.png') });

      await panel.getByRole('tab', { name: /^Changes/ }).click();
      const changed = panel.getByRole('button', { name: /^src\/app\.js/ });
      await expect(changed).toContainText('+1');
      await expect(changed).toContainText('−1');
      await expect(panel.getByRole('button', { name: /^notes\.txt/ })).toContainText('new');
      await changed.click();
      await expect(panel.getByRole('table', { name: /Changes to/ }).locator('mark').first()).toHaveText('10');
      await page.screenshot({ path: info.outputPath('panel-changes.png') });

      await panel.getByRole('button', { name: 'Discard changes to src/app.js' }).click();
      await page.getByRole('alertdialog').getByRole('button', { name: 'Discard changes' }).click();
      await expect(panel.getByRole('button', { name: /^src\/app\.js/ })).toHaveCount(0);
      expect(await readFile(join(site, 'src/app.js'), 'utf8')).toBe('const limit = 10;\nconsole.log(limit);\n');

      if (!mobile) {
        // Dragging the edge resizes it; its width, tab and fold survive a reload.
        const width = async () => Math.round((await panel.boundingBox())!.width);
        const start = await width();
        const handle = panel.getByRole('separator', { name: 'Resize workspace' });
        await handle.focus();
        for (let step = 0; step < 5; step++) await page.keyboard.press('ArrowLeft');
        expect(await width()).toBeGreaterThan(start);
        const wide = await width();
        await page.reload();
        await expect(panel.getByRole('tab', { name: /^Changes/ })).toHaveAttribute('aria-selected', 'true');
        expect(await width()).toBe(wide);
        await panel.getByRole('button', { name: 'Collapse workspace' }).click();
        await expect(panel.getByRole('tab')).toHaveCount(0);
        await page.reload();
        await expect(panel.getByRole('button', { name: 'Open Files' })).toBeVisible();
      }
    } finally { await app.close(); await rm(root, { recursive: true, force: true }); }
  });
