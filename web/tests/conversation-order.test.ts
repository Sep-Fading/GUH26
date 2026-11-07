import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../src/App.svelte';

// The conversation reads in the order of its turns. A turn's Activity and its delegation cards sit
// with that turn, before its first reply or, when it has none (Stop, a failure), after its message.
// A late update (a step finishing, a run ending) changes a card in place and never moves it.

class Feed extends EventTarget {
  static current: Feed | undefined;
  readyState = 1;
  close = vi.fn();
  constructor(public url: string) { super(); Feed.current = this; }
  emit(type: string, data: unknown) {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
}

const task = { id: 'task-1', name: 'Ordered chat', model: 'test-model' };
const other = { id: 'task-2', name: 'Other chat', model: 'test-model' };
type Snapshot = Record<string, unknown>;
let opened: Snapshot;
let otherOpened: Snapshot;
let older: { messages: unknown[]; nextCursor: string | null };
const base = { thread: task, messages: [], activeTurn: null, pending: false, eventCursor: 'epoch:0' };

const user = (id: string, text: string, turnId?: string) => ({ id, role: 'user', text, complete: true, ...(turnId ? { turnId } : {}) });
const reply = (id: string, text: string, turnId: string) => ({ id, role: 'assistant', text, complete: true, turnId });
const step = (id: string, turnId: string, title: string, status = 'ok') => ({ id, turnId, kind: 'command', title,
  detail: null, agent: null, status, startedAt: 1000, ...(status === 'running' ? {} : { endedAt: 1100 }) });
const run = (id: string, turnId: string, agent: string, status = 'running') => ({ id, parent: task.id, turnId, callId: `call-${id}`,
  thread: null, agentId: 'agent-2', agent, task: 'Check the notes.', mode: 'background', status,
  result: status === 'done' ? 'All good.' : null, started: 1000, ended: status === 'done' ? 9000 : null, detached: false });

beforeEach(() => {
  Feed.current = undefined;
  opened = base;
  otherOpened = { ...base, thread: other };
  older = { messages: [], nextCursor: null };
  vi.stubGlobal('EventSource', Feed);
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (url === '/api/health') return Response.json({ backend: 'ok' });
    if (url === '/api/projects') return Response.json({ projects: [] });
    if (url === '/api/account') return Response.json({ status: 'signed_in', email: 'reader@example.test', planType: 'plus' });
    if (url === '/api/usage') return Response.json({ status: 'unavailable', buckets: [] });
    if (url === '/api/threads/working') return Response.json({ threads: [] });
    if (url === '/api/threads') return Response.json(init?.method === 'POST' ? { thread: task } : { threads: [task, other], nextCursor: null });
    if (url.includes('/history?cursor=')) return Response.json(older);
    if (url === `/api/threads/${task.id}`) return Response.json(opened);
    if (url === `/api/threads/${other.id}`) return Response.json(otherOpened);
    return Response.json({});
  }));
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

// What the conversation shows, top to bottom: messages, Activity lists (by their steps) and run cards.
function order() {
  const content = document.querySelector('[data-conversation-content]');
  if (!content) return [];
  return [...content.querySelectorAll('article, [role="group"]')]
    .filter(node => !node.parentElement?.closest('[aria-label^="Delegated to"]'))
    .map(node => {
      const label = node.getAttribute('aria-label') ?? '';
      if (label === 'Activity') return `activity ${[...node.querySelectorAll('li button[title]')].map(item => item.getAttribute('title')).join(',')}`;
      if (label.startsWith('Delegated to')) return `run ${label.slice('Delegated to '.length)}`;
      return `${label === 'Your message' ? 'user' : 'assistant'} ${node.textContent?.trim()}`;
    });
}

// Renders the page and returns the open conversation's event stream.
async function opens() {
  render(App);
  await waitFor(() => expect(Feed.current?.url).toBe(`/api/threads/${task.id}/events`));
  return Feed.current!;
}

