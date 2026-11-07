// Checks machine choices through visible controls and fake local workers.
import { test, expect } from '@playwright/test';
// Starts disposable MiniClaw backends without live Codex calls.
import { launchApp } from '../support/harness.mjs';
// Opens the composer's Tools dropdown.
import { openTools } from './settings.js';

// Opens task navigation on mobile without changing the desktop path.
async function navigation(page: import('@playwright/test').Page) {
  const toggle = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await toggle.isVisible()) await toggle.click();
  return page.getByRole('navigation', { name: 'Tasks', exact: true });
}

test('project machine choice is visible and locked after saving', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New project', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    await dialog.getByRole('textbox', { name: 'Name' }).fill('Research');
    await dialog.getByRole('radio', { name: 'Virtual machine' }).check();
    await expect(dialog.getByRole('checkbox', { name: 'Use a separate VM' })).toBeEnabled();
    await dialog.getByRole('button', { name: 'Save project' }).click();
    await expect(dialog.getByRole('status')).toHaveText('Saved');
    await expect(dialog.getByRole('radio', { name: 'Virtual machine' })).toBeDisabled();
    await page.screenshot({ path: info.outputPath('project-machine.png') });
    expect((await (await app.request('/api/projects')).json()).projects[0].environment)
      .toEqual({ kind: 'vm', dedicated: false });
  } finally { await app.close(); }
});

test('conversation may use Mac under a VM project and cannot switch later', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    const { project } = await (await app.request('/api/projects', { method: 'POST', body: {
      name: 'VM project', environment: { kind: 'vm', dedicated: false },
    } })).json();
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New chat in VM project' }).click();
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await expect(machine).toContainText('Master VM');
    await machine.click();
    await page.getByRole('option', { name: 'Mac' }).click();
    await page.getByRole('textbox', { name: 'Message' }).fill('Use Mac');
    await page.getByRole('button', { name: 'Send' }).click();
    await expect(page.getByRole('article', { name: 'Your message' })).toHaveText('Use Mac');
    await expect(machine).toBeDisabled();
    await page.screenshot({ path: info.outputPath('conversation-machine.png') });
    const { threads } = await (await app.request('/api/threads')).json();
    expect(threads[0].projectId).toBe(project.id);
    expect(threads[0].environment).toEqual({ kind: 'host' });
  } finally { await app.close(); }
});

test('unavailable VM leaves an unsent draft and shows the cause', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.goto(app.url);
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await machine.click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    await page.getByRole('textbox', { name: 'Message' }).fill('Stay on the VM');
    await page.getByRole('button', { name: 'Send' }).click();
    await expect(page.getByRole('alert')).toContainText('Master VM status is unavailable');
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('Stay on the VM');
    expect((await app.inspect()).requests.filter((r: { method: string }) =>
      r.method === 'thread/start').length).toBe(0);
  } finally { await app.close(); }
});

test('guest desktop stays view-only and sends selected window and shutdown actions',
  async ({ page }, info) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    let state = 'running';
    const actions: string[] = [];
    try {
      // A JPEG start/end marker pair is not a picture: decoding failed after the
      // assertion sometimes passed. Supply a real frame and wait for it to be painted.
      const jpeg = await page.evaluate(() => {
        const canvas = document.createElement('canvas');
        canvas.width = 16; canvas.height = 9;
        canvas.getContext('2d')!.fillRect(0, 0, 16, 9);
        return canvas.toDataURL('image/jpeg').split(',')[1];
      });
      await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
        state, ready: state === 'running',
      } }));
      await page.route('**/api/vms/master/desktop?*', route => route.fulfill({
        body: Buffer.from(jpeg, 'base64'), contentType: 'image/jpeg',
      }));
      await page.route('**/api/vms/master/desktop/*', route => {
        actions.push(route.request().url().split('/').pop()!);
        expect(route.request().headers()['x-miniclaw-request']).toBe('1');
        // The backend's mutation guard refuses a POST without a JSON content type.
        expect(route.request().headers()['content-type']).toBe('application/json');
        if (actions.at(-1) === 'shutdown') state = 'off';
        return route.fulfill({ json: { state } });
      });
      await page.goto(app.url);
      await page.getByRole('combobox', { name: 'Machine' }).click();
      await page.getByRole('option', { name: 'Master VM' }).click();
      const desktop = page.getByRole('complementary', { name: 'Guest desktop' });
      await expect(desktop.getByRole('img', { name: 'Live Master VM desktop' })).toBeVisible();
      await expect.poll(() => desktop.locator('canvas').evaluate(canvas => (canvas as HTMLCanvasElement).width)).toBe(16);
      await page.screenshot({ path: info.outputPath('guest-desktop.png') });
      await desktop.getByRole('button', { name: 'Open desktop window' }).click();
      await desktop.getByRole('button', { name: 'Shut down Master VM' }).click();
      await expect(desktop).toContainText('No desktop signal while this VM is off');
      expect(actions).toEqual(['show', 'shutdown']);
    } finally { await app.close(); }
  });

