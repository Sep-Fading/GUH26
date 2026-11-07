// The figures the Agents page reads from /api/analytics/agents, and the words it shows them in.

export type AgentState = 'running' | 'healthy' | 'stopped' | 'failed' | 'idle';

export type AgentEvent =
  | { kind: 'response'; at: number; status: string; seconds: number; tokens: number }
  | { kind: 'task'; at: number; started: number; ended: number | null; status: string; summary: string };

export type AgentStats = {
  name: string; main: boolean; id: string | null; state: AgentState;
  lastActive: number | null; runningSince: number | null; runningMs: number | null;
  responses: { count: number; completed: number; interrupted: number; failed: number;
    averageSeconds: number; busySeconds: number; share: number };
  tokens: { total: number; input: number; output: number; perResponse: number };
  tasks: { count: number; done: number; failed: number; cancelled: number; declined: number; running: number;
    averageSeconds: number | null; share: number };
  recent: AgentEvent[];
  // Responses running right now, which the kill switch can stop.
  live?: ActiveTurn[];
  // Tasks this agent has handed to others that are still going, and the agents it is doing tasks for
  // ('' is the main agent).
  delegatingTo?: Delegating[];
  workingFor?: string[];
};

export type Delegating = { agent: string; status: 'asking' | 'running'; started: number; mode: string; summary: string };

// "Builder", or "Builder and Tester", or "Builder and 2 others". Names the main agent for ''.
export function names(list: string[]) {
  const shown = [...new Set(list.map(name => name || MAIN_AGENT))];
  if (shown.length <= 2) return shown.join(' and ');
  return `${shown[0]} and ${shown.length - 1} others`;
}

// One response running right now. `agent` is '' for the main agent.
export type ActiveTurn = { threadId: string; turnId: string; agent: string; conversation: string;
  startedAt: number | null; stopping: boolean };

// A running swarm as the running box shows it: its agents, their links and what happened last.
export type SwarmMember = { name: string; role: 'lead' | 'dev' | 'review' | 'test'; model: string; linkUp: boolean; reachable: boolean;
  status: string; issue: number | null; thread: string | null; queued: number };
export type LiveSwarm = { id: string; goal: string; status: string; members: SwarmMember[];
  events: { at: number; kind: string; text: string }[]; counts: { done: number; total: number } };

export type AgentReport = { days: number; from: number; now: number; agents: AgentStats[]; active?: ActiveTurn[];
  timeline?: Timeline };

// One block of the timeline: working time that finished, was stopped or failed (ms), and failures that ended in it.
export type Block = { ok: number; stopped: number; failed: number; failures: number };
export type TimelineRow = { name: string; okMs: number; stoppedMs: number; failedMs: number; downtime: number;
  failures: number; blocks: Block[] };
export type Timeline = { from: number; to: number; width: number; count: number; agents: TimelineRow[] };

export type BlockState = 'empty' | 'ok' | 'stopped' | 'failed';

// A block is as bad as the worst thing in it.
export function blockState(block: Block): BlockState {
  if (block.failed > 0 || block.failures > 0) return 'failed';
  if (block.stopped > 0) return 'stopped';
  return block.ok > 0 ? 'ok' : 'empty';
}

// Adds rows block by block, for the all-agents line.
export function combine(rows: TimelineRow[], count: number, name = 'All agents'): TimelineRow {
  const blocks = Array.from({ length: count }, (_, index) => rows.reduce<Block>((sum, row) => {
    const block = row.blocks[index];
    return block ? { ok: sum.ok + block.ok, stopped: sum.stopped + block.stopped, failed: sum.failed + block.failed,
      failures: sum.failures + block.failures } : sum;
  }, { ok: 0, stopped: 0, failed: 0, failures: 0 }));
  const okMs = rows.reduce((sum, row) => sum + row.okMs, 0);
  const stoppedMs = rows.reduce((sum, row) => sum + row.stoppedMs, 0);
  const failedMs = rows.reduce((sum, row) => sum + row.failedMs, 0);
  const total = okMs + stoppedMs + failedMs;
  return { name, okMs, stoppedMs, failedMs, downtime: total ? (stoppedMs + failedMs) / total : 0,
    failures: rows.reduce((sum, row) => sum + row.failures, 0), blocks };
}