test('a turn stopped before any reply keeps its Activity and run card above later messages, live', async () => {
  // A long conversation: older history exists, so a turn missing from the messages could be an old one.
  opened = { ...base, historyCursor: 'older' };
  const feed = await opens();
  // The backend records the message, then the turn starts and says which message started it.
  feed.emit('snapshot', { ...opened, messages: [user('u1', 'First')], pending: true });
  feed.emit('turn.started', { turnId: 'turn-1', messageId: 'u1' });
  feed.emit('activity', step('s1', 'turn-1', 'npm test', 'running'));
  feed.emit('delegation', run('r1', 'turn-1', 'Reviewer'));
  feed.emit('turn.completed', { turnId: 'turn-1', status: 'interrupted' });
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer']));

  // The next message and its reply come after the stopped turn.
  feed.emit('snapshot', { ...opened, messages: [user('u1', 'First', 'turn-1'), user('u2', 'Second')], pending: true });
  feed.emit('turn.started', { turnId: 'turn-2', messageId: 'u2' });
  feed.emit('message.delta', { itemId: 'a2', turnId: 'turn-2', delta: 'Second answer' });
  feed.emit('message.completed', { itemId: 'a2', turnId: 'turn-2', text: 'Second answer' });
  feed.emit('turn.completed', { turnId: 'turn-2', status: 'completed' });
  // Late updates: the command finishes and the delegated agent ends after the newer reply.
  feed.emit('activity', step('s1', 'turn-1', 'npm test'));
  feed.emit('delegation', run('r1', 'turn-1', 'Reviewer', 'done'));
  const expected = ['user First', 'activity npm test', 'run Reviewer', 'user Second', 'assistant Second answer'];
  await waitFor(() => expect(order()).toEqual(expected));
  expect(screen.getByRole('group', { name: 'Delegated to Reviewer' })).toHaveTextContent('All good.');
});

// "Send now" on a queued message stops the running turn and the backend sends that message itself.
test('a message sent now stops the running turn, whose Activity and run card stay above it when they end late', async () => {
  opened = { ...base, historyCursor: 'older' };
  const feed = await opens();
  feed.emit('snapshot', { ...opened, messages: [user('u1', 'First')], pending: true });
  feed.emit('turn.started', { turnId: 'turn-1', messageId: 'u1' });
  feed.emit('message.delta', { itemId: 'a1', turnId: 'turn-1', delta: 'Working on it' });
  feed.emit('activity', step('s1', 'turn-1', 'npm test', 'running'));
  feed.emit('delegation', { ...run('r1', 'turn-1', 'Reviewer'), mode: 'wait' });
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer', 'assistant Working on it']));
  // The interrupted turn ends; the queued message starts the next turn before the step and the run report back.
  feed.emit('turn.completed', { turnId: 'turn-1', status: 'interrupted' });
  feed.emit('snapshot', { ...opened, messages: [user('u1', 'First', 'turn-1'), reply('a1', 'Working on it', 'turn-1'),
    user('u2', 'Urgent')], activities: [step('s1', 'turn-1', 'npm test', 'running')],
    delegations: [{ ...run('r1', 'turn-1', 'Reviewer'), mode: 'wait' }], pending: true });
  feed.emit('turn.started', { turnId: 'turn-2', messageId: 'u2' });
  feed.emit('message.completed', { itemId: 'a2', turnId: 'turn-2', text: 'Urgent answer' });
  feed.emit('activity', step('s1', 'turn-1', 'npm test', 'refused'));
  feed.emit('delegation', { ...run('r1', 'turn-1', 'Reviewer', 'stopped'), mode: 'wait', ended: 9000 });
  feed.emit('turn.completed', { turnId: 'turn-2', status: 'completed' });
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer', 'assistant Working on it',
    'user Urgent', 'assistant Urgent answer']));
  expect(screen.getByRole('group', { name: 'Delegated to Reviewer' })).toHaveTextContent('Did not finish');
});

// The same when nothing was said before Stop: the steps and the card sit under the stopped message.
test('a message sent now after a silent turn keeps that turn\'s cards under its own message', async () => {
  const feed = await opens();
  feed.emit('snapshot', { ...base, messages: [user('u1', 'First')], pending: true });
  feed.emit('turn.started', { turnId: 'turn-1', messageId: 'u1' });
  feed.emit('activity', step('s1', 'turn-1', 'npm test', 'running'));
  feed.emit('delegation', run('r1', 'turn-1', 'Reviewer'));
  feed.emit('turn.completed', { turnId: 'turn-1', status: 'interrupted' });
  // The backend's next snapshot is the one it sent before naming the turn: the page's own naming holds.
  feed.emit('snapshot', { ...base, messages: [user('u1', 'First'), user('u2', 'Urgent')], pending: true,
    activities: [step('s1', 'turn-1', 'npm test', 'running')], delegations: [run('r1', 'turn-1', 'Reviewer')] });
  feed.emit('turn.started', { turnId: 'turn-2', messageId: 'u2' });
  feed.emit('message.completed', { itemId: 'a2', turnId: 'turn-2', text: 'Urgent answer' });
  feed.emit('delegation', run('r1', 'turn-1', 'Reviewer', 'stopped'));
  feed.emit('turn.completed', { turnId: 'turn-2', status: 'completed' });
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer', 'user Urgent', 'assistant Urgent answer']));
});

