// Checks guarded project edits and conversation selection.
import { afterEach, expect, test, vi } from 'vitest';
// Exercises the same state used by Svelte.
import { Chat } from '../src/chat.svelte';
// Tests project refresh races without rendering.
import { Projects, type Project } from '../src/project-state.svelte';

const project: Project = { id: 'project-1', name: 'Game', revision: 2,
  agentIds: [], skillIds: [], environment: { kind: 'host' } };

afterEach(() => { vi.unstubAllGlobals(); localStorage.clear(); });

test('a stale project list cannot overwrite a saved revision', async () => {
  let release!: (value: unknown) => void;
  const api = vi.fn().mockImplementationOnce(() => new Promise(resolve => release = resolve))
    .mockResolvedValueOnce({ project });
  const state = new Projects(api, () => true);
  const loading = state.refresh();
  const { id, ...draft } = project;
  await state.save(id, draft);
  release({ projects: [{ ...project, revision: 1, name: 'Old' }] });
  await loading;
  expect(state.items).toEqual([project]);
});

test('account clearing discards pending project replies', async () => {
  let release!: (value: unknown) => void;
  const api = vi.fn().mockImplementation(() => new Promise(resolve => release = resolve));
  const state = new Projects(api, () => true);
  const loading = state.refresh();
  state.clear();
  release({ projects: [project] });
  await loading;
  expect(state.items).toEqual([]);
});

test('project selection filters agents without mutating their skills', () => {
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.projectId = project.id;
  chat.projects.items = [{ ...project, agentIds: ['editor'] }];
  const agent = { id: 'editor', name: 'Editor', active: true, revision: 1,
    instructions: 'Review', model: null, effort: null, capabilities: [],
    access: { applications: [], files: [], network: [] }, delegatesTo: [], delegation: { maxPerTurn: 3, minutes: 10 }, skillIds: ['own-skill'] };
  chat.agents = [agent, { ...agent, id: 'other' }];
  expect(chat.availableAgents.map(a => a.id)).toEqual(['editor']);
  chat.chooseAgent('editor');
  expect(chat.agentReady).toBe(true);
  expect(chat.agents[0].skillIds).toEqual(['own-skill']);
  chat.projects.items = [project];
  expect(chat.agentReady).toBe(false);
});

test('deleting another chat does not invalidate the active stream', async () => {
  const task = { id: 'one', name: 'First', model: null };
  const other = { ...task, id: 'two' };
  let feed!: EventTarget;
  vi.stubGlobal('EventSource', class extends EventTarget {
    // Exposes the active stream to synthetic notification assertions.
    constructor() { super(); feed = this; }
    // Matches the browser cleanup contract without network traffic.
    close() {}
  });
  vi.stubGlobal('fetch', vi.fn(async (url: string) => Response.json(url.endsWith('/delete') ? {} : {
    thread: task, messages: [], activeTurn: { id: 'turn' }, pending: false,
  })));
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.tasks = [task, other];
  await chat.select(task);
  await chat.deleteChat(other);
  feed.dispatchEvent(new MessageEvent('message.delta', { data: JSON.stringify({ itemId: 'answer', delta: 'Still here' }) }));
  expect(chat.messages[0]?.text).toBe('Still here');
  expect(chat.active?.id).toBe('one');
});
