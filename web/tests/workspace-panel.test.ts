// Checks the workspace panel's Issues tab: shown only for a project, with the review count.
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
// Renders the panel and drives it as a user would.
import { render, screen, waitFor, within, fireEvent } from '@testing-library/svelte';
// The views the Issues tests do not open are replaced by a stub.
vi.mock('../src/DesktopView.svelte', () => import('./StubView.svelte'));
vi.mock('../src/FilesView.svelte', () => import('./StubView.svelte'));
vi.mock('../src/ChangesView.svelte', () => import('./StubView.svelte'));
vi.mock('../src/Terminal.svelte', () => import('./StubView.svelte'));
vi.mock('../src/AgentCards.svelte', () => import('./StubView.svelte'));
// The component under test and the chat shape it reads.
import WorkspacePanel from '../src/WorkspacePanel.svelte';
import type { Chat } from '../src/chat.svelte';

/** Builds a chat in a project, or outside one, whose project has two issues waiting for review. */
function setup(projectId?: string) {
  const waiting = [1, 2].map(number => ({ number, kind: 'bug', title: `Waiting ${number}`, text: '', severity: 'medium', tags: [],
    status: 'review', owner: null, ownerId: null, createdBy: 'Builder', createdAt: 1, updatedAt: 1 }));
  const projects = {
    items: [{ id: 'p', name: 'Game', agentIds: [], skillIds: [], revision: 1 }],
    issues: vi.fn(async (_id: string, _query = '', status?: string) => ({ closeWithoutReview: 'low',
      issues: !status || status === 'review' ? waiting : [] })),
    issue: vi.fn(async (_id: string, number: number) => ({ ...waiting[number - 1], events: [] })),
  };
  const chat = { machine: 'host', active: { id: 'c', projectId }, delegations: [], activities: [], agents: [], projects } as unknown as Chat;
  render(WorkspacePanel, { chat });
  return { projects };
}

beforeEach(() => { try { localStorage.clear(); } catch { /* No storage. */ } });
afterEach(() => { vi.useRealTimers(); });

describe('the Issues tab', () => {
  test('is missing outside a project', async () => {
    const { projects } = setup();
    expect(screen.queryByRole('button', { name: 'Open Issues' })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Expand workspace' }));
    const tabs = screen.getByRole('tablist', { name: 'Workspace views' });
    expect(within(tabs).queryByRole('tab', { name: /Issues/ })).toBeNull();
    expect(within(tabs).getByRole('tab', { name: 'Files' }).querySelector('span')!.className).toContain('@[34rem]:not-sr-only');
    expect(projects.issues).not.toHaveBeenCalled();
  });

  test('shows the review count on the rail and the tab', async () => {
    const { projects } = setup('p');
    const rail = screen.getByRole('button', { name: 'Open Issues' });
    await waitFor(() => expect(rail.querySelector('.workspace-dot')).not.toBeNull());
    expect(projects.issues).toHaveBeenCalledWith('p', '', 'review');
    await fireEvent.click(rail);
    const tab = within(screen.getByRole('tablist', { name: 'Workspace views' })).getByRole('tab', { name: /Issues/ });
    expect(tab.getAttribute('aria-selected')).toBe('true');
    expect(tab.querySelector('span')!.className).toContain('@[42rem]:not-sr-only');
    await waitFor(() => expect(tab.querySelector('.workspace-count')?.textContent).toBe('2'));
    expect(await screen.findByRole('group', { name: 'Issue status' })).toBeTruthy();
  });

  test('hears changes live from the project stream, with a 30 s fallback', async () => {
    const sources: FakeSource[] = [];
    class FakeSource {
      url: string; closed = false; listeners: Record<string, (() => void)[]> = {};
      constructor(url: string) { this.url = url; sources.push(this); }
      addEventListener(name: string, listener: () => void) { (this.listeners[name] ??= []).push(listener); }
      emit(name: string) { for (const listener of this.listeners[name] ?? []) listener(); }
      close() { this.closed = true; }
    }
    vi.stubGlobal('EventSource', FakeSource);
    vi.useFakeTimers();
    try {
      const { projects } = setup('p');
      const reviews = () => projects.issues.mock.calls.filter(call => call[2] === 'review').length;
      await vi.advanceTimersByTimeAsync(0);
      expect(sources.map(source => source.url)).toEqual(['/api/projects/p/issues/live']);
      expect(reviews()).toBe(1);
      sources[0].emit('issues');
      await vi.advanceTimersByTimeAsync(0);
      expect(reviews()).toBe(2);
      await vi.advanceTimersByTimeAsync(30000);
      expect(reviews()).toBe(3);
    } finally { vi.unstubAllGlobals(); }
  });

  test('the Board shows every stage, and a card opens its issue in the Issues tab', async () => {
    setup('p');
    await fireEvent.click(screen.getByRole('button', { name: 'Open Board' }));
    const stages = await screen.findByRole('list', { name: 'Issue stages' });
    const names = within(stages).getAllByRole('listitem').filter(item => item.tagName === 'SECTION').map(item => item.getAttribute('aria-label'));
    expect(names).toEqual(['Backlog, 0', 'In Dev, 0', 'QA, 0', 'Review, 0', 'Human Review, 2', 'Closed, 0']);
    await fireEvent.click(await screen.findByRole('button', { name: /Waiting 2/ }));
    const tab = within(screen.getByRole('tablist', { name: 'Workspace views' })).getByRole('tab', { name: /Issues/ });
    await waitFor(() => expect(tab.getAttribute('aria-selected')).toBe('true'));
    expect(await screen.findByRole('heading', { name: 'Waiting 2' })).toBeTruthy();
  });
});
