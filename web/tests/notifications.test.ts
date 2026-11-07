// Exercises visible notice actions through accessible controls.
import { act, render, screen, fireEvent, waitFor } from '@testing-library/svelte';
// Supplies isolated network responses and assertions.
import { afterEach, expect, test, vi } from 'vitest';
// Renders the global notification management surface.
import Notifications from '../src/Notifications.svelte';

const notice = { id: 'notice-1', kind: 'response_failed', message: 'Codex could not complete this response.',
  threadId: 'thread-1', createdAt: 1900000000, read: false };
afterEach(() => vi.unstubAllGlobals());

test('shows unread notice and persists the read action', async () => {
  const fetcher = vi.fn().mockImplementation((_url, init) => Promise.resolve(Response.json(
    init?.method === 'POST' ? { ok: true } : { notifications: [notice], storageError: false })));
  vi.stubGlobal('fetch', fetcher);
  const openConversation = vi.fn();
  render(Notifications, { account: 'synthetic-owner', onconversation: openConversation });
  await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
  expect(await screen.findByText(notice.message)).toBeVisible();
  await fireEvent.click(screen.getByRole('button', { name: 'Mark as read' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Mark as read' })).toBeNull());
  expect(fetcher).toHaveBeenCalledWith('/api/notifications/notice-1/read', expect.objectContaining({ method: 'POST' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Open conversation' }));
  expect(openConversation).toHaveBeenCalledWith('thread-1');
});

test('failed reads stay unread and show a safe error', async () => {
  vi.stubGlobal('fetch', vi.fn().mockImplementation((_url, init) => Promise.resolve(
    init?.method === 'POST' ? new Response('secret upstream text', { status: 503 })
      : Response.json({ notifications: [notice] }))));
  render(Notifications, { account: 'synthetic-owner', onconversation: vi.fn() });
  await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
  await fireEvent.click(await screen.findByRole('button', { name: 'Mark as read' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not mark this notice as read. Try again.');
  expect(screen.getByRole('button', { name: 'Mark as read' })).toBeEnabled();
  expect(document.body.textContent).not.toContain('secret upstream text');
});

test('account changes discard in-flight notification content', async () => {
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(resolve => { finish = resolve; })));
  const view = render(Notifications, { account: 'first-owner', onconversation: vi.fn() });
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await view.rerender({ account: undefined });
  finish(Response.json({ notifications: [notice] }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Notifications' })).toBeDisabled());
  expect(document.body.textContent).not.toContain(notice.message);
});

test('a rejected account refresh clears previously loaded notices', async () => {
  let allowed = true;
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(allowed
    ? Response.json({ notifications: [notice] }) : new Response('', { status: 401 }))));
  render(Notifications, { account: 'synthetic-owner', onconversation: vi.fn() });
  await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
  expect(await screen.findByText(notice.message)).toBeVisible();
  allowed = false;
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh notifications' }));
  await screen.findByRole('alert');
  expect(screen.queryByText(notice.message)).toBeNull();
});

test.each([{}, { notifications: [null] }, { notifications: [{ ...notice, createdAt: 1e100 }] }])(
  'malformed notice responses show a retryable error', async result => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(Response.json(result))));
    render(Notifications, { account: 'synthetic-owner', onconversation: vi.fn() });
    await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load notifications. Try again.');
    expect(screen.getByRole('button', { name: 'Refresh notifications' })).toBeEnabled();
  });

test.each([true, false])('clear read waits for acknowledgement, success=%s', async success => {
  const read = { ...notice, id: 'read-notice', message: 'A previous failure.', read: true };
  let finish!: (response: Response) => void;
  const fetcher = vi.fn((url: string) => url === '/api/notifications/clear-read'
    ? new Promise<Response>(resolve => { finish = resolve; })
    : Promise.resolve(Response.json({ notifications: [notice, read] })));
  vi.stubGlobal('fetch', fetcher);
  render(Notifications, { account: 'synthetic-owner', onconversation: vi.fn() });
  await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
  const clear = await screen.findByRole('button', { name: 'Clear read' });
  await waitFor(() => expect(clear).toBeEnabled());
  await fireEvent.click(clear);
  expect(clear).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Mark all as read' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Mark as read' })).toBeDisabled();
  expect(screen.getByText(read.message)).toBeVisible();
  expect(fetcher).toHaveBeenCalledWith('/api/notifications/clear-read', expect.objectContaining({
    method: 'POST', headers: expect.objectContaining({ 'X-MiniClaw-Request': '1' }),
  }));
  finish(success ? Response.json({ cleared: 1 }) : new Response('PRIVATE_SENTINEL', { status: 503 }));
  if (success) {
    await waitFor(() => expect(screen.queryByText(read.message)).toBeNull());
    expect(clear).toBeDisabled();
  } else {
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not clear read notifications. Try again.');
    expect(screen.getByText(read.message)).toBeVisible();
    expect(clear).toBeEnabled();
    expect(document.body.textContent).not.toContain('PRIVATE_SENTINEL');
  }
  expect(screen.getByText(notice.message)).toBeVisible();
  expect(screen.getByText(/1 unread\./)).toBeVisible();
});

test('a pending clear cannot remove the next account\'s notices', async () => {
  const read = { ...notice, read: true };
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn((url: string) => url === '/api/notifications/clear-read'
    ? new Promise<Response>(resolve => { finish = resolve; })
    : Promise.resolve(Response.json({ notifications: [read] }))));
  const view = render(Notifications, { account: 'first-owner', onconversation: vi.fn() });
  await fireEvent.click(screen.getByRole('button', { name: 'Notifications' }));
  const clear = await screen.findByRole('button', { name: 'Clear read' });
  await waitFor(() => expect(clear).toBeEnabled());
  await fireEvent.click(clear);
  await view.rerender({ account: 'next-owner' });
  await waitFor(() => expect(clear).toBeEnabled());
  await act(() => finish(Response.json({ cleared: 1 })));
  await waitFor(() => expect(screen.getByText(notice.message)).toBeVisible());
  expect(clear).toBeEnabled();
});
