import { render, screen, fireEvent } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../src/App.svelte';

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) =>
    url === '/api/health' ? fetchMock(url, init) :
      // Health tests keep unrelated account state valid and inactive.
      url === '/api/account' ? Promise.resolve(Response.json({ status: 'unsupported_auth' })) :
      Promise.resolve(new Response('', { status: 503 })));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

test('shows checking', () => {
  fetchMock.mockReturnValue(new Promise(() => {}));
  render(App);

  expect(screen.getByRole('status')).toHaveTextContent('Checking');
  expect(screen.getByRole('button', {
    name: 'Check connection',
  })).toBeDisabled();
});

test('shows connected', async () => {
  fetchMock.mockResolvedValue(Response.json({ backend: 'ok' }));
  render(App);

  expect(await screen.findByText('Backend connected'))
    .toBeVisible();
  expect(screen.getByRole('region', { name: 'Conversation' }))
    .toBeVisible();
  expect(fetchMock).toHaveBeenCalledWith(
    '/api/health',
    expect.objectContaining({ cache: 'no-store' }),
  );
  expect(screen.getByRole('button', { name: /^Send$/ }))
    .toBeDisabled();
  expect(screen.queryByText('Codex connected')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /sign in/i }))
    .not.toBeInTheDocument();
});

test('waiting worker recovers automatically without remounting', async () => {
  vi.useFakeTimers();
  fetchMock.mockResolvedValueOnce(Response.json({
    backend: 'ok', codex: 'unavailable', startup: 'waiting_for_worker',
  }));
  fetchMock.mockResolvedValue(Response.json({ backend: 'ok', codex: 'connected' }));
  const view = render(App);
  await vi.advanceTimersByTimeAsync(0);
  expect(screen.getByRole('status')).toHaveTextContent('Waiting for Master VM');
  expect(screen.getByRole('button', { name: /^Send$/ })).toBeDisabled();
  await vi.advanceTimersByTimeAsync(3000);
  expect(screen.getByRole('status')).toHaveTextContent('Backend connected');
  view.unmount();
  const count = fetchMock.mock.calls.length;
  await vi.advanceTimersByTimeAsync(3000);
  expect(fetchMock).toHaveBeenCalledTimes(count);
});

test.each([
  ['HTTP error', () => new Response('', { status: 503 })],
  ['wrong payload', () => Response.json({ backend: 'broken' })],
  ['invalid JSON', () => new Response('<html>Not JSON</html>')],
])('rejects %s', async (_, reply) => {
  fetchMock.mockResolvedValue(reply());
  render(App);

  expect(await screen.findByRole('alert'))
    .toHaveTextContent('Backend unavailable');
});

test('retries a failed check', async () => {
  fetchMock.mockRejectedValueOnce(new TypeError('Network failure'));
  fetchMock.mockResolvedValueOnce(Response.json({ backend: 'ok' }));
  render(App);

  expect(await screen.findByRole('alert')).toBeVisible();
  await fireEvent.click(screen.getByRole('button', {
    name: 'Check connection',
  }));

  expect(await screen.findByText('Backend connected')).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

test('cancels on unmount', () => {
  fetchMock.mockReturnValue(new Promise(() => {}));
  const view = render(App);
  const options = fetchMock.mock.calls[0][1];

  view.unmount();

  expect(options?.signal?.aborted).toBe(true);
});

test('times out a stalled check', async () => {
  vi.useFakeTimers();
  fetchMock.mockImplementation((_, options) => {
    return new Promise((_, reject) => {
      options?.signal?.addEventListener('abort', () => {
        reject(new DOMException('Aborted', 'AbortError'));
      });
    });
  });
  render(App);

  await vi.advanceTimersByTimeAsync(8000);

  expect(screen.getByRole('alert'))
    .toHaveTextContent('Backend unavailable');
});