test('a VM without a picture explains the guest permission and offers its window', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const actions: string[] = [];
  try {
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'running', ready: true,
    } }));
    await page.route('**/api/vms/master/desktop?*', route => route.fulfill({ status: 503,
      json: { error: { code: 'capture_unavailable', message: 'Guest screen capture is unavailable.' } } }));
    await page.route('**/api/vms/master/desktop/*', route => {
      actions.push(route.request().url().split('/').pop()!);
      return route.fulfill({ json: { state: 'running' } });
    });
    await page.goto(app.url);
    await page.getByRole('combobox', { name: 'Machine' }).click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    const desktop = page.getByRole('complementary', { name: 'Guest desktop' });
    await expect(desktop.getByRole('status')).toContainText('No picture from this VM yet');
    await expect(desktop.getByRole('status')).toContainText('Screen Recording');
    await expect(desktop.getByRole('status')).toContainText('Control this VM');
    await page.screenshot({ path: info.outputPath('no-picture.png') });
    await desktop.getByRole('button', { name: 'Open VM window' }).click();
    expect(actions).toEqual(['show']);
  } finally { await app.close(); }
});

test('the Computer Use toggle follows the helper of the VM the chat runs in', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  let computerReady = false;
  try {
    await page.route('**/api/computer', route => route.fulfill({ json: { available: true, host: true } }));
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'running', ready: true, computerReady,
    } }));
    await page.route('**/api/vms/master/desktop?*', route => route.fulfill({ status: 503, json: {} }));
    await page.goto(app.url);
    // The dropdown closes when another control is used, so it is opened again after each machine choice.
    const computerUse = async () => (await openTools(page)).getByRole('switch', { name: 'Computer Use', exact: true });
    // On the Mac the Mac's own helper decides.
    await page.getByRole('combobox', { name: 'Machine' }).click();
    await page.getByRole('option', { name: 'Mac' }).click();
    await expect(await computerUse()).toBeEnabled();
    await page.keyboard.press('Escape');
    await page.getByRole('combobox', { name: 'Machine' }).click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    const toggle = await computerUse();
    await expect(toggle).toBeDisabled();
    await (await openTools(page)).getByText('Computer Use', { exact: true }).hover();
    await expect(page.getByRole('tooltip')).toContainText('miniclaw update-vm --computer-use');
    computerReady = true;
    await expect(toggle).toBeEnabled({ timeout: 10_000 });
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
  } finally { await app.close(); }
});

