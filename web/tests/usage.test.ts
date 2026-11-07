// Checks that quotas cannot cross provider identities during a slow refresh.
import { afterEach, expect, test, vi } from 'vitest';
import { Chat } from '../src/chat.svelte';

afterEach(() => vi.unstubAllGlobals());

test('changing Claude accounts hides old quotas and ignores an in-flight reading', async () => {
  const chat = new Chat();
  chat.active = { id: 'claude-test', name: 'Test', model: 'claude-sonnet-5', provider: 'claude' };
  chat.account = { status: 'signed_in', providers: [{ id: 'claude', installed: true,
    enabled: true, status: 'signed_in', email: 'first@example.test' }] };
  const reading = { status: 'fresh', ordinaryUsageAllowed: null, fetchedAt: new Date().toISOString(),
    buckets: [{ limitId: 'claude', primary: { usedPercent: 11, resetsAt: null, windowDurationMins: 300 }, secondary: null }] };
  vi.stubGlobal('fetch', vi.fn(async () => Response.json(reading)));
  await chat.refreshUsage();
  expect(chat.usage.buckets[0].primary?.usedPercent).toBe(11);
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(resolve => { finish = resolve; })));
  const refresh = chat.refreshUsage(true);
  chat.account = { ...chat.account, providers: [{ ...chat.account.providers![0], email: 'second@example.test' }] };
  try { expect(chat.usage.buckets).toEqual([]); }
  finally { finish(Response.json(reading)); await refresh; }
  expect(chat.usage.buckets).toEqual([]);
});
