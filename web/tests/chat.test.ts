import { render, screen, within, fireEvent, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../src/App.svelte';

class Feed extends EventTarget {
  static CLOSED = 2;
  static current: Feed;
  readyState = 1;
  close = vi.fn();
  constructor(public url: string) {
    super();
    Feed.current = this;
  }
  emit(type: string, data: unknown) {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
}

let signedIn = true;
let allowed: boolean | null = null;
const send = vi.fn();
const auth = vi.fn();
const task = { id: 'task-1', name: 'New chat', model: 'test-model' };
const snapshot = { thread: task, messages: [], activeTurn: null,
  pending: false, eventCursor: 'epoch:0' };

beforeEach(() => {
  signedIn = true;
  allowed = null;
  send.mockReset().mockResolvedValue(Response.json({ turnId: 'turn-1' }));
  auth.mockReset().mockResolvedValue(Response.json({
    status: 'pending', authUrl: 'https://auth.openai.com/authorize',
    attemptId: 'login-1',
  }));
  vi.stubGlobal('EventSource', Feed);
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (url === '/api/health') return Response.json({ backend: 'ok' });
    if (url === '/api/projects') return Response.json({ projects: [] });
    if (url === '/api/account') return Response.json({
      status: signedIn ? 'signed_in' : 'signed_out',
      email: signedIn ? 'reader@example.test' : null, planType: 'plus',
    });
    if (url === '/api/usage') return Response.json({
      status: 'unavailable', buckets: [], ordinaryUsageAllowed: allowed,
    });
    if (url === '/api/auth/start') return auth();
    if (url === '/api/auth/status') return Response.json({ status: 'pending' });
    if (url === '/api/threads/working') return Response.json({ threads: [] });
    if (url === '/api/threads') return Response.json(init?.method === 'POST'
      ? { thread: task } : { threads: [], nextCursor: null });
    if (url.endsWith('/messages')) return send();
    if (url.endsWith('/interrupt')) return Response.json({});
    return Response.json(snapshot);
  }));
});

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

// A terminal SSE failure must recover without navigation, draft loss or resending.
test.each(['closed', 'retrying'])('recovers a %s stream without losing state', async mode => {
  const original = fetch;
  vi.stubGlobal('fetch', vi.fn((url: string, init?: RequestInit) => {
    if (url === '/api/threads' && init?.method !== 'POST') {
      return Promise.resolve(Response.json({ threads: [task], nextCursor: null }));
    }
    return original(url, init);
  }));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Retain this draft' } });
  const previous = Feed.current;
  previous.dispatchEvent(new Event('open'));
  previous.readyState = mode === 'closed' ? Feed.CLOSED : 0;
  previous.dispatchEvent(new Event('error'));
  await fireEvent(window, new Event('focus'));
  if (mode === 'retrying') previous.dispatchEvent(new Event('open'));
  await waitFor(() => expect(Feed.current).not.toBe(previous));
  expect(composer).toBeDisabled();
  const current = Feed.current;
  current.dispatchEvent(new Event('open'));
  expect(composer).toBeDisabled();
  current.emit('snapshot', { ...snapshot,
    messages: [{ id: 'saved', role: 'assistant', text: 'Recovered history' }] });
  await waitFor(() => expect(composer).toBeEnabled());
  expect(composer).toHaveValue('Retain this draft');
  previous.emit('message.delta', { itemId: 'stale', delta: 'Wrong stream' });
  expect(screen.queryByText('Wrong stream')).not.toBeInTheDocument();
  expect(screen.getByRole('article', { name: 'Assistant message' }))
    .toHaveTextContent('Recovered history');
  expect(send).not.toHaveBeenCalled();
});

test('offers ChatGPT sign-in', async () => {
  signedIn = false;
  vi.spyOn(window, 'open').mockReturnValue(null);
  render(App);
  await fireEvent.click(await screen.findByRole('button', { name: /^Sign in$/ }));
  expect(auth).toHaveBeenCalledOnce();
  expect(await screen.findByRole('link', { name: 'Continue sign-in' }))
    .toHaveAttribute('href', 'https://auth.openai.com/authorize');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBeDisabled();
});

test('locks composer during send', async () => {
  send.mockReturnValue(new Promise(() => {}));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Hello' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  expect(composer).toBeDisabled();
});

test('appends text and finishes', async () => {
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Hello' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  Feed.current.emit('turn.started', { turnId: 'turn-1' });
  Feed.current.emit('message.delta', { itemId: 'reply-1', delta: 'Hi' });
  Feed.current.emit('message.delta', { itemId: 'reply-1', delta: ' there' });
  expect(await screen.findByRole('article', { name: 'Assistant message' }))
    .toHaveTextContent('Hi there');
  Feed.current.emit('message.completed', { itemId: 'reply-1', text: 'Hello there' });
  Feed.current.emit('turn.completed', { turnId: 'turn-1', status: 'completed' });
  await waitFor(() => expect(composer).toBeEnabled());
  expect(screen.getByRole('article', { name: 'Assistant message' }))
    .toHaveTextContent('Hello there');
});