test('the desktop panel reconnects when its stream ends, so the picture cannot freeze', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  // An 8x8 JPEG; each connection sends one frame and then ends, as a stream does after a VM hiccup.
  const jpeg = Buffer.from('/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAACKADAAQAAAABAAAACAAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgACAAIAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMAAgICAgICAwICAwUDAwMFBgUFBQUGCAYGBgYGCAoICAgICAgKCgoKCgoKCgwMDAwMDA4ODg4ODw8PDw8PDw8PD//bAEMBAgICBAQEBwQEBxALCQsQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEP/dAAQAAf/aAAwDAQACEQMRAD8A9o1v4h/BDw38Pz/wkXwwFjqTWjwJeSThma5ERHmLHErsxDANgAnFfK//AAuH4X/9Az/yXvP/AJHrtf2iv+Ra0X/r5m/9EmvjavWhn+L1/eP73/mfFYbJaFWnGUor5KP/AMif/9k=', 'base64');
  let connections = 0;
  try {
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: { state: 'running', ready: true } }));
    await page.route('**/api/vms/master/desktop?*', route => {
      connections += 1;
      const head = Buffer.from(`--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`);
      return route.fulfill({ contentType: 'multipart/x-mixed-replace; boundary=frame',
        body: Buffer.concat([head, jpeg, Buffer.from('\r\n')]) });
    });
    await page.goto(app.url);
    await page.getByRole('combobox', { name: 'Machine' }).click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    const desktop = page.getByRole('complementary', { name: 'Guest desktop' });
    const picture = desktop.getByRole('img', { name: 'Live Master VM desktop' });
    await expect(picture).toBeVisible();
    // The frame must really decode under the page's Content-Security-Policy; a broken
    // picture is still "visible", so the decoded size is what proves it.
    await expect.poll(() => picture.evaluate(node => node.querySelector('canvas')?.width)).toBe(8);
    await expect.poll(() => connections, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
    await expect(desktop.getByRole('img', { name: 'Live Master VM desktop' })).toBeVisible();
  } finally { await app.close(); }
});

test('older conversation asks once for its machine', async ({ page }) => {
  const app = await launchApp();
  try {
    await page.goto(app.url);
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await expect(machine).toContainText('Choose machine');
    await machine.click();
    await page.getByRole('option', { name: 'Mac' }).click();
    await expect(machine).toContainText('Mac');
    await expect(machine).toBeDisabled();
    await page.reload();
    await expect(machine).toContainText('Mac');
    await expect(machine).toBeDisabled();
  } finally { await app.close(); }
});

test('VM send waits for approval and sends once after Master is ready', async ({ page }, info) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  let started = false;
  let checks = 0;
  let sends = 0;
  let prepared: any;
  try {
    await page.route('**/api/vms/master/status', route => {
      checks += 1;
      return route.fulfill({ json: started && checks > 3
        ? { state: 'running', ready: true }
        : { state: started ? 'starting' : 'off', ready: false } });
    });
    await page.route('**/api/vms/master/prepare', route => {
      prepared = route.request().postDataJSON();
      return route.fulfill({ json: { approvalId: '019a9999-9999-7999-8999-999999999999' } });
    });
    await page.route('**/api/vms/master/start', route => {
      expect(route.request().postDataJSON().approvalId)
        .toBe('019a9999-9999-7999-8999-999999999999');
      started = true;
      return route.fulfill({ status: 202, json: { state: 'starting', ready: false } });
    });
    await page.route('**/api/threads', route => route.request().method() === 'POST'
      ? route.fulfill({ status: 201, json: { thread: {
        id: '019a8888-8888-7888-8888-888888888888', name: 'VM task',
        environment: { kind: 'vm' }, vmId: 'master',
      } } }) : route.continue());
    await page.route('**/api/threads/*/messages', route => {
      sends += 1;
      return route.fulfill({ status: 202, json: { turnId: 'vm-turn' } });
    });
    await page.goto(app.url);
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await machine.click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    await expect(page.locator('footer').getByText('Master VM is off')).toBeVisible();
    await page.getByRole('textbox', { name: 'Message' }).fill('Send once in Master');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Start Master VM and send' })).toBeVisible();
    const prompt = page.getByRole('group', { name: 'Start Master VM' });
    await page.screenshot({ path: info.outputPath('start-prompt.png') });
    await expect(prompt).toContainText('Your message is sent as soon as it is ready');
    // The prompt's buttons use the app's styles, not the browser's defaults.
    expect(await prompt.getByRole('button', { name: 'Start Master VM and send' })
      .evaluate(node => getComputedStyle(node).backgroundColor)).toBe('rgb(29, 29, 31)');
    expect(prepared.environment).toEqual({ kind: 'vm' });
    expect(started).toBe(false);
    expect(sends).toBe(0);
    await page.getByRole('button', { name: 'Start Master VM and send' }).click();
    await expect(page.getByText('Starting Master VM')).toBeVisible();
    await expect.poll(() => sends).toBe(1);
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('');
  } finally { await app.close(); }
});

