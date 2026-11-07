import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../src/App.svelte';
import { setHost } from '../src/platform.svelte';

class Feed extends EventTarget {
  static current: Feed | undefined;
  close() {}
  constructor() { super(); Feed.current = this; }
  emit(type: string, data: unknown) {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
}
const task = { id: 'task-1', name: 'Calculator', model: null, computerUse: false };
const snapshot = { thread: task, messages: [], activeTurn: null,
  pending: false, approval: null, computerActivity: [] };
const approval = { id: 'approval-1', message: 'Allow Computer Use to use Calculator?',
  app: 'com.apple.calculator' };
let requests: { url: string; body: any }[];
let enabled: boolean;
let switchReply: (() => Promise<Response>) | undefined;
let computerReply: Record<string, unknown>;
let platform: string | undefined;

beforeEach(() => {
  Feed.current = undefined;
  requests = [];
  enabled = false;
  switchReply = undefined;
  computerReply = { available: true };
  platform = undefined;
  vi.stubGlobal('EventSource', Feed);
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    if (body) requests.push({ url, body });
    if (url === '/api/health') return Response.json({ backend: 'ok', ...(platform ? { platform } : {}) });
    if (url === '/api/projects') return Response.json({ projects: [] });
    if (url === '/api/account') return Response.json({ status: 'signed_in' });
    if (url === '/api/computer') return Response.json(computerReply);
    if (url === '/api/models') return Response.json({ models: [] });
    if (url === '/api/threads') {
      if (body) enabled = body.computerUse === true;
      const current = { ...task, computerUse: enabled };
      return Response.json(body ? { thread: current } : {
        threads: requests.some(request => request.url === '/api/threads') ? [current] : [],
      });
    }
    if (url === '/api/usage') return Response.json({ status: 'unavailable', buckets: [] });
    if (url.includes('/approvals/') || url.endsWith('/interrupt')) return Response.json({});
    if (url.endsWith('/messages')) return Response.json({ turnId: 'turn-1' });
    if (url.endsWith('/computer')) {
      if (switchReply) return switchReply();
      enabled = body.enabled;
    }
    return Response.json({ ...snapshot, thread: { ...task, computerUse: enabled } });
  }));
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); setHost('macos'); });


// Opens the composer's Tools dropdown, if it is closed, and returns one capability's switch.
// Requirement change 2026-09-21: Computer Use and the coding session moved into that dropdown.
async function tool(name: string) {
  // The dropdown closes when focus moves to another control, so it is opened whenever the switch is not there.
  if (!screen.queryByRole('switch', { name })) {
    const trigger = await screen.findByRole('button', { name: 'Tools' });
    // The dropdown opens once the page is connected.
    await waitFor(() => expect(trigger).toBeEnabled());
    await fireEvent.click(trigger);
    if (!screen.queryByRole('switch', { name })) await fireEvent.click(trigger);
  }
  return screen.findByRole('switch', { name });
}

async function computerChat() {
  render(App);
  const button = await tool('Computer Use');
  await waitFor(() => expect(button).toBeEnabled());
  await fireEvent.click(button);
  await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Use Calculator' },
  });
  await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(Feed.current).toBeDefined());
  await screen.findByRole('button', { name: 'Stop' });
}

test('computer toggle does not create a separate chat', async () => {
  render(App);
  const button = await tool('Computer Use');
  await waitFor(() => expect(button).toBeEnabled());
  expect(button).toHaveAttribute('aria-checked', 'false');
  expect(screen.queryByRole('button', { name: 'New computer chat' })).not.toBeInTheDocument();
  await fireEvent.click(button);
  expect(button).toHaveAttribute('aria-checked', 'true');
  expect(requests).toHaveLength(0);
});

// The backend looks for the Windows helper in the installed Codex app each time; when it cannot use one,
// the Tools menu shows the backend's reason rather than a generic note.
test('on Windows an unavailable helper shows the backend reason', async () => {
  platform = 'windows';
  const reason = 'The installed Codex app has no Computer Use helper. In the Codex app open Plugins > Computer Use and install it; miniclaw guide computer-use shows how.';
  computerReply = { available: false, host: false, reason };
  render(App);
  const button = await tool('Computer Use');
  await waitFor(() => expect(button).toHaveAccessibleDescription(reason));
  expect(button).toBeDisabled();
});

