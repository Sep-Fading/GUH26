// Checks permission settings in a real browser.
import { test, expect } from '@playwright/test';
// Starts a disposable backend without native actions.
import { launchApp, json } from '../support/harness.mjs';

test('conversation can block an inherited app grant', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const agent = (await json(await app.request('/api/agents', { method: 'POST', body: {
      name: 'Editor', instructions: 'Use approved tools only.', active: true,
    } }), 201)).agent;
    await json(await app.request('/api/permissions/agent/' + agent.id, {
      method: 'POST', body: { target: { target_type: 'application',
        id: 'com.apple.TextEdit', action: 'control' }, effect: 'allow' },
    }));
    await json(await app.request('/api/threads', { method: 'POST', body: {
      agentId: agent.id, environment: { kind: 'host' },
    } }), 201);

    await page.goto(app.url);
    await page.getByRole('button', { name: 'Conversation permissions' }).click();
    const settings = page.getByRole('dialog', { name: 'Conversation permissions' });
    await expect(settings.getByText('com.apple.TextEdit', { exact: true })).toBeVisible();
    await expect(settings.getByText('Allowed by agent', { exact: true })).toBeVisible();
    await settings.getByRole('button', { name: 'Block in this conversation' }).click();
    await expect(settings.getByText('Blocked in this conversation', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('permissions.png') });
  } finally { await app.close(); }
});

test('conversation requests another app without enabling it', async ({ page }) => {
  const app = await launchApp();
  try {
    await json(await app.request('/api/threads', { method: 'POST', body: {
      environment: { kind: 'host' },
    } }), 201);
    await page.goto(app.url);
    await page.getByRole('button', { name: 'Conversation permissions' }).click();
    const settings = page.getByRole('dialog', { name: 'Conversation permissions' });
    await settings.getByRole('textbox', { name: 'Request another app' }).fill('Preview');
    await settings.getByRole('button', { name: 'Request app review' }).click();
    await expect(settings.getByText('Preview: pending review')).toBeVisible();
    await expect(settings.getByText('No saved permissions for this scope.')).toBeVisible();
  } finally { await app.close(); }
});

test('scoped popup shows exact command and three choices', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: {
      environment: { kind: 'host' },
    } }), 201);
    const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
    snapshot.scopedApproval = { id: 'fixture-approval', expiresInSeconds: 60,
      target: { target_type: 'command', kind: 'shell',
        text: 'printf approved', cwd: '/private/tmp/exact-cwd' } };
    await page.route(`**/api/threads/${thread.id}`, route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(snapshot) }));
    await page.route(`**/api/threads/${thread.id}/events`, route => route.abort());
    await page.route(`**/api/threads/${thread.id}/scoped-approvals/*`, route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: '{}' }));
    await page.goto(app.url);
    const dialog = page.getByRole('dialog', { name: 'Approve agent access' });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText('printf approved', { exact: true })).toBeVisible();
    await expect(dialog.getByText('Working directory: /private/tmp/exact-cwd')).toBeVisible();
    for (const label of ['Approve Once', 'Approve for this session', 'Reject']) {
      await expect(dialog.getByRole('button', { name: label, exact: true })).toBeVisible();
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('scoped-approval.png') });
    await dialog.getByRole('button', { name: 'Approve for this session' }).click();
    await expect(dialog).toHaveCount(0);
  } finally { await app.close(); }
});

test('a command popup lists the hosts it wants and offers the whole domain', async ({ page }, info) => {
  const app = await launchApp();
  try {
    const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: {
      environment: { kind: 'host' },
    } }), 201);
    const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
    snapshot.scopedApproval = { id: 'fixture-approval', expiresInSeconds: 60,
      hosts: ['registry.npmjs.org', 'objects.githubusercontent.com'],
      domains: ['npmjs.org', 'githubusercontent.com'],
      target: { target_type: 'command', kind: 'shell', text: 'npm install', cwd: '/private/tmp/project' } };
    let decided = '';
    await page.route(`**/api/threads/${thread.id}`, route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(snapshot) }));
    await page.route(`**/api/threads/${thread.id}/events`, route => route.abort());
    await page.route(`**/api/threads/${thread.id}/scoped-approvals/*`, route => {
      decided = route.request().postDataJSON().action;
      return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
    });
    await page.goto(app.url);
    const dialog = page.getByRole('dialog', { name: 'Approve agent access' });
    const hosts = dialog.getByRole('list', { name: 'Hosts this command may reach' });
    await expect(hosts.getByRole('listitem')).toHaveText(['registry.npmjs.org', 'objects.githubusercontent.com']);
    await expect(dialog).toContainText('HTTPS only');
    await page.screenshot({ path: info.outputPath('hosts-approval.png') });
    await expect(dialog).toContainText('would cover npmjs.org, githubusercontent.com');
    await dialog.getByRole('button', { name: 'Session, whole domains' }).click();
    expect(decided).toBe('approve_session_domains');
  } finally { await app.close(); }
});

// Requirement change 2026-09-21: capabilities moved out of this dialog. The agent's settings say what
// is allowed and the composer's Tools dropdown switches it (tests/browser/capabilities.spec.ts).
test('the conversation permissions dialog keeps rules only and lists no switch among them',
  async ({ page }) => {
    const app = await launchApp();
    try {
      const { thread } = await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' } } }), 201);
      await json(await app.request('/api/permissions/agent/00000000-0000-0000-0000-000000000000', { method: 'POST',
        body: { target: { target_type: 'feature', name: 'network' }, effect: 'allow' } }));
      await page.goto(app.url);
      await page.getByRole('button', { name: 'Conversation permissions' }).click();
      const settings = page.getByRole('dialog', { name: 'Conversation permissions' });
      await expect(settings.getByText('Capabilities are switched from Tools')).toBeVisible();
      await expect(settings.getByRole('switch')).toHaveCount(0);
      // The switch the main agent allows is off until the conversation switches it on, and it is not listed among the resource rules.
      const saved = await json(await app.request(`/api/permissions/conversation/${thread.id}`));
      expect(saved.effective.find((row: any) => row.target.target_type === 'feature').decision).toBe('ask');
      await expect(settings.getByRole('listitem').filter({ hasText: 'network' })).toHaveCount(0);
    } finally { await app.close(); }
  });