test('a reopened conversation places each turn\'s Activity and runs by the turn, whatever changed last', async () => {
  opened = { ...base,
    messages: [user('u1', 'First', 'turn-1'), user('u2', 'Second', 'turn-2'), reply('a2', 'Second answer', 'turn-2'),
      user('u3', 'Third', 'turn-3'), reply('a3', 'Third answer', 'turn-3')],
    // Listed in the order they last changed: the stopped turn's run ended last.
    activities: [step('s3', 'turn-3', 'ls'), step('s2', 'turn-2', 'cat notes'), step('s1', 'turn-1', 'npm test', 'refused')],
    delegations: [run('r2', 'turn-2', 'Writer', 'done'), run('r1', 'turn-1', 'Reviewer', 'done')] };
  await opens();
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer',
    'user Second', 'activity cat notes', 'run Writer', 'assistant Second answer',
    'user Third', 'activity ls', 'assistant Third answer']));
});

test('a long conversation keeps older turns\' Activity and runs out of the bottom until their messages load', async () => {
  opened = { ...base, historyCursor: 'older',
    messages: [user('u5', 'Fifth', 'turn-5'), reply('a5', 'Fifth answer', 'turn-5')],
    activities: [step('s1', 'turn-1', 'npm test'), step('s5', 'turn-5', 'ls')],
    delegations: [run('r1', 'turn-1', 'Reviewer', 'done')] };
  older = { messages: [user('u1', 'First', 'turn-1'), reply('a1', 'First answer', 'turn-1')], nextCursor: null };
  await opens();
  await waitFor(() => expect(order()).toEqual(['user Fifth', 'activity ls', 'assistant Fifth answer']));
  await fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }));
  await waitFor(() => expect(order()).toEqual(['user First', 'activity npm test', 'run Reviewer', 'assistant First answer',
    'user Fifth', 'activity ls', 'assistant Fifth answer']));
});

// A background run goes on after its turn ended. While it works it stays in view at the bottom, even when
// its turn is older than the loaded page; once that page is loaded it sits with its turn.
test('a background run still working stays at the bottom while its turn is on an older page', async () => {
  opened = { ...base, historyCursor: 'older',
    messages: [user('u5', 'Fifth', 'turn-5'), reply('a5', 'Fifth answer', 'turn-5')],
    delegations: [run('r1', 'turn-1', 'Reviewer'), run('r0', 'turn-0', 'Writer', 'done')] };
  older = { messages: [user('u1', 'First', 'turn-1'), reply('a1', 'First answer', 'turn-1')], nextCursor: 'oldest' };
  await opens();
  await waitFor(() => expect(order()).toEqual(['user Fifth', 'assistant Fifth answer', 'run Reviewer']));
  await fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }));
  await waitFor(() => expect(order()).toEqual(['user First', 'run Reviewer', 'assistant First answer',
    'user Fifth', 'assistant Fifth answer']));
});

// Which message started which turn is remembered per conversation: another conversation's message with
// the same id is not given that turn.
test('the turn each message started is forgotten when another conversation opens', async () => {
  const feed = await opens();
  feed.emit('snapshot', { ...base, messages: [user('u1', 'First')], pending: true });
  feed.emit('turn.started', { turnId: 'turn-1', messageId: 'u1' });
  feed.emit('turn.completed', { turnId: 'turn-1', status: 'completed' });
  await waitFor(() => expect(order()).toEqual(['user First']));
  otherOpened = { ...base, thread: other, historyCursor: 'older', messages: [user('u1', 'Elsewhere')],
    activities: [step('s1', 'turn-1', 'npm test')] };
  await fireEvent.click(screen.getByRole('button', { name: 'Other chat' }));
  await waitFor(() => expect(order()).toEqual(['user Elsewhere']));
});
