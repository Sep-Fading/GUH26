// Checks project VM lifecycle controls through visible, named controls.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';
// Opens a category of the Settings sheet.
import { openSettings } from './settings.js';

const vmId = '019a3333-3333-7333-8333-333333333333';
const project = { id: '019a2222-2222-7222-8222-222222222222', name: 'Separate workspace',
  agentIds: [], skillIds: [], revision: 1, environment: { kind: 'vm', dedicated: true, vmId } };

// Opens task navigation on mobile without changing the desktop path.
async function navigation(page: import('@playwright/test').Page) {
  const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await toggle.isVisible()) await toggle.click();
  return page.getByRole('navigation', { name: 'Tasks', exact: true });
}

// Serves one project VM: its state, idle setting and power requests, all recorded.
async function fakeVm(page: import('@playwright/test').Page, state: { value: string }) {
  const seen: { power: string[]; idle: number[]; deleted: unknown[] } = { power: [], idle: [], deleted: [] };
  await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: {
    state: state.value, ready: state.value === 'running' } }));
  await page.route(`**/api/vms/${vmId}/idle`, route => {
    if (route.request().method() === 'POST') seen.idle.push(route.request().postDataJSON().minutes);
    return route.fulfill({ json: { minutes: seen.idle.at(-1) ?? 0, choices: [0, 15, 30, 60, 120] } });
  });
  await page.route(`**/api/vms/${vmId}/power`, route => {
    const { action } = route.request().postDataJSON();
    seen.power.push(action);
    state.value = action === 'start' ? 'running' : action === 'stop' ? 'off' : state.value;
    return route.fulfill({ json: { state: action === 'start' ? 'starting' : action === 'stop' ? 'stopping' : 'running' } });
  });
  return seen;
}

test('a project controls its own VM: start, shut down, window and idle time', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const state = { value: 'off' };
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects: [project] } }) : route.continue());
    const seen = await fakeVm(page, state);
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'Settings for Separate workspace' }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    const controls = dialog.getByRole('group', { name: 'Project VM' });
    await expect(controls.getByText('Off', { exact: true })).toBeVisible();
    await expect(controls.getByRole('button', { name: 'Shut down' })).toBeDisabled();
    await controls.getByRole('button', { name: 'Start' }).click();
    await expect(controls.getByText('Running', { exact: true })).toBeVisible();
    await controls.getByRole('button', { name: 'Open window' }).click();
    await controls.getByRole('combobox', { name: 'Shut down when idle' }).selectOption('30');
    await expect(controls.getByRole('status')).toHaveText('Saved');
    await controls.getByRole('button', { name: 'Shut down' }).click();
    await expect(controls.getByText('Off', { exact: true })).toBeVisible();
    expect(seen).toEqual({ power: ['start', 'show', 'stop'], idle: [30], deleted: [] });
    await page.screenshot({ path: info.outputPath('project-vm-controls.png') });
  } finally { await app.close(); }
});

test('deleting a project takes its VM along only with the box ticked and the name typed', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const state = { value: 'off' };
  let projects = [project];
  const posted: unknown[] = [];
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects } }) : route.continue());
    await page.route(`**/api/projects/${project.id}/delete`, route => {
      posted.push(route.request().postDataJSON());
      projects = [];
      return route.fulfill({ json: {} });
    });
    // One conversation belongs to the project and one does not; only the first may vanish.
    await page.route('**/api/threads', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { threads: [
        { id: '019a4444-4444-7444-8444-444444444444', name: 'Project conversation', model: null,
          projectId: project.id, environment: { kind: 'vm' }, vmId },
        { id: '019a6666-6666-7666-8666-666666666666', name: 'Standalone conversation', model: null,
          environment: { kind: 'host' } }], nextCursor: null } }) : route.continue());
    await fakeVm(page, state);
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'Settings for Separate workspace' }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    await dialog.getByRole('button', { name: 'Delete project' }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Delete project?' });
    await expect(confirm.getByText('every conversation in it')).toBeVisible();
    const box = confirm.getByRole('checkbox', { name: "Also delete this project's VM" });
    await expect(box).not.toBeChecked();
    await box.check();
    const name = confirm.getByRole('textbox', { name: 'Type the project name to confirm' });
    const action = confirm.getByRole('button', { name: 'Delete project and VM' });
    await expect(action).toBeDisabled();
    await name.fill('Separate workspace');
    await action.click();
    await expect(confirm).toBeHidden();
    expect(posted).toEqual([{ revision: 1, deleteVm: true, confirmName: 'Separate workspace' }]);
    await expect(nav.getByRole('button', { name: 'Separate workspace', exact: true })).toHaveCount(0);
    await expect(nav.getByRole('button', { name: 'Project conversation', exact: true })).toHaveCount(0);
    await expect(nav.getByRole('button', { name: 'Standalone conversation', exact: true })).toBeVisible();
  } finally { await app.close(); }
});