test('failed VM start shows its cause and retains the waiting message', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  try {
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.route('**/api/vms/master/prepare', route => route.fulfill({ json: {
      approvalId: '019a9999-9999-7999-8999-999999999999',
    } }));
    await page.route('**/api/vms/master/start', route => route.fulfill({ status: 503,
      json: { error: { code: 'vm_start_failed', message: 'VM resource limit exceeded.',
        outcomeUnknown: false } },
    }));
    await page.goto(app.url);
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await machine.click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    await page.getByRole('textbox', { name: 'Message' }).fill('Keep this message');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Start Master VM and send' }).click();
    await expect(page.getByRole('alert')).toHaveText('VM resource limit exceeded.');
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('Keep this message');
    expect((await app.inspect()).requests.filter((r: { method: string }) =>
      r.method === 'thread/start' || r.method === 'turn/start').length).toBe(0);
  } finally { await app.close(); }
});

test('an offline VM task remains visible without opening a dead event stream', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const id = '019a6666-6666-7666-8666-666666666666';
  const offline = { id, name: 'VM task', model: null,
    environment: { kind: 'vm' }, vmId: 'master', vmOffline: true };
  let streams = 0;
  try {
    await page.route('**/api/threads', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { threads: [offline], nextCursor: null } })
      : route.continue());
    await page.route(`**/api/threads/${id}`, route => route.fulfill({ json: {
      thread: offline, messages: [], activeTurn: null, pending: false, vmOffline: true,
    } }));
    await page.route(`**/api/threads/${id}/events`, route => {
      streams += 1;
      return route.fulfill({ status: 503, body: '' });
    });
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.goto(app.url);
    await expect(page.locator('footer').getByText('Master VM is off')).toBeVisible();
    await expect(page.getByText('History loads when Master VM starts.')).toBeVisible();
    expect(streams).toBe(0);
  } finally { await app.close(); }
});

test('an offline VM task shows the history kept on the Mac, then reloads once its VM is ready',
  async ({ page }) => {
    const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
    const id = '019a4444-4444-7444-8444-444444444444';
    const task = { id, name: 'Kept task', model: null, environment: { kind: 'vm' }, vmId: 'master' };
    let ready = false;
    try {
      await page.route('**/api/threads', route => route.request().method() === 'GET'
        ? route.fulfill({ json: { threads: [{ ...task, vmOffline: !ready }], nextCursor: null } })
        : route.continue());
      await page.route(`**/api/threads/${id}`, route => route.fulfill({ json: ready
        ? { thread: task, messages: [{ id: 'm1', role: 'user', text: 'Kept on the Mac', complete: true },
            { id: 'm2', role: 'assistant', text: 'Fresh from the VM', complete: true }],
          activeTurn: null, pending: false, eventCursor: 'e:0', historyCursor: null }
        : { thread: { ...task, vmOffline: true },
          messages: [{ id: 'm1', role: 'user', text: 'Kept on the Mac', complete: true }],
          activeTurn: null, pending: false, vmOffline: true } }));
      await page.route(`**/api/threads/${id}/events*`, route => route.fulfill({ status: 503, body: '' }));
      await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
        state: ready ? 'running' : 'off', ready,
      } }));
      await page.goto(app.url);
      await expect(page.getByText('Kept on the Mac')).toBeVisible();
      await expect(page.getByText('History loads when Master VM starts.')).toHaveCount(0);
      // The VM becomes ready by another route than this page's own start prompt.
      ready = true;
      await expect(page.getByText('Fresh from the VM')).toBeVisible({ timeout: 10_000 });
    } finally { await app.close(); }
  });

