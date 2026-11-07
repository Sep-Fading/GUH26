// Synthetic examples shaped against codex-cli 0.154.0-alpha.6.1 schemas.
export const THREAD = '019a1111-1111-7111-8111-111111111111';
export const TURN = '019a2222-2222-7222-8222-222222222222';
export const OTHER = '019a3333-3333-7333-8333-333333333333';
export const CLIENT = '019a4444-4444-7444-8444-444444444444';
export const models = [
  { id: 'catalog-a', model: 'fixture-model', displayName: 'Model A',
    description: 'First model', hidden: false, isDefault: true,
    defaultReasoningEffort: 'low', supportedReasoningEfforts: [
      { reasoningEffort: 'low', description: 'Less reasoning' },
      { reasoningEffort: 'high', description: 'More reasoning' },
    ],
    // Codex 0.155 advertises speed tiers per model; a tier's id is what a turn sends as serviceTier.
    serviceTiers: [{ id: 'priority', name: 'Fast', description: '1.5x speed, increased usage' }], defaultServiceTier: null },
  { id: 'catalog-b', model: 'other-model', displayName: 'Model B',
    description: 'Second model', hidden: true, isDefault: false,
    defaultReasoningEffort: 'none', supportedReasoningEfforts: [
      { reasoningEffort: 'none', description: 'No reasoning' },
      { reasoningEffort: 'ultra', description: 'Most reasoning' },
    ] },
];
export const signedIn = { account: { type: 'chatgpt', email: 'reader@example.test', planType: 'plus' }, requiresOpenaiAuth: true };
export const signedOut = { account: null, requiresOpenaiAuth: true };
export const window = { usedPercent: 25, windowDurationMins: 300, resetsAt: 2000000000 };
export const usage = {
  rateLimits: { limitId: 'codex', primary: window, secondary: null },
  rateLimitsByLimitId: { codex: { limitId: 'codex', limitName: 'Codex', primary: window, secondary: null } },
  ordinaryUsageAllowed: true,
};
export function thread(cwd, changes = {}) {
  return { id: THREAD, sessionId: THREAD, projectId: null, name: 'First task',
    preview: 'First task', ephemeral: false, modelProvider: 'openai',
    cliVersion: '0.154.0-alpha.6.1', cwd, source: 'appServer',
    createdAt: 1900000000, updatedAt: 1900000000, status: { type: 'idle' }, turns: [], ...changes };
}
export function started(cwd, changes = {}) {
  return { thread: thread(cwd, changes), model: 'fixture-model', modelProvider: 'openai',
    cwd, approvalPolicy: 'never', approvalsReviewer: 'user',
    sandbox: { type: 'readOnly' }, reasoningEffort: null, instructionSources: [] };
}
export function turn(status = 'inProgress', items = []) { return { id: TURN, status, items }; }
export function delta(text, itemId = 'assistant-1', threadId = THREAD) {
  return { method: 'item/agentMessage/delta', params: { threadId, turnId: TURN, itemId, delta: text } };
}
export function message(text, id = 'assistant-1') { return { id, type: 'agentMessage', text, phase: 'final_answer' }; }
export function itemDone(text, id = 'assistant-1') {
  return { method: 'item/completed', params: { threadId: THREAD, turnId: TURN,
    completedAtMs: 1900000000000, item: message(text, id) } };
}
export function turnDone(status = 'completed') {
  return { method: 'turn/completed', params: { threadId: THREAD, turn: turn(status) } };
}
export function defaults(cwd) {
  return {
    'config/read': { result: { config: {
      model_provider: 'openai', mcp_servers: {},
    }, origins: {}, layers: null } },
    'account/read': { result: signedIn },
    'model/list': { result: { data: models, nextCursor: null } },
    'account/rateLimits/read': { result: usage },
    'thread/list': { result: { data: [thread(cwd)], nextCursor: null }, cases: [
      { params: { ancestorThreadId: THREAD }, handler: { result: { data: [], nextCursor: null } } },
    ] },
    'thread/start': { result: started(cwd) },
    'thread/resume': { result: started(cwd) },
    'thread/read': { result: { thread: thread(cwd) } },
    'thread/turns/list': { result: { data: [], nextCursor: null } },
    'thread/items/list': { result: { data: [], nextCursor: null } },
    'turn/start': { result: { turn: turn() }, after: [{ method: 'turn/started', params: { threadId: THREAD, turn: turn() } }] },
    'turn/interrupt': { result: {} },
    'account/login/start': { result: { type: 'chatgpt', loginId: 'login-fixture', authUrl: 'https://auth.openai.com/authorize?state=synthetic' } },
    'account/login/cancel': { result: {} },
    'account/logout': { result: {} },
  };
}