test('a project is deleted without its VM by default, and a running VM cannot be taken along', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const state = { value: 'running' };
  const posted: unknown[] = [];
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects: [project] } }) : route.continue());
    await page.route(`**/api/projects/${project.id}/delete`, route => {
      posted.push(route.request().postDataJSON());
      return route.fulfill({ json: {} });
    });
    await fakeVm(page, state);
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'Settings for Separate workspace' }).click();
    await page.getByRole('dialog', { name: 'Project settings' }).getByRole('button', { name: 'Delete project' }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Delete project?' });
    await expect(confirm.getByRole('checkbox', { name: "Also delete this project's VM" })).toBeDisabled();
    await expect(confirm.getByText('Shut the VM down first to delete it.')).toBeVisible();
    await confirm.getByRole('button', { name: 'Delete project', exact: true }).click();
    await expect(confirm).toBeHidden();
    expect(posted).toEqual([{ revision: 1 }]);
  } finally { await app.close(); }
});

test('Settings lists project VMs; an orphan can be started, stopped and deleted after confirmation',
  async ({ page }, info) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    const state = { value: 'off' };
    const owned = '019a5555-5555-7555-8555-555555555555';
    const GiB = 1024 ** 3;
    let deleted = false;
    const view = () => ({ available: true, workerCurrent: null, master: { state: 'off', ready: false },
      ledger: { vms: [], limits: { cpuPercent: 50, memoryPercent: 50, diskPercent: 25 },
        capacity: { cpuCount: 5, memoryBytes: 12 * GiB, diskBytes: 231 * GiB },
        used: { cpuCount: 0, memoryBytes: 0, diskBytes: 0 }, exceeded: [] },
      projectVms: [{ vmId: owned, state: 'off', projectId: project.id, projectName: 'Owned project' },
        ...(deleted ? [] : [{ vmId, state: state.value, projectId: null, projectName: null }])] });
    try {
      await page.route('**/api/virtualisation', route => route.fulfill({ json: view() }));
      await page.route('**/api/vms/master/idle', route => route.fulfill({ json: { minutes: 0, choices: [0, 15, 30, 60, 120] } }));
      await page.route(`**/api/vms/${owned}/**`, route => route.fulfill({ json: { state: 'off', ready: false, minutes: 0, choices: [0, 15] } }));
      await page.route(`**/api/vms/${vmId}/delete`, route => { deleted = true; return route.fulfill({ json: {} }); });
      const seen = await fakeVm(page, state);
      await page.goto(app.url);
      const dialog = await openSettings(page, 'Virtualisation');
      await expect(dialog.getByRole('group', { name: 'Master VM' })
        .getByRole('combobox', { name: 'Shut down when idle' })).toHaveValue('0');
      const ownedRow = dialog.getByRole('group', { name: 'Owned project' });
      await expect(ownedRow.getByRole('button', { name: 'Delete VM' })).toHaveCount(0);
      const orphan = dialog.getByRole('group', { name: 'Orphaned VM 019a3333' });
      await expect(orphan.getByText('Its project was deleted.')).toBeVisible();
      await orphan.getByRole('button', { name: 'Start' }).click();
      await expect(orphan.getByText('Running', { exact: true })).toBeVisible();
      await expect(orphan.getByRole('button', { name: 'Delete VM' })).toBeDisabled();
      await orphan.getByRole('button', { name: 'Shut down' }).click();
      await expect(orphan.getByText('Off', { exact: true })).toBeVisible();
      await page.screenshot({ path: info.outputPath('orphaned-vm.png') });
      await orphan.getByRole('button', { name: 'Delete VM' }).click();
      const confirm = page.getByRole('alertdialog', { name: 'Delete VM?' });
      await expect(confirm.getByText('cannot be undone')).toBeVisible();
      await confirm.getByRole('button', { name: 'Delete VM' }).click();
      await expect(dialog.getByRole('group', { name: 'Orphaned VM 019a3333' })).toHaveCount(0);
      expect(seen.power).toEqual(['start', 'stop']);
    } finally { await app.close(); }
  });