test('approved send reloads an offline task before sending once', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const id = '019a5555-5555-7555-8555-555555555555';
  const task = { id, name: 'VM task', model: null,
    environment: { kind: 'vm' }, vmId: 'master' };
  let started = false;
  let checks = 0;
  let sends = 0;
  try {
    await page.route('**/api/threads', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { threads: [{ ...task, vmOffline: !started }], nextCursor: null } })
      : route.continue());
    await page.route(`**/api/threads/${id}`, route => route.fulfill({ json: {
      thread: { ...task, vmOffline: !started }, messages: [], activeTurn: null,
      pending: false, vmOffline: !started,
    } }));
    await page.route('**/api/vms/master/status', route => {
      checks += 1;
      return route.fulfill({ json: started && checks > 3
        ? { state: 'running', ready: true }
        : { state: started ? 'starting' : 'off', ready: false } });
    });
    await page.route('**/api/vms/master/prepare', route => route.fulfill({ json: {
      approvalId: '019a9999-9999-7999-8999-999999999999',
    } }));
    await page.route('**/api/vms/master/start', route => {
      started = true;
      return route.fulfill({ status: 202, json: { state: 'starting', ready: false } });
    });
    await page.route(`**/api/threads/${id}/messages`, route => {
      sends += 1;
      return route.fulfill({ status: 202, json: { turnId: 'vm-turn' } });
    });
    await page.goto(app.url);
    await expect(page.getByText('History loads when Master VM starts.')).toBeVisible();
    await page.getByRole('textbox', { name: 'Message' }).fill('Continue the same task');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Start Master VM and send' }).click();
    await expect.poll(() => sends).toBe(1);
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('');
  } finally { await app.close(); }
});

test('declining VM startup revokes approval and keeps the draft', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  let declined = false;
  let started = false;
  try {
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.route('**/api/vms/master/prepare', route => route.fulfill({ json: {
      approvalId: '019a9999-9999-7999-8999-999999999999',
    } }));
    await page.route('**/api/vms/master/decline', route => {
      declined = true;
      return route.fulfill({ json: {} });
    });
    await page.route('**/api/vms/master/start', route => {
      started = true;
      return route.fulfill({ status: 202, json: { state: 'starting' } });
    });
    await page.goto(app.url);
    const machine = page.getByRole('combobox', { name: 'Machine' });
    await machine.click();
    await page.getByRole('option', { name: 'Master VM' }).click();
    await page.getByRole('textbox', { name: 'Message' }).fill('Do not send yet');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await expect.poll(() => declined).toBe(true);
    expect(started).toBe(false);
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('Do not send yet');
  } finally { await app.close(); }
});

test('dedicated task uses only its project VM control path', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const id = '019a4444-4444-7444-8444-444444444444';
  const vmId = '019a3333-3333-7333-8333-333333333333';
  const task = { id, name: 'Project task', model: null,
    environment: { kind: 'vm' }, vmId, vmOffline: true };
  let masterStarts = 0;
  let projectStarts = 0;
  try {
    await page.route('**/api/threads', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { threads: [task], nextCursor: null } }) : route.continue());
    await page.route(`**/api/threads/${id}`, route => route.fulfill({ json: {
      thread: task, messages: [], activeTurn: null, pending: false, vmOffline: true,
    } }));
    await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.route(`**/api/vms/${vmId}/prepare`, route => route.fulfill({ json: {
      approvalId: '019a2222-2222-7222-8222-222222222222',
    } }));
    await page.route(`**/api/vms/${vmId}/start`, route => {
      projectStarts += 1;
      return route.fulfill({ status: 503, json: { error: {
        code: 'vm_start_failed', message: 'Assigned VM does not exist.',
        outcomeUnknown: false,
      } } });
    });
    await page.route('**/api/vms/master/start', route => {
      masterStarts += 1;
      return route.fulfill({ status: 202, json: { state: 'starting' } });
    });
    await page.goto(app.url);
    await expect(page.getByRole('heading', { name: 'Project VM is off' })).toBeVisible();
    await page.getByRole('textbox', { name: 'Message' }).fill('This project only');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Start Project VM and send' }).click();
    await expect.poll(() => projectStarts).toBe(1);
    expect(masterStarts).toBe(0);
    await expect(page.getByRole('alert')).toHaveText('Assigned VM does not exist.');
  } finally { await app.close(); }
});

