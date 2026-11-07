// Checks the Issues tab: grouping, filters, review count, history and the user's changes.
import { describe, expect, test, vi } from 'vitest';
// Renders the panel and drives it as a user would.
import { render, screen, waitFor, within, fireEvent } from '@testing-library/svelte';
import IssuesPanel from '../src/IssuesPanel.svelte';
// The shapes the panel reads from the chat and the backend.
import type { Chat } from '../src/chat.svelte';
import type { ProjectIssue } from '../src/project-state.svelte';

const issue = (number: number, fields: Partial<ProjectIssue>): ProjectIssue => ({
  number, kind: 'bug', title: `Issue ${number}`, text: 'What happens.', severity: 'low', tags: [], status: 'backlog',
  owner: null, ownerId: null, createdBy: 'Builder', createdAt: 1, updatedAt: 1, ...fields });

const listed = [
  issue(1, { title: 'Unowned bug' }),
  issue(2, { title: 'Reviewer bug', owner: 'Reviewer', ownerId: 'r', severity: 'medium', tags: ['queue'], status: 'qa' }),
  issue(3, { title: 'Builder bug', owner: 'Builder', ownerId: 'b', severity: 'high', status: 'dev' }),
  issue(4, { title: 'Waiting bug', owner: 'Reviewer', ownerId: 'r', status: 'review', severity: 'medium' }),
  issue(5, { title: 'Closed bug', status: 'closed' }),
];
const detail = issue(4, { title: 'Waiting bug', owner: 'Reviewer', ownerId: 'r', status: 'review', severity: 'medium',
  events: [
    { at: 1, author: 'Builder', kind: 'filed', text: '' },
    { at: 2, author: 'Main agent', kind: 'handover', text: 'Cause found; the fix is left.' },
    { at: 3, author: 'Reviewer', kind: 'done', text: 'Fixed and tested.' },
  ] });

/** Builds a chat whose projects answer from the lists above. */
function setup(overrides: Record<string, unknown> = {}) {
  const projects = {
    items: [{ id: 'p', name: 'Game', agentIds: ['b', 'r'], skillIds: [], revision: 1 }],
    issues: vi.fn(async (_id: string, query = '', status?: string) => ({ closeWithoutReview: 'low',
      issues: listed.filter(item => (!status || item.status === status) && (!query || item.title.toLowerCase().includes(query.toLowerCase()))) })),
    issue: vi.fn(async () => detail),
    commentIssue: vi.fn(async () => ({})), assignIssue: vi.fn(async () => ({})), closeIssue: vi.fn(async () => ({})),
    reopenIssue: vi.fn(async () => ({})), setIssueSeverity: vi.fn(async () => ({})),
    ...overrides,
  };
  const agents = [{ id: 'b', name: 'Builder' }, { id: 'r', name: 'Reviewer' }, { id: 'x', name: 'Elsewhere' }];
  const chat = { projects, agents } as unknown as Chat;
  const oncount = vi.fn();
  render(IssuesPanel, { chat, projectId: 'p', oncount });
  return { projects, oncount };
}

