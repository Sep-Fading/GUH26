// Checks how a refused tool call reads in a conversation's activity list.
import { describe, expect, test } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import ActivityList from '../src/ActivityList.svelte';
import type { ToolActivity } from '../src/chat.svelte';

const reason = 'MiniClaw refused this, and nothing was run: in a conversation with no project and no agent, the file and command tool cannot act on this computer.';

describe('a refused call', () => {
  // User report 2026-10-05: a refused command showed only "Refused", and a call sent without its folder
  // showed as raw JSON beside it. Each now reads as the command it was, with the reason in view.
  test('reads as one row naming the command, and says why without being expanded', () => {
    const entries: ToolActivity[] = [{ id: 'a', turnId: 't', kind: 'command', title: 'pwd; echo hi > hi.txt', status: 'refused',
      startedAt: 1, endedAt: 2, output: reason }];
    render(ActivityList, { entries });
    const row = screen.getByRole('button', { name: /Ran\s+pwd; echo hi > hi\.txt\s+Refused/ });
    expect(row).toBeTruthy();
    expect(screen.getByText(reason).closest('[role="note"]')).toBeTruthy();
    expect(screen.queryByText(/"action"/)).toBeNull();
  });

  test('a call that ran shows no reason line', () => {
    render(ActivityList, { entries: [{ id: 'b', turnId: 't', kind: 'command', title: 'ls', status: 'ok', startedAt: 1, endedAt: 2, output: 'a.txt' }] });
    expect(screen.queryByRole('note')).toBeNull();
  });
});
