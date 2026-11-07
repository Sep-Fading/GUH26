// Checks the timeline under the Agents page's tiles: blocks coloured by what happened, and downtime percentages.
import { describe, expect, test } from 'vitest';
import { render, screen, within } from '@testing-library/svelte';
import AgentTimeline from '../src/AgentTimeline.svelte';
import { blockState, combine, type Block, type Timeline } from '../src/agent-stats';

const empty = (): Block => ({ ok: 0, stopped: 0, failed: 0, failures: 0 });
const blocks = (set: Record<number, Partial<Block>>) => Array.from({ length: 4 }, (_, index) => ({ ...empty(), ...set[index] }));

const timeline: Timeline = { from: 0, to: 4 * 3_600_000, width: 3_600_000, count: 4, agents: [
  // 3 minutes finished and 1 failed: a quarter of its working time is downtime.
  { name: '', okMs: 180_000, stoppedMs: 0, failedMs: 60_000, downtime: 0.25, failures: 1,
    blocks: blocks({ 0: { ok: 180_000 }, 2: { failed: 60_000, failures: 1 } }) },
  { name: 'Builder', okMs: 60_000, stoppedMs: 60_000, failedMs: 0, downtime: 0.5, failures: 0,
    blocks: blocks({ 1: { ok: 60_000, stopped: 60_000 } }) },
] };

describe('timeline', () => {
  test('a block is as bad as the worst thing in it', () => {
    expect(blockState(empty())).toBe('empty');
    expect(blockState({ ...empty(), ok: 1 })).toBe('ok');
    expect(blockState({ ...empty(), ok: 5, stopped: 1 })).toBe('stopped');
    expect(blockState({ ...empty(), ok: 5, failures: 1 })).toBe('failed');
  });

  test('the all-agents line adds every block and works out its own downtime', () => {
    const all = combine(timeline.agents, 4);
    expect(all.blocks[1]).toEqual({ ok: 60_000, stopped: 60_000, failed: 0, failures: 0 });
    // 120 s of 360 s did not finish.
    expect(all.downtime).toBeCloseTo(1 / 3);
  });

  test('draws a row per agent with its downtime, an all-agents row, and a quiet agent as empty', () => {
    render(AgentTimeline, { timeline, days: 1, order: [{ name: 'MiniClaw', main: true }, { name: 'Builder', main: false },
      { name: 'Tester', main: false }] });
    const section = screen.getByRole('region', { name: 'Timeline' });
    expect(within(section).getByLabelText('MiniClaw downtime 25%')).toBeInTheDocument();
    expect(within(section).getByLabelText('Builder downtime 50%')).toBeInTheDocument();
    expect(within(section).getByLabelText('Tester downtime 0%')).toBeInTheDocument();
    expect(within(section).getByLabelText('All agents downtime 33.3%')).toBeInTheDocument();
    expect(within(section).getByRole('img', { name: '66.7% finished, 16.7% stopped, 16.7% failed' })).toBeInTheDocument();
    const main = within(section).getByRole('img', { name: 'MiniClaw timeline' });
    expect(main.children).toHaveLength(4);
    expect(main.children[0].className).toContain('bg-emerald-500');
    expect(main.children[2].className).toContain('bg-destructive');
    expect(main.children[3].className).toContain('bg-muted');
  });
});