test('new task in a dedicated project selects its saved VM', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const vmId = '019a3333-3333-7333-8333-333333333333';
  const project = { id: '019a2222-2222-7222-8222-222222222222',
    name: 'Separate workspace', agentIds: [], skillIds: [], revision: 1,
    environment: { kind: 'vm', dedicated: true, vmId } };
  let projectChecks = 0;
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects: [project] } }) : route.continue());
    await page.route(`**/api/vms/${vmId}/status`, route => {
      projectChecks += 1;
      return route.fulfill({ json: { state: 'off', ready: false } });
    });
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New chat in Separate workspace' }).click();
    await expect(page.getByRole('combobox', { name: 'Machine' })).toContainText('Project VM');
    await expect(page.locator('footer').getByText('Project VM is off')).toBeVisible();
    expect(projectChecks).toBeGreaterThan(0);
  } finally { await app.close(); }
});

test('dedicated project shows install progress and saves only after its VM is ready', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const vmId = '019a3333-3333-7333-8333-333333333333';
  let ready = false;
  let saves = 0;
  try {
    await page.route('**/api/vms/projects', route => route.fulfill({ status: 202,
      json: { vmId, state: 'starting', stage: 'preparing' } }));
    await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: ready
      ? { state: 'running', stage: 'awaiting_setup', percent: 100, ready: true }
      : { state: 'starting', stage: 'installing', percent: 42, ready: false } }));
    await page.route('**/api/projects', route => {
      if (route.request().method() !== 'POST') return route.continue();
      saves += 1;
      const draft = route.request().postDataJSON();
      expect(draft.environment).toEqual({ kind: 'vm', dedicated: true, vmId });
      return route.fulfill({ status: 201, json: { project: {
        id: '019a2222-2222-7222-8222-222222222222',
        ...draft, revision: 1,
      } } });
    });
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New project' }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    await dialog.getByRole('textbox', { name: 'Name' }).fill('Fresh workspace');
    await dialog.getByRole('radio', { name: 'Virtual machine' }).check();
    await dialog.getByRole('checkbox', { name: 'Use a separate VM' }).check();
    await dialog.getByRole('button', { name: 'Save project' }).click();
    await expect(dialog.getByRole('progressbar', { name: 'Project VM setup progress' }))
      .toHaveAttribute('value', '42');
    await expect(dialog.getByText('Installing macOS')).toBeVisible();
    expect(saves).toBe(0);
    ready = true;
    await expect(dialog.getByRole('status', { name: 'Project save status' })).toHaveText('Saved');
    expect(saves).toBe(1);
  } finally { await app.close(); }
});

test('dedicated install failure stays in the project form for retry', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const vmId = '019a3333-3333-7333-8333-333333333333';
  let failed = true;
  let retries = 0;
  let saves = 0;
  try {
    await page.route('**/api/vms/projects', route => route.fulfill({ status: 202,
      json: { vmId, state: 'starting', stage: 'preparing' } }));
    await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: failed
      ? { state: 'failed', stage: 'failed', percent: 42,
          reason: 'macOS installation failed. Its files were kept for retry.', ready: false }
      : { state: 'running', stage: 'awaiting_setup', percent: 100, ready: true } }));
    await page.route(`**/api/vms/${vmId}/retry`, route => {
      retries += 1;
      failed = false;
      return route.fulfill({ status: 202,
        json: { state: 'starting', stage: 'preparing' } });
    });
    await page.route('**/api/projects', route => {
      if (route.request().method() !== 'POST') return route.continue();
      saves += 1;
      return route.fulfill({ status: 201, json: { project: {
        id: '019a2222-2222-7222-8222-222222222222',
        ...route.request().postDataJSON(), revision: 1,
      } } });
    });
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New project' }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    await dialog.getByRole('textbox', { name: 'Name' }).fill('Retry workspace');
    await dialog.getByRole('radio', { name: 'Virtual machine' }).check();
    await dialog.getByRole('checkbox', { name: 'Use a separate VM' }).check();
    await dialog.getByRole('button', { name: 'Save project' }).click();
    await expect(dialog.getByRole('alert')).toContainText('macOS installation failed');
    expect(saves).toBe(0);
    await dialog.getByRole('button', { name: 'Retry VM setup' }).click();
    await expect.poll(() => retries).toBe(1);
    await expect(dialog.getByRole('status', { name: 'Project save status' })).toHaveText('Saved');
    expect(saves).toBe(1);
  } finally { await app.close(); }
});