describe('the issue list', () => {
  test('groups open issues by owner, Unclaimed last', async () => {
    setup();
    await screen.findByText('Unowned bug');
    const groups = screen.getAllByRole('group').filter(group => group.getAttribute('aria-label') !== 'Issue status').map(group => group.getAttribute('aria-label'));
    expect(groups).toEqual(['Builder issues', 'Reviewer issues', 'Unclaimed issues']);
    expect(within(screen.getByRole('group', { name: 'Reviewer issues' })).getByText('Reviewer bug')).toBeTruthy();
    expect(screen.queryByText('Closed bug')).toBeNull();
  });

  test('reports and shows the issues waiting for review', async () => {
    const { oncount } = setup();
    await waitFor(() => expect(oncount).toHaveBeenLastCalledWith(1));
    expect(screen.getByRole('button', { name: 'Needs you, 1' })).toBeTruthy();
  });

  test('search and view ask the backend with q and status', async () => {
    const { projects } = setup();
    await screen.findByText('Unowned bug');
    await fireEvent.input(screen.getByRole('searchbox', { name: 'Search issues' }), { target: { value: 'waiting' } });
    await waitFor(() => expect(projects.issues).toHaveBeenCalledWith('p', 'waiting', undefined));
    await fireEvent.click(screen.getByRole('button', { name: /^Needs you/ }));
    await waitFor(() => expect(projects.issues).toHaveBeenCalledWith('p', 'waiting', 'review'));
    expect(screen.getByRole('button', { name: /^Needs you/ }).getAttribute('aria-pressed')).toBe('true');
    expect(await screen.findByText('Waiting bug')).toBeTruthy();
  });

  test('the status filter is a group of pressed buttons, not tabs', async () => {
    setup();
    await screen.findByText('Unowned bug');
    const filter = screen.getByRole('group', { name: 'Issue status' });
    expect(within(filter).queryAllByRole('tab')).toHaveLength(0);
    expect(within(filter).getByRole('button', { name: 'Open, 3' }).getAttribute('aria-pressed')).toBe('true');
    expect(within(filter).getByRole('button', { name: 'Closed, 1' }).getAttribute('aria-pressed')).toBe('false');
  });

  test('a failed request shows why', async () => {
    setup({ issues: vi.fn(async () => { throw new Error('Issues unavailable'); }) });
    expect((await screen.findByRole('alert')).textContent).toMatch('Issues unavailable');
  });
});

