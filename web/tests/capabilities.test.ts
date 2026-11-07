// Checks draft choices without starting a provider.
import { afterEach, expect, test, vi } from 'vitest';
// Owns the composer's draft and tool settings.
import { Chat } from '../src/chat.svelte';

afterEach(() => vi.unstubAllGlobals());

test('draft tools stay local and reset for a new agent or chat', async () => {
  const request = vi.fn(async (_url: RequestInfo | URL, _init?: RequestInit) => Response.json({}));
  vi.stubGlobal('fetch', request);
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.draft = 'Keep this prompt';
  expect(await chat.toggleCapability('coding_session', true)).toBe(true);
  expect(await chat.toggleCapability('full_access', true)).toBe(true);
  expect(chat.codingSession).toBe(true);
  expect(chat.fullAccess).toBe(true);
  expect(request).not.toHaveBeenCalled();
  chat.agents = [{ id: 'scout', name: 'Scout', instructions: '', active: true, revision: 1,
    model: null, effort: null, capabilities: [], skillIds: [], access: { applications: [], files: [], network: [] },
    delegatesTo: [], delegation: { maxPerTurn: 3, minutes: 10 } }];
  chat.chooseAgent('scout');
  expect(chat.draft).toBe('Keep this prompt');
  expect(chat.codingSession).toBe(false);
  expect(chat.fullAccess).toBe(false);
  await chat.toggleCapability('full_access', true);
  await chat.newChat();
  expect(chat.fullAccess).toBe(false);
});

test('draft tools cannot change while creation is in flight', async () => {
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.working = true;
  expect(await chat.toggleCapability('full_access', true)).toBe(false);
  expect(chat.fullAccess).toBe(false);
});

test('shell grants send a duration and cannot be staged in drafts', async () => {
  const request = vi.fn(async (_url: RequestInfo | URL, _init?: RequestInit) => Response.json({}));
  vi.stubGlobal('fetch', request);
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  expect(await chat.toggleCapability('unsandboxed_shell', true, 900)).toBe(false);
  expect(request).not.toHaveBeenCalled();
  chat.active = { id: 'timed-chat' } as typeof chat.active;
  expect(await chat.toggleCapability('unsandboxed_shell', true, 900)).toBe(true);
  expect(JSON.parse(String(request.mock.calls[0]?.[1]?.body))).toEqual({ key: 'unsandboxed_shell', on: true, durationSeconds: 900 });
});