test('dedicated project offers Codex sign-in on its own guest', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const vmId = '019a3333-3333-7333-8333-333333333333';
  let signedIn = false;
  let logins = 0;
  let saves = 0;
  try {
    await page.route('**/api/vms/projects', route => route.fulfill({ status: 202,
      json: { vmId, state: 'starting', stage: 'preparing' } }));
    await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: {
      state: 'running', stage: 'awaiting_setup', percent: 100,
      workerConnected: true, ready: signedIn,
    } }));
    await page.route(`**/api/vms/${vmId}/auth/start`, route => {
      logins += 1;
      expect(route.request().postDataJSON()).toEqual({ method: 'device' });
      return route.fulfill({ json: { status: 'pending',
        verificationUrl: 'https://auth.openai.com/codex/device', userCode: 'TEST-CODE' } });
    });
    await page.route('**/api/projects', route => {
      if (route.request().method() !== 'POST') return route.continue();
      saves += 1;
      return route.fulfill({ status: 201, json: { project: {
        id: '019a2222-2222-7222-8222-222222222222',
        ...route.request().postDataJSON(), revision: 1,
      } } });
    });
    await page.goto(app.url);
    const nav = await navigation(page);
    await nav.getByRole('button', { name: 'New project' }).click();
    const dialog = page.getByRole('dialog', { name: 'Project settings' });
    await dialog.getByRole('textbox', { name: 'Name' }).fill('Signed-in workspace');
    await dialog.getByRole('radio', { name: 'Virtual machine' }).check();
    await dialog.getByRole('checkbox', { name: 'Use a separate VM' }).check();
    await dialog.getByRole('button', { name: 'Save project' }).click();
    await dialog.getByRole('button', { name: 'Sign in to Codex for this VM' }).click();
    await expect(dialog.getByText('TEST-CODE')).toBeVisible();
    await expect(dialog.getByRole('link', { name: 'Open Codex sign-in' }))
      .toHaveAttribute('href', 'https://auth.openai.com/codex/device');
    expect(logins).toBe(1);
    expect(saves).toBe(0);
    signedIn = true;
    await expect.poll(() => saves).toBe(1);
  } finally { await app.close(); }
});

test('Mac task under a dedicated project shows only a passive project VM indicator', async ({ page }) => {
  const app = await launchApp({ 'thread/list': { result: { data: [], nextCursor: null } } });
  const vmId = '019a3333-3333-7333-8333-333333333333';
  const projectId = '019a2222-2222-7222-8222-222222222222';
  const id = '019a4444-4444-7444-8444-444444444444';
  const project = { id: projectId, name: 'Separate workspace',
    agentIds: [], skillIds: [], revision: 1,
    environment: { kind: 'vm', dedicated: true, vmId } };
  const task = { id, name: 'Mac task', model: null, projectId,
    environment: { kind: 'host' } };
  let starts = 0;
  try {
    await page.route('**/api/projects', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { projects: [project] } }) : route.continue());
    await page.route('**/api/threads', route => route.request().method() === 'GET'
      ? route.fulfill({ json: { threads: [task], nextCursor: null } }) : route.continue());
    await page.route(`**/api/threads/${id}`, route => route.fulfill({ json: {
      thread: task, messages: [], activeTurn: null, pending: false,
    } }));
    await page.route(`**/api/vms/${vmId}/status`, route => route.fulfill({ json: {
      state: 'off', ready: false,
    } }));
    await page.route('**/api/vms/master/status', route => route.fulfill({ json: {
      state: 'running', ready: true,
    } }));
    await page.route('**/api/vms/*/start', route => {
      starts += 1;
      return route.fulfill({ status: 202, json: { state: 'starting' } });
    });
    await page.goto(app.url);
    await expect(page.getByRole('combobox', { name: 'Machine' })).toContainText('Mac');
    await expect(page.getByText('Project VM is off')).toBeVisible();
    await expect(page.getByRole('button', { name: /Start Project VM/ })).toHaveCount(0);
    expect(starts).toBe(0);
  } finally { await app.close(); }
});
