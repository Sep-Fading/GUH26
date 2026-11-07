// User report 2026-10-07 (0.1.5, Windows): pressing Sign in for Claude only waited and nothing
// opened. The page must offer the sign-in page as a link, take the code Claude shows, and cancel.
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import Providers from './ProvidersHarness.svelte';
import App from '../src/App.svelte';
import { Chat } from '../src/chat.svelte';

const page = 'https://claude.com/cai/oauth/authorize?code=true&client_id=c&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&state=s';
let codexSignedIn = true;
let start: () => Response;
let status: () => Response;
const posts: { url: string; body: unknown }[] = [];

class Feed extends EventTarget {
  static CLOSED = 2;
  readyState = 1;
  close = vi.fn();
}

beforeEach(() => {
  codexSignedIn = true;
  posts.length = 0;
  start = () => Response.json({ status: 'pending', provider: 'claude', attemptId: 'a', authUrl: page, codeEntry: true });
  status = () => Response.json({ status: 'pending', provider: 'claude', attemptId: 'a', authUrl: page, codeEntry: true });
  vi.stubGlobal('EventSource', Feed);
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === 'POST') posts.push({ url, body: JSON.parse(String(init.body ?? '{}')) });
    if (url === '/api/account') return Response.json({
      status: codexSignedIn ? 'signed_in' : 'signed_out', email: codexSignedIn ? 'reader@example.test' : null,
      providers: [{ id: 'codex', enabled: true, installed: true, status: codexSignedIn ? 'signed_in' : 'signed_out' },
        { id: 'claude', enabled: true, installed: true, status: 'signed_out' }] });
    if (url === '/api/auth/start') return start();
    if (url === '/api/auth/status') return status();
    if (url === '/api/auth/code' || url === '/api/auth/cancel') return Response.json({});
    return Response.json({ threads: [], nextCursor: null, models: [], agents: [], projects: [], available: false, status: 'unavailable', buckets: [] });
  }));
});

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

// Starts a Claude sign-in from Settings > Providers.
async function startFromSettings() {
  const chat = new Chat();
  render(Providers, { chat });
  await fireEvent.click(await screen.findByRole('button', { name: 'Sign in to Claude Code' }));
  return chat;
}

test('Settings shows the Claude sign-in page as a link and takes the code', async () => {
  const chat = await startFromSettings();
  const link = await screen.findByRole('link', { name: 'Open the Claude sign-in page' });
  expect(link.getAttribute('href')).toBe(page);
  expect(link.getAttribute('target')).toBe('_blank');
  expect(link.getAttribute('rel')).toContain('noopener');
  // Codex is signed in, so the chat is ready; the Claude attempt must still be waiting.
  await chat.refresh();
  expect(chat.login.status).toBe('pending');
  await fireEvent.input(screen.getByRole('textbox', { name: 'Code from Claude' }), { target: { value: ' abc#def ' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Finish sign-in' }));
  await waitFor(() => expect(posts).toContainEqual({ url: '/api/auth/code', body: { code: 'abc#def' } }));
  await chat.cancelLogin();
});

test('a Claude sign-in can be cancelled from Settings', async () => {
  const chat = await startFromSettings();
  await fireEvent.click(await screen.findByRole('button', { name: 'Cancel Claude sign-in' }));
  await waitFor(() => expect(posts.some(post => post.url === '/api/auth/cancel')).toBe(true));
  expect(await screen.findByRole('button', { name: 'Sign in to Claude Code' })).toBeTruthy();
  expect(screen.queryByRole('link', { name: 'Open the Claude sign-in page' })).toBeNull();
});

test('the error the Claude CLI stopped with is shown', async () => {
  start = () => Response.json({ status: 'failed', provider: 'claude', attemptId: 'a', error: 'Login failed: Request failed with status code 403' });
  const chat = await startFromSettings();
  expect(await screen.findByText('Login failed: Request failed with status code 403')).toBeTruthy();
  expect(await screen.findByRole('button', { name: 'Sign in to Claude Code' })).toBeTruthy();
  expect(chat.login.status).toBe('failed');
});

test('the first screen offers the same Claude sign-in link', async () => {
  codexSignedIn = false;
  render(App);
  await fireEvent.click(await screen.findByRole('button', { name: 'Sign in with Claude' }));
  const link = await screen.findByRole('link', { name: 'Open the Claude sign-in page' });
  expect(link.getAttribute('href')).toBe(page);
  expect(screen.getByRole('textbox', { name: 'Code from Claude' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Cancel Claude sign-in' })).toBeTruthy();
});
