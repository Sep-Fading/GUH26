// Checks the delegation limits shared by the main agent and named agents.
import { describe, expect, test, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import DelegationLimits from '../src/DelegationLimits.svelte';
// Words a limit in hours and minutes, as the form and the delegation prompt show it.
import { duration, runPhase } from '../src/delegation-words';
import type { DelegatedRun } from '../src/chat.svelte';

// Requirement 2026-10-11 (user decision): "add the ability to configure delegation settings on main agent so we
// are not locked into a value for the time limit". 1 to 1440 minutes for every agent; a waited run gets an hour.
describe('delegation limits', () => {
  test('the time limit is typed in minutes from 1 to 1440, with an hour hint from 60 minutes', async () => {
    render(DelegationLimits, { value: { maxPerTurn: 3, minutes: 10 }, onchange: () => {} });
    const minutes = screen.getByRole('spinbutton', { name: 'Time limit per run' });
    expect(minutes).toHaveAttribute('min', '1');
    expect(minutes).toHaveAttribute('max', '1440');
    expect(minutes).toHaveValue(10);
    expect(screen.getByRole('combobox', { name: 'Delegations per message' })).toHaveValue('3');
    expect(screen.queryByText(/\d+ h/)).toBeNull();
    expect(screen.queryByText(/waits for at most/)).toBeNull();
  });

  test('a limit over an hour shows hours and says a waited run continues in the background after an hour', () => {
    render(DelegationLimits, { value: { maxPerTurn: 3, minutes: 150 }, onchange: () => {} });
    expect(screen.getByText('2 h 30 min')).toBeInTheDocument();
    expect(screen.getByText(/conversation waits for at most 1 h.*background.*full limit/)).toBeInTheDocument();
    // Requirement change 2026-10-11 (lead decision): at the hour a longer run continues in the background.
    expect(screen.getByText(/at most 1 h; then the run continues in the background, up to the full limit/)).toBeInTheDocument();
    expect(screen.queryByText(/then the run\s+stops/)).toBeNull();
  });

  test('a valid change is sent, and an out-of-range one is refused with a message', async () => {
    const onchange = vi.fn();
    render(DelegationLimits, { value: { maxPerTurn: 3, minutes: 10 }, onchange });
    const minutes = screen.getByRole('spinbutton', { name: 'Time limit per run' });
    await fireEvent.input(minutes, { target: { value: '1441' } });
    await fireEvent.change(minutes);
    expect(onchange).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a whole number of minutes from 1 to 1440.');
    await fireEvent.input(minutes, { target: { value: '1440' } });
    await fireEvent.change(minutes);
    expect(onchange).toHaveBeenLastCalledWith({ maxPerTurn: 3, minutes: 1440 });
    expect(screen.queryByRole('alert')).toBeNull();
    await fireEvent.change(screen.getByRole('combobox', { name: 'Delegations per message' }), { target: { value: '7' } });
    // The props are not updated here, as a parent would after saving, so only the count is checked.
    expect(onchange).toHaveBeenLastCalledWith(expect.objectContaining({ maxPerTurn: 7 }));
  });

  test('durations read in hours and minutes', () => {
    expect([duration(1), duration(59), duration(60), duration(90), duration(1440)]).toEqual(['1 min', '59 min', '1 h', '1 h 30 min', '24 h']);
  });

  // Requirement 2026-10-11 (lead decision): a waited run still going at the hour is handed to the background,
  // and its block and card say so.
  test('a run handed to the background at the hour says so while it runs', () => {
    const run: DelegatedRun = { id: 'r1', turnId: 't1', thread: 'c1', agent: 'Reviewer', task: 'Review.', mode: 'background',
      status: 'running', result: null, started: 1, ended: null, detached: false, parent: 'p1', handedOver: true };
    expect(runPhase(run)).toBe('Continues in the background after the 1 h wait');
    expect(runPhase({ ...run, handedOver: false })).toBe('Running in the background');
    expect(runPhase({ ...run, mode: 'wait', handedOver: false })).toBe('Running, the conversation waits');
    expect(runPhase({ ...run, status: 'done', result: 'Done.' })).toBe('Finished');
  });
});
