// Checks prompt ownership through navigation and renewal polls.
import { afterEach, expect, test, vi } from 'vitest';
// Owns global approvals and selected conversation state.
import { Chat } from '../src/chat.svelte';

afterEach(() => vi.unstubAllGlobals());

test('switching chats preserves prompts and each answer targets its owner', async () => {
  const request = vi.fn(async () => Response.json({ threads: [], delegationPrompts: [] }));
  vi.stubGlobal('fetch', request);
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  const first = { id: 'a', thread: 'origin-a', threadName: 'Project A', agent: 'Reviewer',
    task: 'Check A', expiresInSeconds: 120, expiresAt: Date.now() + 120000 };
  const second = { ...first, id: 'b', thread: 'origin-b', task: 'Check B' };
  chat.backgroundDelegations = [{ ...first, id: 'expired', expiresAt: Date.now() - 1000 }, first, second];
  chat.active = { id: 'elsewhere', name: 'Other chat' } as typeof chat.active;
  expect(chat.pendingDelegation?.id).toBe('a');
  chat.active = { id: 'another', name: 'Another chat' } as typeof chat.active;
  expect(chat.pendingDelegation?.id).toBe('a');
  await chat.answerDelegation('background');
  expect(request).toHaveBeenCalledWith('/api/threads/origin-a/delegations/a', expect.objectContaining({ body: '{"mode":"background"}' }));
  expect(chat.active?.id).toBe('another');
  // A response already in flight must not resurrect the answered prompt.
  chat.backgroundDelegations = [first, second];
  expect(chat.pendingDelegation?.id).toBe('b');
  await chat.answerDelegation('wait');
  expect(request).toHaveBeenCalledWith('/api/threads/origin-b/delegations/b', expect.objectContaining({ body: '{"mode":"wait"}' }));
});

// Parallel delegates and the parent retain their own requests and answer routes.
test('parent approvals queue child requests and suppress answered polls', async () => {
  const request = vi.fn(async () => Response.json({ threads: [], delegatedRequests: [] }));
  vi.stubGlobal('fetch', request);
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.active = { id: 'parent', name: 'Lead' } as typeof chat.active;
  const permission = { id: 'own', target: { target_type: 'file' as const, path: '/etc/hosts', action: 'read' as const }, expiresInSeconds: 120 };
  chat.scopedApproval = permission;
  const child = { parent: 'parent', thread: 'child', agent: 'Reviewer', runId: 'run',
    expiresAt: Date.now() + 120000, kind: 'scoped' as const, request: { ...permission, id: 'child-ask' } };
  const other = { ...child, thread: 'second', runId: 'second-run', request: { ...permission, id: 'second-ask' } };
  chat.delegatedRequests = [child, other];
  expect(chat.pendingScopedApproval?.id).toBe('own');
  await chat.decideScoped('reject');
  expect(request).toHaveBeenCalledWith('/api/threads/parent/scoped-approvals/own', expect.anything());
  chat.delegatedRequests = [child, other];
  expect(chat.pendingScopedApproval?.id).toBe('child-ask');
  await chat.decideScoped('approve_once');
  expect(request).toHaveBeenCalledWith('/api/threads/child/scoped-approvals/child-ask', expect.anything());
  chat.delegatedRequests = [child, other];
  expect(chat.pendingScopedApproval?.id).toBe('second-ask');
  chat.active = { id: 'elsewhere', name: 'Other' } as typeof chat.active;
  expect(chat.pendingScopedApproval).toBeNull();
  chat.active = { id: 'parent', name: 'Lead' } as typeof chat.active;
  expect(chat.pendingScopedApproval?.id).toBe('second-ask');
  chat.delegatedRequests = [{ ...other, expiresAt: Date.now() - 1000 }];
  expect(chat.pendingScopedApproval).toBeNull();
});
