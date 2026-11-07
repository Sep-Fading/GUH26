// Checks the running-agents box over the chat: it appears only while something runs, groups by agent, and its
// kill switch stops one response, one agent or every agent.
import { describe, expect, test, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import RunningAgents from '../src/RunningAgents.svelte';

const turn = (threadId: string, agent: string) => ({ threadId, turnId: `${threadId}-turn`, agent,
  conversation: `Chat ${threadId}`, startedAt: Date.now() - 65_000, stopping: false });

describe('running agents box', () => {
  test('stays hidden while nothing runs', async () => {
    const activeTurns = vi.fn().mockResolvedValue({ active: [] });
    render(RunningAgents, { chat: { activeTurns, interruptTurn: vi.fn() } });
    await vi.waitFor(() => expect(activeTurns).toHaveBeenCalled());
    expect(screen.queryByRole('region', { name: 'Running agents' })).toBeNull();
  });

  test('groups running responses by agent and stops them', async () => {
    const activeTurns = vi.fn().mockResolvedValue({ active: [turn('a', ''), turn('b', 'Builder'), turn('c', 'Builder')] });
    const interruptTurn = vi.fn().mockResolvedValue({});
    const onopen = vi.fn();
    render(RunningAgents, { chat: { activeTurns, interruptTurn }, onopen });

    const box = await screen.findByRole('region', { name: 'Running agents' });
    expect(within(box).getByText('2 agents running')).toBeInTheDocument();
    expect(within(box).getByRole('group', { name: 'MiniClaw running now' })).toBeInTheDocument();
    const builder = within(box).getByRole('group', { name: 'Builder running now' });
    expect(within(builder).getAllByRole('listitem')).toHaveLength(2);

    await fireEvent.click(within(builder).getByRole('button', { name: 'Stop Chat b' }));
    expect(interruptTurn).toHaveBeenCalledExactlyOnceWith('b', 'b-turn');
    await fireEvent.click(within(builder).getByRole('button', { name: 'Stop Builder' }));
    expect(interruptTurn).toHaveBeenLastCalledWith('c', 'c-turn');
    await fireEvent.click(within(box).getByRole('button', { name: 'Stop all agents' }));
    expect(interruptTurn).toHaveBeenLastCalledWith('a', 'a-turn');
    expect(interruptTurn).toHaveBeenCalledTimes(3);

    await fireEvent.click(within(box).getByText('2 agents running'));
    expect(onopen).toHaveBeenCalled();
  });

  test('folds to a single line', async () => {
    const activeTurns = vi.fn().mockResolvedValue({ active: [turn('a', '')] });
    render(RunningAgents, { chat: { activeTurns, interruptTurn: vi.fn() } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Fold running agents' }));
    expect(screen.queryByRole('group')).toBeNull();
    expect(screen.getByRole('button', { name: 'Stop all agents' })).toBeInTheDocument();
  });
});