test('tool activity appears live under its turn, updates in place and expands to a diff', async () => {
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Edit it' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  Feed.current.emit('turn.started', { turnId: 'turn-1' });
  const entry = { id: 'call-1', turnId: 'turn-1', kind: 'edit', title: '/work/src/main.rs', detail: null,
    agent: null, status: 'running', startedAt: 1000 };
  Feed.current.emit('activity', entry);
  const list = await screen.findByRole('group', { name: 'Activity' });
  expect(list).toHaveTextContent('main.rs');
  expect(list).toHaveTextContent('Running');
  Feed.current.emit('activity', { ...entry, status: 'ok', endedAt: 1900,
    diff: '@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n' });
  await waitFor(() => expect(list).not.toHaveTextContent('Running'));
  expect(within(list).getAllByRole('button', { name: /main\.rs/ })).toHaveLength(1);
  await fireEvent.click(within(list).getByRole('button', { name: /main\.rs/ }));
  const removed = await within(list).findByText('-fn old() {}');
  expect(removed.className).toContain('diff-removed');
  expect(within(list).getByText('+fn new() {}').className).toContain('diff-added');
  Feed.current.emit('message.delta', { itemId: 'reply-1', turnId: 'turn-1', delta: 'Done' });
  const reply = await screen.findByRole('article', { name: 'Assistant message' });
  expect(list.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test('a delegation asks wait or background, then shows a named block with the run, its answer and a way out', async () => {
  const calls: [string, unknown][] = [];
  const base = fetch;
  let requests: unknown[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (url === '/api/threads/working') return Response.json({ threads: [], delegatedRequests: requests });
    if (init?.method === 'POST' && /delegations|scoped-approvals/.test(url)) {
      calls.push([url, JSON.parse(String(init.body))]);
      return Response.json(url.endsWith('/detach') ? { thread: 'run-thread' } : {});
    }
    if (url === '/api/threads/run-thread') return Response.json({ ...snapshot, thread: { id: 'run-thread', name: 'Run' },
      messages: [{ id: 'm1', role: 'user', text: 'Review the notes.', complete: true },
        { id: 'm2', role: 'assistant', text: 'Two typos.', complete: true }] });
    return base(url, init);
  }));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Ship it' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  Feed.current.emit('turn.started', { turnId: 'turn-1' });
  const run = { id: 'run-1', parent: 'task-1', turnId: 'turn-1', callId: 'c1', thread: null, agentId: 'a2',
    agent: 'Reviewer', task: 'Review the notes.', mode: '', status: 'asking', result: null, started: 1000, ended: null, detached: false };
  Feed.current.emit('delegation', run);
  Feed.current.emit('delegation.prompt', { id: 'run-1', agent: 'Reviewer', task: 'Review the notes.', expiresInSeconds: 120 });
  const prompt = await screen.findByRole('alertdialog', { name: 'Delegate to Reviewer?' });
  expect(prompt).toHaveTextContent('Review the notes.');
  await fireEvent.click(within(prompt).getByRole('button', { name: 'Run in background' }));
  expect(calls[0]).toEqual(['/api/threads/task-1/delegations/run-1', { mode: 'background' }]);
  Feed.current.emit('delegation.prompt', null);
  Feed.current.emit('delegation', { ...run, mode: 'background', status: 'running', thread: 'run-thread' });
  const block = await screen.findByRole('group', { name: 'Delegated to Reviewer' });
  expect(block).toHaveTextContent('Running in the background');
  // The delegated agent's own tool calls sit inside its block, not in the parent's list.
  Feed.current.emit('activity', { id: 'x1', turnId: 'turn-1', runId: 'run-1', agent: 'Reviewer', kind: 'read',
    title: '/work/notes.md', status: 'ok', startedAt: 1, endedAt: 2 });
  await waitFor(() => expect(within(block).getByRole('group', { name: 'Activity' })).toHaveTextContent('notes.md'));
  expect(screen.getAllByRole('group', { name: 'Activity' })).toHaveLength(1);

  // Its approval is answered here, names the agent, and goes to the run's own thread.
  requests = [{ runId: 'run-1', parent: 'task-1',
    thread: 'run-thread', agent: 'Reviewer', kind: 'scoped', expiresAt: Date.now() + 60000,
    request: { id: 'ap-1', expiresInSeconds: 60, target: { target_type: 'file', path: '/etc/hosts', action: 'read' } } }];
  Feed.current.emit('delegation.requests', { runId: 'run-1', requests });
  const approval = await screen.findByRole('dialog', { name: 'Approve agent access' });
  expect(approval).toHaveTextContent('Reviewer');
  await fireEvent.click(within(approval).getByRole('button', { name: /Reject|Deny/ }));
  await waitFor(() => expect(calls[1]).toEqual(['/api/threads/run-thread/scoped-approvals/ap-1', { action: 'reject' }]));

  Feed.current.emit('delegation', { ...run, mode: 'background', status: 'done', thread: 'run-thread',
    result: 'Two typos on line 4.', ended: 9000 });
  const answer = await within(block).findByRole('article', { name: 'Answer from Reviewer' });
  expect(answer).toHaveTextContent('Two typos on line 4.');
  await fireEvent.click(within(block).getByRole('button', { name: 'Show the full run' }));
  expect(await within(block).findByText('Two typos.')).toBeInTheDocument();
  await fireEvent.click(within(block).getByRole('button', { name: 'Continue with Reviewer' }));
  await waitFor(() => expect(calls.at(-1)).toEqual(['/api/delegations/run-1/detach', {}]));
});

test('a coding session shows each change for review: diff, apply, edit before applying, reject with a note', async () => {
  const calls: [string, unknown][] = [];
  const base = fetch;
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === 'POST' && /reviews|permissions/.test(url)) { calls.push([url, JSON.parse(String(init.body))]); return Response.json({}); }
    return base(url, init);
  }));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Fix it' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  const review = { id: 'rev-1', path: '/work/src/main.rs', kind: 'edit', expiresInSeconds: 1800,
    diff: '@@ -1,2 +1,2 @@\n-fn old_name() {}\n+fn new_name() {}\n fn keep() {}\n', proposed: 'fn new_name() {}\nfn keep() {}\n' };
  Feed.current.emit('snapshot', { ...snapshot, activeTurn: { id: 'turn-1' }, codingSession: true, review });
  // The lit toggle is the indicator; the strip above the composer was removed at the user's request.
  // Requirement change 2026-09-21: the coding session is a switch in the Tools dropdown.
  await fireEvent.click(await screen.findByRole('button', { name: 'Tools' }));
  await waitFor(() => expect(screen.getByRole('switch', { name: 'Coding session' })).toHaveAttribute('aria-checked', 'true'));
  await fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  expect(screen.queryByText(/every change is reviewed/)).toBeNull();
  const dialog = await screen.findByRole('dialog', { name: 'Review change to main.rs' });
  expect(dialog).toHaveTextContent('/work/src');
  const table = within(dialog).getByRole('table', { name: /Changes to/ });
  expect(within(table).getByText('old', { selector: 'mark' })).toBeInTheDocument();
  expect(within(table).getByText('new', { selector: 'mark' })).toBeInTheDocument();

  // Reject needs no note, but a note goes with it when written.
  await fireEvent.click(within(dialog).getByRole('button', { name: 'Reject' }));
  await fireEvent.input(within(dialog).getByRole('textbox', { name: 'Why, for the agent (optional)' }), { target: { value: 'Keep the old name.' } });
  await fireEvent.click(within(dialog).getByRole('button', { name: 'Reject change' }));
  await waitFor(() => expect(calls.at(-1)).toEqual(['/api/threads/task-1/reviews/rev-1', { action: 'reject', note: 'Keep the old name.' }]));

  // Editing the proposal sends the user's own text.
  Feed.current.emit('snapshot', { ...snapshot, activeTurn: { id: 'turn-1' }, codingSession: true, review: { ...review, id: 'rev-2' } });
  const second = await screen.findByRole('dialog', { name: 'Review change to main.rs' });
  await fireEvent.click(within(second).getByRole('button', { name: 'Edit before applying' }));
  await fireEvent.input(within(second).getByRole('textbox', { name: 'New content of main.rs' }), { target: { value: 'fn mine() {}\n' } });
  await fireEvent.click(within(second).getByRole('button', { name: 'Apply my version' }));
  await waitFor(() => expect(calls.at(-1)).toEqual(['/api/threads/task-1/reviews/rev-2', { action: 'apply', text: 'fn mine() {}\n' }]));

  Feed.current.emit('snapshot', { ...snapshot, activeTurn: { id: 'turn-1' }, codingSession: true, review: { ...review, id: 'rev-3' } });
  const third = await screen.findByRole('dialog', { name: 'Review change to main.rs' });
  await fireEvent.click(within(third).getByRole('button', { name: 'Apply all in this turn' }));
  await waitFor(() => expect(calls.at(-1)).toEqual(['/api/threads/task-1/reviews/rev-3', { action: 'apply_all' }]));
});

