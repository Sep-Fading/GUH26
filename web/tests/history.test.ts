import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { Chat } from '../src/chat.svelte';

class Feed extends EventTarget {
  static current: Feed;
  close = vi.fn();
  constructor(public url: string) { super(); Feed.current = this; }
  emit(name: string, data: unknown) {
    this.dispatchEvent(new MessageEvent(name, { data: JSON.stringify(data) }));
  }
}
const first = { id: 'task-1', name: 'First', model: null };
const second = { id: 'task-2', name: 'Second', model: null };
const recent = { id: 'recent', role: 'assistant', text: 'Recent answer' };
const old = { id: 'old', role: 'assistant', text: 'Older answer' };
let offline = false;
let read: ReturnType<typeof vi.fn>;
const snapshot = (task = first) => ({ thread: task, messages: [recent],
  activeTurn: null, pending: false, historyCursor: 'older' });

beforeEach(() => {
  localStorage.clear();
  offline = false;
  vi.stubGlobal('EventSource', Feed);
  read = vi.fn(async (path: string) => {
    if (offline) throw new Error('Offline');
    if (path === '/api/account') return Response.json({ status: 'signed_in', email: 'test@example.test' });
    if (path === '/api/computer') return Response.json({ available: false });
    if (path === '/api/models') return Response.json({ models: [] });
    if (path === '/api/usage') return Response.json({ status: 'unavailable', buckets: [] });
    if (path === '/api/threads') return Response.json({ threads: [first], nextCursor: 'next' });
    if (path === '/api/threads?cursor=next') return Response.json({ threads: [second], nextCursor: null });
    if (path.endsWith('/history?cursor=older')) return Response.json({ messages: [old, recent], nextCursor: null });
    return Response.json(snapshot(path.includes(second.id) ? second : first));
  });
  vi.stubGlobal('fetch', read);
});
afterEach(() => { localStorage.clear(); vi.unstubAllGlobals(); });

test('reload restores selection across list pages', async () => {
  const firstPage = new Chat();
  await firstPage.refresh();
  await firstPage.select(second);
  const reloaded = new Chat();
  await reloaded.refresh();
  expect(reloaded.tasks.map(task => task.id)).toEqual([first.id, second.id]);
  expect(reloaded.active?.id).toBe(second.id);
});

test('older messages survive live snapshots without duplicates', async () => {
  const chat = new Chat();
  await chat.refresh();
  await chat.loadOlder();
  expect(chat.messages.map(message => message.text)).toEqual(['Older answer', 'Recent answer']);
  Feed.current.emit('snapshot', snapshot());
  expect(chat.messages.map(message => message.id)).toEqual(['old', 'recent']);
  expect(chat.historyCursor).toBeNull();
});

test('temporary account failure preserves the active draft', async () => {
  const chat = new Chat();
  await chat.refresh();
  chat.draft = 'Keep this draft';
  offline = true;
  await chat.refresh();
  expect(chat.ready).toBe(false);
  offline = false;
  await chat.refresh();
  expect(chat.ready).toBe(true);
  expect(chat.draft).toBe('Keep this draft');
  expect(chat.active?.id).toBe(first.id);
});