describe('one issue', () => {
  /** Opens issue #4 from the Needs you view. */
  async function open(overrides: Record<string, unknown> = {}) {
    const stub = setup(overrides);
    await fireEvent.click(await screen.findByRole('button', { name: /^Needs you/ }));
    await fireEvent.click(await screen.findByRole('button', { name: /Waiting bug/ }));
    await screen.findByRole('heading', { name: 'Waiting bug' });
    return stub;
  }

  test('shows its history with the handover', async () => {
    await open();
    const history = screen.getByRole('group', { name: 'History' });
    expect(within(history).getByText('Cause found; the fix is left.')).toBeTruthy();
    expect(within(history).getByText('Fixed and tested.')).toBeTruthy();
  });

  test('Close it closes and refreshes', async () => {
    const { projects } = await open();
    const calls = projects.issue.mock.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Close it' }));
    expect(projects.closeIssue).toHaveBeenCalledWith('p', 4);
    await waitFor(() => expect(projects.issue.mock.calls.length).toBeGreaterThan(calls));
  });

  test('the owner can be an agent, the main agent or nobody', async () => {
    const { projects } = await open();
    const owner = screen.getByRole('combobox', { name: 'Change owner' });
    expect(within(owner).queryByRole('option', { name: 'Elsewhere' })).toBeNull();
    await fireEvent.change(owner, { target: { value: 'b' } });
    expect(projects.assignIssue).toHaveBeenLastCalledWith('p', 4, 'b');
    await fireEvent.change(owner, { target: { value: 'main' } });
    expect(projects.assignIssue).toHaveBeenLastCalledWith('p', 4, 'main');
    await fireEvent.change(owner, { target: { value: '' } });
    expect(projects.assignIssue).toHaveBeenLastCalledWith('p', 4, null);
  });

  test('history rows do not repeat their verb', async () => {
    await open();
    const rows = within(screen.getByRole('group', { name: 'History' })).getAllByRole('listitem').map(row => row.firstElementChild!.textContent!.replace(/\s+/g, ' '));
    expect(rows[0]).toContain('Filed this bug');
    expect(rows[0]).not.toMatch(/Filed\s+Filed/);
    expect(rows[2]).toContain('Marked done with a note');
    expect(rows[2]).not.toMatch(/done as done/);
  });

  test('a note is added and the field cleared', async () => {
    const { projects } = await open();
    const field = screen.getByRole('textbox', { name: 'Add a note to the history' }) as HTMLTextAreaElement;
    await fireEvent.input(field, { target: { value: 'Seen on Windows too.' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add note' }));
    await waitFor(() => expect(projects.commentIssue).toHaveBeenCalledWith('p', 4, 'Seen on Windows too.'));
    await waitFor(() => expect(field.value).toBe(''));
  });

  test('the note field keeps focus while it is sent', async () => {
    let finish = () => {};
    const { projects } = await open({ commentIssue: vi.fn(() => new Promise<void>(resolve => { finish = resolve; })) });
    const field = screen.getByRole('textbox', { name: 'Add a note to the history' }) as HTMLTextAreaElement;
    field.focus();
    await fireEvent.input(field, { target: { value: 'Still broken.' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => expect(projects.commentIssue).toHaveBeenCalled());
    expect(field.disabled).toBe(false);
    expect(document.activeElement).toBe(field);
    finish();
  });

  test('severity can be changed', async () => {
    const { projects } = await open();
    await fireEvent.change(screen.getByRole('combobox', { name: 'Severity' }), { target: { value: 'high' } });
    expect(projects.setIssueSeverity).toHaveBeenCalledWith('p', 4, 'high');
  });

  test('severity choices keep the normal text colour', async () => {
    await open();
    const select = screen.getByRole('combobox', { name: 'Severity' });
    expect(select.className).not.toMatch(/amber|destructive/);
    for (const option of within(select).getAllByRole('option')) expect(option.className).toContain('text-foreground');
  });

  test('Not done reopens the issue', async () => {
    const { projects } = await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Not done' }));
    expect(projects.reopenIssue).toHaveBeenCalledWith('p', 4);
  });

  test('a closed issue can be reopened', async () => {
    const closed = { ...detail, status: 'closed' as const };
    const { projects } = await open({ issue: vi.fn(async () => closed) });
    await fireEvent.click(await screen.findByRole('button', { name: 'Reopen' }));
    expect(projects.reopenIssue).toHaveBeenCalledWith('p', 4);
  });

  test('a refused change stays explained after the reload', async () => {
    const { projects } = await open({ closeIssue: vi.fn(async () => { throw new Error('Only the user may close this.'); }) });
    const calls = projects.issue.mock.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Close it' }));
    await waitFor(() => expect(projects.issue.mock.calls.length).toBeGreaterThan(calls));
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(screen.getByRole('alert').textContent).toContain('Only the user may close this.');
  });

  test('a refused owner change puts the owner choice back', async () => {
    await open({ assignIssue: vi.fn(async () => { throw new Error('No such agent.'); }) });
    const owner = screen.getByRole('combobox', { name: 'Change owner' }) as HTMLSelectElement;
    await fireEvent.change(owner, { target: { value: 'b' } });
    await screen.findByRole('alert');
    await waitFor(() => expect(owner.value).toBe('r'));
  });

  test('a refused severity change puts the severity back', async () => {
    await open({ setIssueSeverity: vi.fn(async () => { throw new Error('Refused.'); }) });
    const select = screen.getByRole('combobox', { name: 'Severity' }) as HTMLSelectElement;
    await fireEvent.change(select, { target: { value: 'high' } });
    await screen.findByRole('alert');
    await waitFor(() => expect(select.value).toBe('medium'));
  });

  test('focus moves to the issue, and back to its row', async () => {
    await open();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Waiting bug' })));
    await fireEvent.click(screen.getByRole('button', { name: 'Back to issues' }));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: /Waiting bug/ })));
  });

  test('after Close it, focus moves to Reopen', async () => {
    const closed = { ...detail, status: 'closed' as const };
    const issue = vi.fn(async () => detail);
    const { projects } = await open({ issue });
    issue.mockImplementation(async () => closed);
    await fireEvent.click(screen.getByRole('button', { name: 'Close it' }));
    expect(projects.closeIssue).toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Reopen' })));
  });
});

describe('stages and live changes', () => {
  // Requirement 2026-11-07: issues move Backlog, In Dev, QA, Review, Human Review, Closed, and the tab updates live.
  test('open rows name their stage', async () => {
    const chatless = setup();
    await screen.findByText('Builder bug');
    expect(within(screen.getByRole('group', { name: 'Builder issues' })).getByText('In Dev')).toBeTruthy();
    expect(within(screen.getByRole('group', { name: 'Reviewer issues' })).getByText('QA')).toBeTruthy();
    expect(chatless.projects.issues).toHaveBeenCalled();
  });
});

