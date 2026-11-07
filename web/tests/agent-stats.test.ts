// Checks the Agents page: each agent's figures, its health, its running time and its share of the work.
import { describe, expect, test, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import AgentStats from '../src/AgentStats.svelte';
import { duration, emptyStats, eventWords, names, shares, withKnownAgents, withLive, type AgentReport, type AgentStats as Stats } from '../src/agent-stats';
import type { Chat } from '../src/chat.svelte';

const stats = (name: string, patch: Partial<Stats> = {}, counts: { responses?: number; tasks?: number } = {}): Stats => {
  const base = emptyStats(name, name.toLowerCase());
  return { ...base, ...patch, responses: { ...base.responses, count: counts.responses ?? 0, ...patch.responses },
    tasks: { ...base.tasks, count: counts.tasks ?? 0, ...patch.tasks } };
};

describe('agent figures', () => {
  test('the main agent and quiet agents get a card, main first and running next', () => {
    const rows = withKnownAgents([stats('Writer', {}, { responses: 3 }), stats('Builder', { state: 'running' }, { responses: 1 })],
      [{ id: 'tester', name: 'Tester' }]);
    expect(rows.map(row => row.name)).toEqual(['MiniClaw', 'Builder', 'Writer', 'Tester']);
    expect(rows[0].main).toBe(true);
    expect(rows.at(-1)!.state).toBe('idle');
  });

  test('shares add up to the whole and leave out agents that did nothing', () => {
    const parts = shares([stats('A', {}, { responses: 3 }), stats('B', {}, { responses: 1 }), stats('C')], 'responses');
    expect(parts.map(part => [part.name, part.share])).toEqual([['A', 0.75], ['B', 0.25]]);
  });

  test('durations and activity read as words', () => {
    expect(duration(42)).toBe('42 s');
    expect(duration(4800)).toBe('1 h 20 min');
    expect(duration(null)).toBe('–');
    expect(eventWords({ kind: 'task', at: 2, started: 0, ended: 90_000, status: 'done', summary: 'Write tests' }))
      .toBe('Finished: Write tests (2 min)');
    expect(eventWords({ kind: 'response', at: 1, status: 'interrupted', seconds: 3, tokens: 0 })).toBe('Stopped in 3 s');
  });
});

describe('agents page', () => {
  test('shows health, running time and share from the backend', async () => {
    const now = Date.now();
    const report: AgentReport = { days: 30, from: 0, now, agents: [
      { ...stats('', { main: true, state: 'healthy', lastActive: now - 60_000 }, { responses: 3 }), id: null },
      stats('Builder', { state: 'running', runningSince: now - 125_000, runningMs: 125_000 }, { responses: 1, tasks: 1 }),
    ] };
    const chat = { agentAnalytics: vi.fn().mockResolvedValue(report), agents: [] } as unknown as Chat;
    render(AgentStats, { chat });

    const builder = await screen.findByRole('article', { name: 'Builder' });
    expect(within(builder).getByRole('status')).toHaveTextContent('Running');
    expect(within(builder).getByText(/Running for/)).toHaveTextContent('2 min');
    expect(within(screen.getByRole('article', { name: 'MiniClaw' })).getByRole('status')).toHaveTextContent('Healthy');
    expect(screen.getByRole('img', { name: 'MiniClaw 75%, Builder 25%' })).toBeInTheDocument();
    expect(chat.agentAnalytics).toHaveBeenCalledWith(30);
  });

  test('the kill switch stops one response, an agent, or every agent', async () => {
    const now = Date.now();
    const turn = (threadId: string, agent: string) => ({ threadId, turnId: `${threadId}-turn`, agent,
      conversation: `Chat ${threadId}`, startedAt: now - 30_000, stopping: false });
    const report: AgentReport = { days: 30, from: 0, now, agents: [stats('Builder', {}, { responses: 1 })],
      active: [turn('a', ''), turn('b', 'Builder'), turn('c', 'Builder')] };
    const interruptTurn = vi.fn().mockResolvedValue({});
    const chat = { agentAnalytics: vi.fn().mockResolvedValue(report), agents: [], interruptTurn } as unknown as Chat;
    render(AgentStats, { chat });

    // A response running now makes its agent Running, the main agent included.
    const builder = await screen.findByRole('article', { name: 'Builder' });
    expect(within(builder).getByRole('status')).toHaveTextContent('Running');
    expect(within(screen.getByRole('article', { name: 'MiniClaw' })).getByRole('status')).toHaveTextContent('Running');

    await fireEvent.click(within(builder).getByRole('button', { name: 'Stop Chat b' }));
    expect(interruptTurn).toHaveBeenCalledExactlyOnceWith('b', 'b-turn');

    await fireEvent.click(within(builder).getByRole('button', { name: 'Stop Builder' }));
    // b is already stopping, so only c is asked.
    expect(interruptTurn).toHaveBeenLastCalledWith('c', 'c-turn');
    expect(interruptTurn).toHaveBeenCalledTimes(2);

    await fireEvent.click(screen.getByRole('button', { name: 'Stop all agents' }));
    expect(interruptTurn).toHaveBeenLastCalledWith('a', 'a-turn');
    expect(interruptTurn).toHaveBeenCalledTimes(3);
  });

  test('a stopped agent with work out shows "Delegating to" beside Stopped, and the helper shows who it works for', async () => {
    const now = Date.now();
    const report: AgentReport = { days: 30, from: 0, now, agents: [
      stats('Writer', { state: 'stopped', delegatingTo: [
        { agent: 'Builder', status: 'running', started: now - 90_000, mode: 'background', summary: 'Build the page' },
        { agent: 'Tester', status: 'asking', started: now, mode: '', summary: 'Test it' }] }, { responses: 2 }),
      stats('Builder', { state: 'running', runningSince: now - 90_000, workingFor: ['Writer', ''] }, { tasks: 1 }),
    ] };
    const chat = { agentAnalytics: vi.fn().mockResolvedValue(report), agents: [] } as unknown as Chat;
    render(AgentStats, { chat });

    const writer = await screen.findByRole('article', { name: 'Writer' });
    expect(within(writer).getByRole('status')).toHaveTextContent('Stopped');
    expect(within(writer).getByText('Delegating to Builder and Tester')).toBeInTheDocument();
    const tasks = within(writer).getByRole('list', { name: 'Writer is delegating' });
    expect(tasks).toHaveTextContent('Build the page');
    expect(tasks).toHaveTextContent('2 min, background');
    expect(tasks).toHaveTextContent('waiting for approval');
    expect(within(screen.getByRole('article', { name: 'Builder' })).getByText('Working for Writer and MiniClaw')).toBeInTheDocument();
  });

  test('a tile lists at most 3 recent events, and the filters and search narrow the tiles', async () => {
    const now = Date.now();
    const events = [1, 2, 3, 4, 5].map(n => ({ kind: 'response' as const, at: now - n * 60_000, status: 'completed', seconds: n, tokens: 0 }));
    const report: AgentReport = { days: 30, from: 0, now, agents: [
      stats('Writer', { state: 'stopped', recent: events }, { responses: 5 }),
      stats('Builder', { state: 'healthy' }, { responses: 1 }),
      stats('Tester', { state: 'failed' }, { responses: 1 }),
    ] };
    const chat = { agentAnalytics: vi.fn().mockResolvedValue(report), agents: [] } as unknown as Chat;
    render(AgentStats, { chat });

    const writer = await screen.findByRole('article', { name: 'Writer' });
    expect(within(within(writer).getByRole('list', { name: 'Writer recent activity' })).getAllByRole('listitem')).toHaveLength(3);

    await fireEvent.click(screen.getByRole('radio', { name: /Stopped or failed/ }));
    expect(screen.getAllByRole('article').map(tile => tile.getAttribute('aria-label')).sort()).toEqual(['Tester', 'Writer']);

    await fireEvent.click(screen.getByRole('radio', { name: /All/ }));
    await fireEvent.input(screen.getByRole('searchbox', { name: 'Find an agent' }), { target: { value: 'buil' } });
    expect(screen.getAllByRole('article').map(tile => tile.getAttribute('aria-label'))).toEqual(['Builder']);
  });

  test('names shortens a long list', () => {
    expect(names(['A', 'B', 'C'])).toBe('A and 2 others');
    expect(names(['A', 'A'])).toBe('A');
  });

  test('withLive leaves an agent with nothing running as it was', () => {
    const [agent] = withLive([stats('Writer', { state: 'healthy' })], []);
    expect(agent.state).toBe('healthy');
    expect(agent.live).toEqual([]);
  });
});
