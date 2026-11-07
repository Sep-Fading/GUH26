// Checks the deletion dialog's optional box: deleting a conversation's own folder is an explicit choice.
import { afterEach, describe, expect, test, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import ConfirmDelete from './ConfirmDeleteHarness.svelte';
// Deletes through the same state the sidebar uses.
import { Chat } from '../src/chat.svelte';
// Switches the wording between macOS and Windows.
import { setHost } from '../src/platform.svelte';

const option = { label: "Also move this conversation's folder to the Trash", detail: '/Users/u/Documents/MiniClaw/Say hi 3f2a9c1d' };

describe('deleting a conversation with its own folder', () => {
  // Requirement by user decision 2026-10-05: a box, unticked by default, offers the folder too.
  test('the box is unticked by default, so the folder is kept', async () => {
    const action = vi.fn(async (_ticked: boolean) => {});
    render(ConfirmDelete, { label: 'Delete chat Say hi', actionLabel: 'Delete chat', description: 'Delete?', option, action });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete chat Say hi' }));
    const box = await screen.findByRole('checkbox', { name: /Also move this conversation's folder to the Trash/ });
    expect((box as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText(option.detail)).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Delete chat' }));
    await waitFor(() => expect(action).toHaveBeenCalledWith(false));
  });

  test('ticking the box asks for the folder too', async () => {
    const action = vi.fn(async (_ticked: boolean) => {});
    render(ConfirmDelete, { label: 'Delete chat Say hi', actionLabel: 'Delete chat', description: 'Delete?', option, action });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete chat Say hi' }));
    await fireEvent.click(await screen.findByRole('checkbox', { name: /Also move/ }));
    await fireEvent.click(screen.getByRole('button', { name: 'Delete chat' }));
    await waitFor(() => expect(action).toHaveBeenCalledWith(true));
  });

  test('a conversation without a folder shows no box', async () => {
    render(ConfirmDelete, { label: 'Delete chat Plain', actionLabel: 'Delete chat', description: 'Delete?', action: async () => {} });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete chat Plain' }));
    await screen.findByRole('alertdialog');
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
});

// Review F-2: the conversation is deleted first, so a folder that could not be moved (for example to a
// Trash on another volume) must be named to the user, who can no longer ask for it from the sidebar.
describe('a folder that could not be moved to the Trash', () => {
  afterEach(() => { setHost('macos'); vi.unstubAllGlobals(); });

  /** Deletes conversation `two` with the box ticked while the backend answers `answer`. */
  async function deleteWith(answer: unknown) {
    const task = { id: 'one', name: 'First', model: null };
    const other = { id: 'two', name: 'Say hi', model: null, folder: '/Users/u/Documents/MiniClaw/Say hi 3f2a9c1d' };
    const sent: unknown[] = [];
    vi.stubGlobal('EventSource', class extends EventTarget { close() {} });
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/delete')) { sent.push(JSON.parse(String(init?.body ?? '{}'))); return Response.json(answer); }
      return Response.json({ thread: task, messages: [], activeTurn: null, pending: false });
    }));
    const chat = new Chat();
    chat.account = { status: 'signed_in' };
    chat.tasks = [task, other];
    await chat.select(task);
    await chat.deleteChat(other, true);
    expect(sent).toEqual([{ deleteFolder: true }]);
    expect(chat.tasks.map(item => item.id)).toEqual(['one']);
    return chat;
  }

  test('the user is told where the folder was kept', async () => {
    const chat = await deleteWith({ folderKept: true, folder: '/Users/u/Documents/MiniClaw/Say hi 3f2a9c1d' });
    expect(chat.error).toBe('The conversation was deleted; its folder could not be moved to the Trash and was kept at /Users/u/Documents/MiniClaw/Say hi 3f2a9c1d.');
  });

  test('Windows names MiniClaw\'s Recovered files folder', async () => {
    setHost('windows');
    const chat = await deleteWith({ folderKept: true, folder: 'D:\\Documents\\MiniClaw\\Say hi 3f2a9c1d' });
    expect(chat.error).toBe("The conversation was deleted; its folder could not be moved to MiniClaw's Recovered files folder and was kept at D:\\Documents\\MiniClaw\\Say hi 3f2a9c1d.");
  });

  test('a moved folder says nothing', async () => {
    const chat = await deleteWith({});
    expect(chat.error).toBe('');
  });
});