test('computer mode applies to the prompt and locks during a response', async () => {
  await computerChat();
  expect(requests.find(request => request.url === '/api/threads')?.body)
    .toEqual({ computerUse: true, environment: { kind: 'host' } });
  expect(await tool('Computer Use')).toBeDisabled();
});

test('computer mode can be switched off in the same chat', async () => {
  await computerChat();
  Feed.current!.emit('turn.completed', { turnId: 'turn-1', status: 'completed' });
  const toggle = await tool('Computer Use');
  await waitFor(() => expect(toggle).toBeEnabled());
  await fireEvent.click(toggle);
  await waitFor(() => expect(requests.at(-1)).toEqual({
    url: '/api/threads/task-1/computer', body: { enabled: false },
  }));
  await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'false'));
  expect(requests.filter(request => request.url === '/api/threads')).toHaveLength(1);
});

test('approval leaves Stop reachable', async () => {
  await computerChat();
  Feed.current!.emit('snapshot', { ...snapshot, approval, activeTurn: { id: 'turn-1' },
    thread: { ...task, computerUse: true } });
  expect(await screen.findByText(approval.message)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Allow once' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled();
  await fireEvent.click(screen.getByRole('button', { name: 'Decline' }));
  await waitFor(() => expect(requests.at(-1)).toEqual({
    url: '/api/threads/task-1/approvals/approval-1', body: { action: 'decline' },
  }));
});

test('completion clears the approval', async () => {
  await computerChat();
  Feed.current!.emit('snapshot', { ...snapshot, approval, activeTurn: { id: 'turn-1' },
    thread: { ...task, computerUse: true } });
  await screen.findByRole('button', { name: 'Allow once' });
  Feed.current!.emit('turn.completed', { turnId: 'turn-1', status: 'completed' });
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Allow once' }))
    .not.toBeInTheDocument());
});

test('switching keeps the draft, messages, and event stream visible', async () => {
  await computerChat();
  Feed.current!.emit('snapshot', { ...snapshot, thread: { ...task, computerUse: true },
    messages: [{ id: 'answer', role: 'assistant', text: 'Keep this answer.' }] });
  const feed = Feed.current;
  const input = screen.getByRole('textbox', { name: 'Message' });
  await fireEvent.input(input, { target: { value: 'Keep my draft' } });
  let finish!: (response: Response) => void;
  switchReply = () => new Promise(resolve => { finish = resolve; });
  await fireEvent.click(await tool('Computer Use'));
  expect(input).toBeEnabled();
  expect(input).toHaveValue('Keep my draft');
  expect(screen.getByText('Keep this answer.')).toBeVisible();
  expect(screen.queryByText('Responding...')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  finish(Response.json({ ...snapshot, messages: [
    { id: 'answer', role: 'assistant', text: 'Keep this answer.' },
  ] }));
  const after = await tool('Computer Use');
  await waitFor(() => expect(after).toHaveAttribute('aria-checked', 'false'));
  expect(Feed.current).toBe(feed);
  expect(input).toHaveValue('Keep my draft');
});

test('new chat waits for Send before creating a runtime task', async () => {
  render(App);
  const button = screen.getAllByRole('button', { name: 'New chat' })[0];
  await waitFor(() => expect(button).toBeEnabled());
  await fireEvent.click(button);
  expect(requests).toHaveLength(0);
  expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
});

test('restart recovery failures show the backend explanation', async () => {
  const original = vi.mocked(fetch).getMockImplementation()!;
  vi.mocked(fetch).mockImplementation((input, init) => input === '/api/threads'
    ? Promise.resolve(Response.json({ error: { code: 'computer_recovery',
      message: 'Could not restore a chat after an interrupted tool switch.' } }, { status: 503 }))
    : original(input, init));
  render(App);
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not restore a chat');
});