test('an idle VM is announced with its seconds left, and Keep running answers it', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  let warnings = [{ vmId, name: 'Separate workspace', secondsLeft: 42 }];
  const kept: string[] = [];
  try {
    await page.route('**/api/vms/idle', route => route.fulfill({ json: { warnings } }));
    await page.route(`**/api/vms/${vmId}/idle/keep`, route => {
      kept.push(vmId); warnings = [];
      return route.fulfill({ json: {} });
    });
    await page.goto(app.url);
    const alert = page.getByRole('alert').filter({ hasText: 'Separate workspace' });
    await expect(alert).toContainText('is idle and shuts down in');
    await alert.getByRole('button', { name: 'Keep running' }).click();
    await expect(alert).toHaveCount(0);
    expect(kept).toEqual([vmId]);
  } finally { await app.close(); }
});

test('backups are labelled experimental, start off, and run or restore only on request', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const state = { value: 'running' };
  const posted: unknown[] = [];
  let settings = { schedule: 'off', keep: 3, folders: [''], lastRun: null, lastError: null };
  const backups = [{ id: '1790000000-abc123', created: 1790000000, files: 12, bytes: 4096 }];
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects: [project] } }) : route.continue());
    await fakeVm(page, state);
    await page.route(`**/api/vms/${vmId}/backups`, route => route.fulfill({ json: { experimental: true, settings, backups } }));
    await page.route(`**/api/vms/${vmId}/backups/settings`, route => {
      posted.push(route.request().postDataJSON());
      settings = { ...settings, ...route.request().postDataJSON() };
      return route.fulfill({ json: { experimental: true, settings, backups } });
    });
    await page.route(`**/api/vms/${vmId}/backups/run`, route => { posted.push('run'); return route.fulfill({ status: 202, json: {} }); });
    await page.route(`**/api/vms/${vmId}/backups/*/restore`, route => { posted.push('restore');
      return route.fulfill({ status: 202, json: { jobId: 'job-1' } }); });
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'Settings for Separate workspace' }).click();
    const section = page.getByRole('dialog', { name: 'Project settings' }).getByRole('group', { name: 'Backups' });
    await expect(section.getByText('Experimental')).toBeVisible();
    await expect(section.getByRole('combobox', { name: 'Backup schedule' })).toHaveValue('off');
    expect(posted).toEqual([]);
    await section.getByRole('combobox', { name: 'Backup schedule' }).selectOption('daily');
    await section.getByRole('button', { name: 'Back up now' }).click();
    await expect(section.getByRole('status')).toContainText('Backup started');
    await section.getByRole('button', { name: 'Restore' }).click();
    await expect(section.getByRole('status')).toContainText('Transfers folder');
    expect(posted).toEqual([{ schedule: 'daily', keep: 3 }, 'run', 'restore']);
  } finally { await app.close(); }
});
