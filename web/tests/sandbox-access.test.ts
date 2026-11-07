// Checks the message shown when Windows changed the settings sandboxed network access relies on.
import { afterEach, expect, test, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { setHost } from '../src/platform.svelte';
import SandboxAccess from '../src/SandboxAccess.svelte';

afterEach(() => { setHost('macos'); vi.unstubAllGlobals(); });

// Answers the sandbox report and the repair request with the given bodies, and records requests.
function backend(report: unknown, repair?: unknown) {
  const requests: { url: string; init?: RequestInit }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: String(url), init });
    return Response.json(String(url).endsWith('/repair') ? repair : report);
  }));
  return requests;
}

test('a failed setup says what happened and offers to set it up again', async () => {
  setHost('windows');
  const requests = backend({ available: true, access: { state: 'failed', reason: 'Cannot open HKLM\\x: Access is denied.' } },
    { ok: true, report: { available: true, access: { state: 'repaired', time: 1_790_000_000 } } });
  render(SandboxAccess);
  const alert = await screen.findByRole('alert', { name: 'Sandbox network access' });
  expect(alert).toHaveTextContent('Network access for sandboxed tools was turned off because Windows changed system settings.');
  expect(alert).toHaveTextContent('Access is denied.');
  await fireEvent.click(screen.getByRole('button', { name: 'Set up again' }));
  expect(await screen.findByText('Network access for sandboxed tools is set up again.')).toBeInTheDocument();
  const repair = requests.find(request => request.url === '/api/sandbox/repair');
  expect(repair?.init?.method).toBe('POST');
  expect(new Headers(repair?.init?.headers).get('X-MiniClaw-Request')).toBe('1');
  expect(screen.queryByRole('button', { name: 'Set up again' })).toBeNull();
});

test('when the service cannot repair it, the elevated command is named', async () => {
  setHost('windows');
  backend({ available: true, access: { state: 'failed', reason: 'x' } },
    { ok: false, reason: 'The MiniClaw network service is not running.', command: 'miniclaw repair', report: { available: false, access: { state: 'failed', reason: 'x' } } });
  render(SandboxAccess);
  await fireEvent.click(await screen.findByRole('button', { name: 'Set up again' }));
  expect(await screen.findByText('The MiniClaw network service is not running.')).toBeInTheDocument();
  expect(screen.getByText('miniclaw repair')).toBeInTheDocument();
  expect(screen.getByRole('alert', { name: 'Sandbox network access' })).toHaveTextContent('administrator approval');
});

test('an automatic repair is reported once, quietly', async () => {
  setHost('windows');
  backend({ available: true, access: { state: 'repaired', time: 1_790_000_000 } });
  render(SandboxAccess);
  const status = await screen.findByRole('status', { name: 'Sandbox network access' });
  expect(status).toHaveTextContent('Windows changed system settings');
  expect(status).toHaveTextContent('set up network access for sandboxed tools again');
  expect(screen.queryByRole('button', { name: 'Set up again' })).toBeNull();
});

test('a stopped service shows why its access is unknown, never an empty box or a repair alert', async () => {
  setHost('windows');
  backend({ available: false, access: { state: 'unknown', reason: 'The MiniClaw network service is not running. Start "MiniClaw network isolation" in Windows Services, or restart Windows.' } });
  render(SandboxAccess);
  const status = await screen.findByRole('status', { name: 'Sandbox network access' });
  expect(status).toHaveTextContent('The MiniClaw network service is not running.');
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Set up again' })).toBeNull();
  expect(status).not.toHaveTextContent('Windows changed system settings');
});

test('nothing is shown when access is healthy, and the Mac never asks', async () => {
  setHost('windows');
  backend({ available: true, access: { state: 'healthy' } });
  const { container, unmount } = render(SandboxAccess);
  await new Promise(resolve => setTimeout(resolve, 10));
  expect(container.textContent?.trim()).toBe('');
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('status')).toBeNull();
  unmount();
  setHost('macos');
  const requests = backend({ available: true });
  const mac = render(SandboxAccess);
  await new Promise(resolve => setTimeout(resolve, 10));
  expect(requests).toEqual([]);
  expect(mac.container.textContent?.trim()).toBe('');
});

test('records of an earlier run pause sandboxed work: the reason is shown and recovery can be tried again', async () => {
  setHost('windows');
  const reason = 'Sandboxed commands, the terminal, Git views and sandboxed MCP servers are paused: MiniClaw could not finish cleaning up after an earlier sandboxed run, and its records and permissions were kept. Windows sandbox recovery retained 1 unresolved records; native job/object cleanup must complete before serving. First: MiniClaw.Run.0a: C:\\Users\\S\\x.txt: Access is denied. Chat still works.';
  const requests = backend({ available: false, recovery: true, reason, access: null },
    { ok: true, report: { available: true, access: { state: 'healthy' } } });
  render(SandboxAccess);
  const alert = await screen.findByRole('alert', { name: 'Sandbox recovery' });
  expect(alert).toHaveTextContent('C:\\Users\\S\\x.txt: Access is denied.');
  expect(alert).toHaveTextContent('Chat still works.');
  await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByText('Sandboxed work is available again.')).toBeInTheDocument();
  expect(requests.some(request => request.url === '/api/sandbox/repair')).toBe(true);
  expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
});

test('a recovery that still cannot finish keeps the reason it gives', async () => {
  setHost('windows');
  backend({ available: false, recovery: true, reason: 'paused: first reason' },
    { ok: false, reason: 'paused: still stuck at C:\\y', command: null, report: { available: false, recovery: true, reason: 'paused: still stuck at C:\\y' } });
  render(SandboxAccess);
  await fireEvent.click(await screen.findByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('alert', { name: 'Sandbox recovery' })).toHaveTextContent('still stuck at C:\\y');
  expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
});