test('preserves a rejected draft', async () => {
  send.mockResolvedValue(Response.json({ error: {
    message: 'Usage limit reached', outcomeUnknown: false,
  } }, { status: 429 }));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Keep this draft' } });
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Usage limit reached');
  expect(composer).toHaveValue('Keep this draft');
});

test('Enter respects usage denial', async () => {
  allowed = false;
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Wait for access' } });
  await waitFor(() => expect(screen.getByRole('button', { name: /^Send$/ }))
    .toBeDisabled());
  await fireEvent.keyDown(composer, { key: 'Enter' });
  await new Promise(resolve => setTimeout(resolve, 50));
  expect(send).not.toHaveBeenCalled();
  expect(composer).toHaveValue('Wait for access');
});

test('background delegation opens here and answers its source chat', async () => {
  const original = fetch;
  let pending = false;
  const request = vi.fn((url: string, init?: RequestInit) => {
    if (url === '/api/threads/working') return Promise.resolve(Response.json({ threads: ['other-chat'],
      delegationPrompts: pending ? [{ id: 'handoff', thread: 'other-chat', threadName: 'Background project',
        agent: 'Reviewer', task: 'Check the changes.', expiresInSeconds: 120, expiresAt: Date.now() + 120000 }] : [] }));
    if (url === '/api/threads/other-chat/delegations/handoff') {
      pending = false; return Promise.resolve(Response.json({}));
    }
    return original(url, init);
  });
  vi.stubGlobal('fetch', request);
  render(App);
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Keep my draft' } });
  pending = true;
  window.dispatchEvent(new Event('focus'));
  const dialog = await screen.findByRole('alertdialog', { name: 'Delegate to Reviewer?' });
  expect(dialog).toHaveTextContent('Background project');
  await fireEvent.click(within(dialog).getByRole('button', { name: 'Decline' }));
  await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(request).toHaveBeenCalledWith('/api/threads/other-chat/delegations/handoff', expect.objectContaining({ body: JSON.stringify({ mode: 'reject' }) }));
  expect(composer).toHaveValue('Keep my draft');
});

