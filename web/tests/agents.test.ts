// Checks profile state with controlled network responses.
import { afterEach, expect, test, vi } from 'vitest';
// Exercises the browser's profile management state.
import { Chat, type Agent } from '../src/chat.svelte';

const agent: Agent = {
  id: '019a4444-4444-7444-8444-444444444444', revision: 2,
  name: 'Editor', instructions: 'Review clarity.', active: false,
  model: null, effort: null, capabilities: [],
  access: { applications: [], files: [], network: [] }, delegatesTo: [], delegation: { maxPerTurn: 3, minutes: 10 }, skillIds: [],
};

afterEach(() => vi.unstubAllGlobals());

test('a late list cannot overwrite a saved deactivation', async () => {
  let release!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn()
    .mockImplementationOnce(() => new Promise<Response>(resolve => { release = resolve; }))
    .mockResolvedValueOnce(Response.json({ agent })));
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  chat.agentId = agent.id;
  const loading = chat.refreshAgents();
  const { id, ...draft } = agent;
  await chat.saveAgent(id, draft);
  release(Response.json({ agents: [{ ...agent, revision: 1, active: true }] }));
  await loading;
  expect(chat.agents).toEqual([agent]);
  expect(chat.agentReady).toBe(false);
  expect(chat.agentsLoading).toBe(false);
});