// Gives each agent its running responses. An agent with one is Running, since the earliest of them.
export function withLive(agents: AgentStats[], active: ActiveTurn[]): AgentStats[] {
  return agents.map(agent => {
    const live = active.filter(turn => agent.main ? turn.agent === '' : turn.agent === agent.name);
    if (!live.length) return { ...agent, live: [] };
    const starts = [agent.runningSince, ...live.map(turn => turn.startedAt)].filter((at): at is number => at !== null);
    return { ...agent, live, state: 'running', runningSince: starts.length ? Math.min(...starts) : agent.runningSince };
  });
}

export const MAIN_AGENT = 'MiniClaw';

export const stateWords: Record<AgentState, string> = {
  running: 'Running', healthy: 'Healthy', stopped: 'Stopped', failed: 'Failed', idle: 'Idle',
};

// A card for an agent that did nothing in the range.
export function emptyStats(name: string, id: string | null = null): AgentStats {
  return { name, main: false, id, state: 'idle', lastActive: null, runningSince: null, runningMs: null,
    responses: { count: 0, completed: 0, interrupted: 0, failed: 0, averageSeconds: 0, busySeconds: 0, share: 0 },
    tokens: { total: 0, input: 0, output: 0, perResponse: 0 },
    tasks: { count: 0, done: 0, failed: 0, cancelled: 0, declined: 0, running: 0, averageSeconds: null, share: 0 },
    recent: [] };
}

// The recorded agents plus every agent the user has made, so a quiet agent still has a card. The main agent is
// first, then running agents, then by how much each did.
export function withKnownAgents(recorded: AgentStats[], known: { id: string; name: string }[]): AgentStats[] {
  const rows = recorded.map(agent => agent.main ? { ...agent, name: MAIN_AGENT } : agent);
  if (!rows.some(agent => agent.main)) rows.push({ ...emptyStats(MAIN_AGENT), main: true });
  for (const agent of known) {
    if (!rows.some(row => !row.main && (row.id === agent.id || row.name === agent.name))) rows.push(emptyStats(agent.name, agent.id));
  }
  const rank = (agent: AgentStats) => agent.main ? 0 : agent.state === 'running' ? 1 : 2;
  return rows.sort((a, b) => rank(a) - rank(b) || b.responses.count + b.tasks.count - a.responses.count - a.tasks.count
    || a.name.localeCompare(b.name));
}

// Each agent's part of all responses or all delegated tasks, largest first, for the share chart.
export function shares(agents: AgentStats[], of: 'responses' | 'tasks') {
  const parts = agents.map(agent => ({ name: agent.name, count: agent[of].count })).filter(part => part.count > 0);
  const whole = parts.reduce((sum, part) => sum + part.count, 0);
  return parts.map(part => ({ ...part, share: whole ? part.count / whole : 0 })).sort((a, b) => b.count - a.count);
}

export const percent = (share: number) => `${Math.round(share * 1000) / 10}%`;

// "45 s", "3 min", "1 h 20 min", "2 d 4 h".
export function duration(seconds: number | null | undefined) {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return '–';
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s} s`;
  const minutes = Math.round(s / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
  const days = Math.floor(hours / 24);
  return hours % 24 ? `${days} d ${hours % 24} h` : `${days} d`;
}

export function ago(at: number | null, now: number) {
  if (at === null) return 'never';
  const seconds = (now - at) / 1000;
  return seconds < 45 ? 'just now' : `${duration(seconds)} ago`;
}

export const short = (value: number) => value >= 1_000_000 ? `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 0 : 1)}M`
  : value >= 1000 ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k` : String(value);

// What one event in the activity list says.
export function eventWords(event: AgentEvent) {
  if (event.kind === 'response') {
    const how = { completed: 'Answered', interrupted: 'Stopped', failed: 'Failed' }[event.status] ?? event.status;
    return `${how} in ${duration(event.seconds)}${event.tokens ? `, ${short(event.tokens)} tokens` : ''}`;
  }
  const how = { asking: 'Waiting for approval', running: 'Working on', done: 'Finished', failed: 'Failed',
    cancelled: 'Cancelled', declined: 'Declined' }[event.status] ?? event.status;
  const took = event.ended ? ` (${duration((event.ended - event.started) / 1000)})` : '';
  return `${how}: ${event.summary || 'a delegated task'}${took}`;
}
