// Checks the project setting that decides which issues agents close alone.
import { describe, expect, test, vi } from 'vitest';
// Renders the section and drives it like a user.
import { render, screen, waitFor, fireEvent } from '@testing-library/svelte';
// The component under test and its backend contract.
import IssueSettings from '../src/IssueSettings.svelte';
import type { Projects, ReviewLevel } from '../src/project-state.svelte';

// A stand-in for the projects store with only the two calls the section uses.
function store(level: ReviewLevel, save: (id: string, level: ReviewLevel) => Promise<unknown> = async () => ({})) {
  return { reviewLevel: vi.fn(async () => level), saveReviewLevel: vi.fn(save) } as unknown as Projects
    & { reviewLevel: ReturnType<typeof vi.fn>; saveReviewLevel: ReturnType<typeof vi.fn> };
}

// Finds the radio for a choice by its visible name.
function choice(name: RegExp) { return screen.getByRole('radio', { name }) as HTMLInputElement; }

describe('issue review level', () => {
  test('offers four described choices', async () => {
    render(IssueSettings, { projects: store('low'), id: 'p1' });
    const group = screen.getByRole('group', { name: 'Agents may close without review' });
    expect(group.tagName).toBe('FIELDSET');
    expect(screen.getAllByRole('radio')).toHaveLength(4);
    for (const [name, value] of [[/^Nothing/, 'none'], [/^Low \(default\)/, 'low'], [/^Low and Medium/, 'medium'], [/^Everything/, 'high']] as const)
      expect(choice(name).value).toBe(value);
    expect(screen.getByText('Every issue waits for you.')).toBeTruthy();
    expect(screen.getByText('Only High waits for you.')).toBeTruthy();
  });

  test('loads the saved level', async () => {
    const projects = store('medium');
    render(IssueSettings, { projects, id: 'p1' });
    await waitFor(() => expect(choice(/^Low and Medium/).checked).toBe(true));
    expect(projects.reviewLevel).toHaveBeenCalledWith('p1');
  });

  test('saves a choice at once', async () => {
    const projects = store('low');
    render(IssueSettings, { projects, id: 'p1' });
    await waitFor(() => expect(choice(/^Low \(default\)/).checked).toBe(true));
    await fireEvent.click(choice(/^Everything/));
    await waitFor(() => expect(projects.saveReviewLevel).toHaveBeenCalledWith('p1', 'high'));
    expect(choice(/^Everything/).checked).toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  test('a failed save restores the previous choice', async () => {
    const projects = store('low', async () => { throw new Error('Disk full'); });
    render(IssueSettings, { projects, id: 'p1' });
    await waitFor(() => expect(choice(/^Low \(default\)/).checked).toBe(true));
    await fireEvent.click(choice(/^Nothing/));
    expect((await screen.findByRole('alert')).textContent).toContain('Disk full');
    await waitFor(() => expect(choice(/^Low \(default\)/).checked).toBe(true));
    expect(choice(/^Nothing/).checked).toBe(false);
  });

  test('the choices stay enabled while a save runs', async () => {
    let finish = () => {};
    const projects = store('low', () => new Promise<void>(resolve => { finish = resolve; }));
    render(IssueSettings, { projects, id: 'p1' });
    await waitFor(() => expect(choice(/^Low \(default\)/).checked).toBe(true));
    choice(/^Low and Medium/).focus();
    await fireEvent.click(choice(/^Low and Medium/));
    expect(projects.saveReviewLevel).toHaveBeenCalledWith('p1', 'medium');
    expect(choice(/^Everything/)).toBeEnabled();
    expect(document.activeElement).toBe(choice(/^Low and Medium/));
    expect(screen.getByRole('group', { name: 'Agents may close without review' }).getAttribute('aria-busy')).toBe('true');
    finish();
  });

  test('the choices are disabled until the level loads', async () => {
    render(IssueSettings, { projects: store('low'), id: 'p1' });
    expect(choice(/^Everything/)).toBeDisabled();
    await waitFor(() => expect(choice(/^Everything/)).toBeEnabled());
  });

  test('the selected row is outlined with a colour every accent shows', async () => {
    render(IssueSettings, { projects: store('low'), id: 'p1' });
    await waitFor(() => expect(choice(/^Low \(default\)/).checked).toBe(true));
    const row = choice(/^Low \(default\)/).closest('label')!;
    expect(row.className).toContain('border-ring');
    expect(row.className).not.toContain('border-brand');
  });
});
