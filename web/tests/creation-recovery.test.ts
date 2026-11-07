// Exercises explicit recovery through accessible controls.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
// Supplies isolated response fixtures and assertions.
import { test, expect, vi, afterEach } from 'vitest';
// Renders the confirmed journal recovery workflow.
import CreationRecovery from '../src/CreationRecovery.svelte';

const preview = { token: 'a'.repeat(64), unknownCount: 4, knownCount: 1,
  quarantined: false, preservedBaselineCount: 0 };
afterEach(() => vi.unstubAllGlobals());

test('recovery requires explicit confirmation and never resends', async () => {
  const fetcher = vi.fn().mockImplementation((_url, init) => Promise.resolve(Response.json(
    init?.method === 'POST' ? { released: 4 } : preview)));
  vi.stubGlobal('fetch', fetcher);
  render(CreationRecovery, { account: 'fixture-owner' });
  await fireEvent.click(screen.getByRole('button', { name: 'Creation recovery' }));
  const action = await screen.findByRole('button', { name: 'Free creation capacity' });
  expect(action).toBeDisabled();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  await fireEvent.click(screen.getByRole('checkbox'));
  await fireEvent.click(action);
  expect(await screen.findByRole('status')).toHaveTextContent('Creation capacity was freed.');
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(1);
  expect(fetcher).toHaveBeenCalledWith('/api/creation-recovery', expect.objectContaining({
    method: 'POST', body: JSON.stringify({ token: preview.token, confirmed: true }),
  }));
  expect(fetcher.mock.calls.every(([url]) => url === '/api/creation-recovery')).toBe(true);
});

test('stale confirmations require a fresh preview and checkbox', async () => {
  vi.stubGlobal('fetch', vi.fn().mockImplementation((_url, init) => Promise.resolve(
    init?.method === 'POST' ? new Response('PRIVATE_SENTINEL', { status: 409 }) : Response.json(preview))));
  render(CreationRecovery, { account: 'fixture-owner' });
  await fireEvent.click(screen.getByRole('button', { name: 'Creation recovery' }));
  await fireEvent.click(await screen.findByRole('checkbox'));
  await fireEvent.click(screen.getByRole('button', { name: 'Free creation capacity' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Refresh the preview');
  expect(document.body.textContent).not.toContain('PRIVATE_SENTINEL');
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh preview' }));
  expect(await screen.findByRole('checkbox')).not.toBeChecked();
});

test('account changes discard an in-flight recovery preview', async () => {
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(resolve => { finish = resolve; })));
  const view = render(CreationRecovery, { account: 'first' });
  await fireEvent.click(screen.getByRole('button', { name: 'Creation recovery' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await view.rerender({ account: undefined });
  finish(Response.json(preview));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.getByRole('button', { name: 'Creation recovery' })).toBeDisabled();
});