// Reopening an idle parent restores requests without a child stream or a running parent turn.
test.each(['computer', 'review', 'question'])('answers delegated %s requests in the parent', async kind => {
  const base = fetch;
  const request = kind === 'computer' ? { id: 'child-request', message: 'Allow Calculator?', expiresInSeconds: 45 }
    : kind === 'review' ? { id: 'child-request', path: '/work/note.txt', kind: 'write', diff: '-old\n+new', proposed: 'new', expiresInSeconds: 120 }
    : { id: 'child-request', expiresInSeconds: 120, questions: [{ question: 'Which format?', header: 'Format', multiSelect: false,
      options: [{ label: 'Text', description: 'Plain text' }, { label: 'JSON', description: 'Structured' }] }] };
  const pending = { runId: 'child-run', parent: 'task-1', thread: 'child-thread', agent: 'Reviewer',
    kind, request, expiresAt: Date.now() + 45000 };
  const calls: [string, unknown][] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (url === '/api/threads/working') return Response.json({ threads: [], delegatedRequests: calls.length ? [] : [pending] });
    if (url === '/api/threads' && init?.method !== 'POST') return Response.json({ threads: [task], nextCursor: null });
    if (url === '/api/threads/task-1') return Response.json({ ...snapshot, delegatedRequests: [pending] });
    if (url.startsWith('/api/threads/child-thread/') && init?.method === 'POST') {
      calls.push([url, JSON.parse(init.body as string)]); return Response.json({});
    }
    return base(url, init);
  }));
  render(App);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(composer).toBeEnabled());
  await fireEvent.input(composer, { target: { value: 'Keep the parent draft' } });
  if (kind === 'computer') {
    const approval = await screen.findByRole('region', { name: 'Computer approval' });
    expect(approval).toHaveTextContent('Reviewer');
    await fireEvent.click(within(approval).getByRole('button', { name: 'Allow once' }));
    expect(calls).toEqual([['/api/threads/child-thread/approvals/child-request', { action: 'accept' }]]);
  } else if (kind === 'review') {
    const dialog = await screen.findByRole('dialog', { name: 'Review change to note.txt' });
    expect(dialog).toHaveTextContent('Reviewer');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^Apply change/ }));
    expect(calls).toEqual([['/api/threads/child-thread/reviews/child-request', { action: 'apply' }]]);
  } else {
    const dialog = await screen.findByRole('dialog', { name: 'The agent has a question' });
    expect(dialog).toHaveTextContent('Reviewer');
    await fireEvent.click(within(dialog).getByRole('radio', { name: /Text/ }));
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Send answers' }));
    expect(calls).toEqual([['/api/threads/child-thread/questions/child-request', { answers: [{ selected: ['Text'] }] }]]);
  }
  expect(composer).toHaveValue('Keep the parent draft');
});
