// Keeps skill management separate from conversation state.
import { Skills } from './skill-state.svelte';
// Separates group settings from individual skill activation.
import { SkillGroups, type SkillSelection } from './skill-groups.svelte';
// Keeps project settings separate from conversation state.
import { Projects } from './project-state.svelte';
// Shares provider quota shapes and defaults.
import { unavailableUsage, type Usage } from './usage';
// Knows whether the backend runs on Windows, which has no VMs.
import { words } from './platform.svelte';

export type Task = { id: string; name: string; model: string | null;
  createdAt?: number; updatedAt?: number;
  effort?: string | null; speed?: string | null; computerUse?: boolean; agentId?: string;
  agentName?: string; agentRevision?: number; projectId?: string; projectName?: string;
  // The conversation's own folder under Documents/MiniClaw, once it has one.
  folder?: string | null;
  environment?: { kind: 'host' | 'vm' }; vmId?: string;
  environmentRequired?: boolean; vmOffline?: boolean; provider?: 'claude';
  // Present while the user's own name lies over the provider's.
  providerName?: string };
export type Agent = {
  id: string; revision: number; name: string; instructions: string; active: boolean;
  model: string | null; effort: string | null; speed?: string | null;
  capabilities: ('computer_use' | 'deep_research')[];
  access: { applications: { id: string; actions: ('observe' | 'control')[] }[];
    files: { path: string; read: boolean; write: boolean }[]; network: string[] };
  delegatesTo: string[];
  // How many delegations one user message may lead to, and how long one may run.
  delegation: { maxPerTurn: number; minutes: number };
  skillIds: string[];
  skillGroupIds?: string[];
};
export type AgentDraft = Omit<Agent, 'id' | 'revision'> & { revision?: number };
export type PermissionTarget =
  | { target_type: 'application'; id: string; action: 'observe' | 'control' }
  | { target_type: 'file'; path: string; action: 'read' | 'write' }
  | { target_type: 'folder'; path: string; action: 'read' | 'write' }
  | { target_type: 'command'; kind: 'shell' | 'applescript'; text: string; cwd: string }
  // A program by name: any plain command line made of allowed programs runs without asking.
  | { target_type: 'program'; name: string }
  // A DNS name or domain a shell command may reach over HTTPS, and an on/off switch.
  | { target_type: 'host'; host: string }
  | { target_type: 'domain'; domain: string }
  | { target_type: 'feature'; name: 'unsandboxed_shell' | 'default_full_access' | 'network' | 'web_fetch' | 'subagents' | 'task_tracking' | 'image_generation' | 'plan_mode' | 'coding_session' | 'no_web_search' }
  // A switch for one saved MCP server, by its id, and one tool of such a server.
  | { target_type: 'mcp_server'; server: string }
  | { target_type: 'mcp_tool'; server: string; tool: string };
// One saved MCP server as the page sees it. It never carries a secret, only their names.
export type McpServer = { id: string; name: string; kind: 'stdio' | 'http'; command: string | null; args: string[];
  url: string | null; folder: string | null; runtimePaths?: string[]; hosts: string[]; ports: number[]; listen: number[]; unrestricted: boolean;
  provider: 'both' | 'codex' | 'claude'; startupSeconds: number; toolSeconds: number; enabledTools: string[]; disabledTools: string[];
  headers: string[]; env: string[]; hasToken: boolean;
  needs: string[]; revision: number;
  /** Windows, for a server saved as `npx`/`npm exec`: what really runs (MiniClaw's Node and npm). */
  runs?: string };
export type PermissionRule = { target: PermissionTarget; effect: 'allow' | 'block' };
export type PermissionSettings = { rules?: PermissionRule[];
  appCatalog: { id: string; name: string; code_requirement: string }[];
  appRequests: { name: string; status: 'pending' }[];
  sources?: {
  agent: PermissionRule[]; project: PermissionRule[]; conversation: PermissionRule[];
  session: { target: PermissionTarget; expiresAt: number }[];
}; effective?: { target: PermissionTarget; decision: 'allow' | 'block' | 'ask' | 'unavailable';
  source: 'agent' | 'project' | 'conversation' | 'session' | 'none' | 'catalog' }[] };
type RequestOwner = { thread?: string; agent?: string };
type Approval = RequestOwner & { id: string; message: string; app?: string | null;
  // Set for a browser, where MiniClaw's own page may be open.
  warning?: string | null };
// One agent doing one task for this conversation's agent.
export type DelegatedRun = { id: string; turnId: string; thread: string | null; agent: string; task: string;
  mode: '' | 'wait' | 'background'; status: 'asking' | 'running' | 'done' | 'failed' | 'declined' | 'cancelled';
  result: string | null; started: number; ended: number | null; detached: boolean;
  // The conversation that asked, and the pictures and files that went with the task.
  parent: string; attachments?: HandedFile[];
  // The request's one-line description, and while it runs, the line saying what it is doing now.
  summary?: string; live?: string;
  // Set when the user waited and the run, still going at the hour, continued in the background.
  handedOver?: boolean };
type DelegatedRequest = { runId: string; parent: string; thread: string; agent: string; expiresAt: number } & (
  { kind: 'scoped'; request: ScopedApproval } | { kind: 'computer'; request: Approval } |
  { kind: 'review'; request: Review } | { kind: 'question'; request: PendingQuestion });
// A picture or file of a conversation that goes with a delegated task.
export type HandedFile = { id: string; name: string; kind: 'image' | 'text' | 'file'; size: number };
// One proposed file change a coding session is holding for the user's review.
type Review = RequestOwner & { id: string; path: string; kind: 'edit' | 'write' | 'create'; diff: string; proposed: string; expiresInSeconds: number };
// `minutes` is the run's limit in the background, `waitMinutes` its limit when the conversation waits (at most an hour).
type DelegationPrompt = { id: string; agent: string; task: string; summary?: string; expiresInSeconds: number; expiresAt?: number; attachments?: HandedFile[];
  minutes?: number; waitMinutes?: number };
type GlobalDelegationPrompt = DelegationPrompt & { thread: string; threadName: string };
type ScopedApproval = { id: string; target: PermissionTarget; expiresInSeconds: number;
  // Set when a delegated agent asks: who it is, and the run's own thread that takes the answer.
  agent?: string; thread?: string;
  // Hosts a command wants, and what the wider choice would cover instead.
  hosts?: string[]; domains?: string[];
  // The programs of a plain shell line, which the popup can allow for good.
  programs?: string[];
  // For an MCP tool: the server's name, what is about to be sent, and how the server runs.
  serverName?: string; arguments?: unknown; unrestricted?: boolean; remote?: boolean };
// A question the model asked: up to four, each with options, one pick or several.
export type PendingQuestion = RequestOwner & { id: string; expiresInSeconds: number; questions: { question: string; header: string; multiSelect: boolean;
  options: { label: string; description: string }[] }[] };
// One message waiting for its turn, and a conversation's queue as the backend reports it.
// A file or picture a message carried, as the backend describes it.
export type Attachment = { id: string; name: string; kind: 'image' | 'text' | 'file'; mediaType: string; size: number };
// A file the user has added to the draft. It is uploaded when the message is sent, because a new
// conversation does not exist until then; `uploaded` is kept so a repeated send does not upload twice.
export type DraftFile = { key: string; file: File; preview: string | null; uploaded?: Attachment };
export type QueuedMessage = { id: string; text: string; clientMessageId: string; model: string | null; effort: string | null; speed?: string | null; createdAt: number;
  attachments?: Attachment[] };
export type MessageQueue = { version: number; items: QueuedMessage[]; paused: boolean; reason: string | null };
type Activity = { id: string; tool: string; app: string | null; status: string; error?: string; hint?: string };
export type Model = { model: string; displayName: string; description: string;
  hidden: boolean; isDefault: boolean; defaultReasoningEffort: string; provider?: 'claude';
  supportedReasoningEfforts: { reasoningEffort: string; description: string }[];
  // The speed tiers Codex advertises for the model; a tier's id is what a turn sends.
  serviceTiers?: { id: string; name: string; description: string }[] };
type Choice = { model: string; effort: string; speed?: string };
type TaskPage = { threads: Task[]; nextCursor?: string | null };
type Message = { id: string; role: 'user' | 'assistant'; text: string;
  complete?: boolean; turnId?: string | null; attachments?: Attachment[];
  imageStatus?: string };
// One call of MiniClaw's own tool, recorded for the user. The model never sees this.
export type ToolActivity = { id: string; turnId: string; runId?: string; kind: string; title: string; detail?: string | null;
  agent?: string | null; status: 'running' | 'ok' | 'failed' | 'refused'; startedAt: number; endedAt?: number;
  output?: string; exitCode?: number | null; diff?: string; refusedHosts?: string[];
  // Pictures a tool returned that MiniClaw kept, linked, never inlined.
  pictures?: { id: string; name: string; url: string }[] };
export type Provider = { id: 'codex' | 'claude'; enabled: boolean; installed: boolean;
  status: string; email?: string | null; planType?: string | null };
type Account = { status: string; email?: string; planType?: string; providers?: Provider[] };
type Snapshot = { thread: Task; messages: Message[]; activities?: ToolActivity[];
  delegations?: DelegatedRun[]; delegationPrompt?: DelegationPrompt | null;
  review?: Review | null; codingSession?: boolean; fullAccess?: boolean; folder?: string | null; queue?: MessageQueue; question?: PendingQuestion | null;
  activeTurn: { id: string } | null; pending: boolean; outcomeUnknown?: boolean;
  vmOffline?: boolean;
  delegatedRequests?: DelegatedRequest[]; approval?: Approval | null; computerActivity?: Activity[]; historyCursor?: string | null;
  scopedApproval?: ScopedApproval | null;
  computerChanging?: boolean; computerError?: string | null;
  imageChanging?: boolean; imageError?: string | null; imageRetryOn?: boolean | null };
type Login = { status: string; authUrl?: string;
  verificationUrl?: string; userCode?: string;
  // A Claude sign-in also reports whether it takes a pasted code, a hint, and the CLI's error.
  provider?: 'claude'; codeEntry?: boolean; notice?: string; error?: string };

class ApiError extends Error {
  // Creates a request error that records whether a send may already have reached Codex.
  constructor(message: string, public unknown = false, public code = '') { super(message); }
}

// Browser state lasts for this page; transcripts belong to Codex.
export class Chat {
  skills = new Skills((path, body) => this.api(path, body), () => this.ready && !this.disposed);
  groups = new SkillGroups((path, body) => this.api(path, body), () => this.ready && !this.disposed);
  newSkillSelection = $state<SkillSelection>({ skillIds: [], skillGroupIds: [], revision: 0 });
  projects = new Projects((path, body) => this.api(path, body), () => this.ready && !this.disposed);
  projectId = $state('');
  // What MiniClaw's tool did in the open conversation, oldest first.
  activities = $state<ToolActivity[]>([]);
  // Runs delegated from the open conversation, and the request waiting for the user's answer.
  delegations = $state<DelegatedRun[]>([]);
  delegationPrompt = $state<DelegationPrompt | null>(null);
  delegatedRequests = $state<DelegatedRequest[]>([]);
  private answeredRequests = new Set<string>();
  private requestsRevision = 0;

  // Selects one child request only after the parent's own requests have been answered.
  private get childRequest() {
    if (this.scopedApproval || this.approval || this.review || this.question) return null;
    return this.delegatedRequests.find(item => item.parent === this.active?.id
      && item.expiresAt > this.approvalNow && !this.answeredRequests.has(item.request.id)) ?? null;
  }

  // Returns the exact permission currently displayed, retaining the child's answer route.
  get pendingScopedApproval(): ScopedApproval | null {
    const child = this.childRequest;
    return this.scopedApproval ?? (child?.kind === 'scoped'
      ? { ...child.request, thread: child.thread, agent: child.agent } : null);
  }

  // Returns the current computer approval with the delegated agent's name when needed.
  get pendingApproval(): Approval | null {
    const child = this.childRequest;
    return this.approval ?? (child?.kind === 'computer'
      ? { ...child.request, thread: child.thread, agent: child.agent } : null);
  }

  // Returns the file review belonging to the parent or the next waiting child.
  get pendingReview(): Review | null {
    const child = this.childRequest;
    return this.review ?? (child?.kind === 'review'
      ? { ...child.request, thread: child.thread, agent: child.agent } : null);
  }

  // Returns a blocking question without changing the selected conversation.
  get pendingQuestion(): PendingQuestion | null {
    const child = this.childRequest;
    return this.question ?? (child?.kind === 'question'
      ? { ...child.request, thread: child.thread, agent: child.agent } : null);
  }

  // Prevents an acknowledged child request from reappearing in an older poll or snapshot.
  private answeredRequest(id: string) {
    this.requestsRevision++;
    this.answeredRequests.add(id);
    if (this.answeredRequests.size > 256) this.answeredRequests.delete(this.answeredRequests.values().next().value!);
    this.delegatedRequests = this.delegatedRequests.filter(item => item.request.id !== id);
  }

  backgroundDelegations = $state<GlobalDelegationPrompt[]>([]);
  delegationDeciding = $state(false);
  private answeredDelegations = new Set<string>();
  private approvalNow = $state(Date.now());

  // Keeps an unanswered request visible regardless of the selected conversation.
  get pendingDelegation(): GlobalDelegationPrompt | null {
    const local = this.delegationPrompt && this.active ? {
      ...this.delegationPrompt, thread: this.active.id, threadName: this.active.name,
    } : null;
    const prompt = [local, ...this.backgroundDelegations].find(prompt => prompt
      && !this.answeredDelegations.has(prompt.id)
      && (!prompt.expiresAt || prompt.expiresAt > this.approvalNow));
    return prompt ? { ...prompt, expiresInSeconds: prompt.expiresAt
      ? Math.max(0, Math.ceil((prompt.expiresAt - this.approvalNow) / 1000)) : prompt.expiresInSeconds } : null;
  }
  // The change waiting for review, and whether this conversation reviews every change.
  review = $state<Review | null>(null);
  codingSession = $state(false);
  // Set while this conversation runs with full access: nothing the agent is allowed asks.
  fullAccess = $state(false);
  // The open conversation's own folder under Documents/MiniClaw, once it has one.
  folder = $state<string | null>(null);
  // Explicit tool choices for the unsent conversation, saved with its first creation request.
  draftCapabilities = $state<Record<string, boolean>>({});
  // The question the model is waiting on, if any.
  question = $state<PendingQuestion | null>(null);
  // Messages waiting for their turn in this conversation, kept by the backend.
  queue = $state<MessageQueue>({ version: 0, items: [], paused: false, reason: null });
  // Set once Stop was pressed, until the response has really ended; nothing is queued meanwhile.
  stopRequested = $state(false);
  newMachine = $state<'host' | 'vm'>('host');
  assigningMachine = $state(false);
  deleting = $state(false);
  account = $state<Account>({ status: 'checking' });
  providerUsage = $state<Record<string, Usage>>({});
  usageLoading = $state(false);
  private usageAccounts = $state<Record<string, string>>({});
  tasks = $state<Task[]>([]);
  workingTasks = $state<string[]>([]);
  readyReplies = $state<string[]>([]);
  private seenReplies = new Map<string, string>();
  private openedAt = Date.now();
  private checkingWork = false;
  agents = $state<Agent[]>([]);
  agentId = $state('');
  agentsLoading = $state(false);
  agentSaving = $state(false);
  agentError = $state('');
  active = $state<Task | null>(null);
  messages = $state<Message[]>([]);
  historyCursor = $state<string | null>(null);
  historyLoading = $state(false);
  draft = $state('');
  // Files added to the draft; they belong to the open conversation and leave with it.
  files = $state<DraftFile[]>([]);
  error = $state('');
  vmStatus = $state<'off' | 'starting' | 'running' | 'failed' | 'stopping' | 'unknown' | 'missing'>('unknown');
  vmReady = $state(false);
  vmReason = $state('');
  vmApproval = $state(false);
  vmStarting = $state(false);
  private vmApproved = false;
  private vmReloading = false;
  private vmApprovalId = '';
  private vmApprovalVmId = 'master';
  private vmWaitingMessageId = '';
  private vmWaitingText = '';
  private vmSendMessageId = '';
  turn = $state<string | null>(null);
  working = $state(false);
  pending = $state(false);
  uncertain = $state(false);
  reconnecting = $state(false);
  login = $state<Login>({ status: 'idle' });
  models = $state<Model[]>([]);
  model = $state('');
  effort = $state('');
  // A speed tier's id, or '' for standard.
  speed = $state('');
  modelsLoading = $state(false);
  modelError = $state('');
  // The Mac and each VM have their own Computer Use helper.
  hostComputer = $state(false);
  // Why this machine's helper cannot be used now, as the backend words it; empty when unknown.
  hostComputerReason = $state('');
  vmComputerReady = $state(false);
  computerUse = $state(false);
  switching = $state(false);
  computerPending = $state(false);
  computerError = $state('');
  imageChanging = $state(false);
  imageError = $state('');
  imageRetryOn = $state<boolean | null>(null);
  approval = $state<Approval | null>(null);
  scopedApproval = $state<ScopedApproval | null>(null);
  computerActivity = $state<Activity[]>([]);
  deciding = $state('');
  private choices = new Map<string, Choice>();
  private identity = 0;
  private catalogRead = 0;
  private agentWrite = 0;
  private taskWrite = 0;
  private feed?: EventSource;
  private disposed = false;
  refreshing = $state(false);
  private selected = 0;
  private finished = new Set<string>();
  // Which user message started which turn in the open conversation, as the stream said.
  private startedTurns = new Map<string, string>();
  private drafts = new Map<string, string>();
  private knownAccount = false;
  private accountEmail?: string;
  private older: Message[] = [];
  private historyPaged = false;
  private composingNew = $state(false);
  private recoveryError = '';

  // Reads, saves, or clears the selected task ID; returns null if storage is unavailable.
  private savedTask(id?: string | null) {
    try {
      if (id === null) localStorage.removeItem('miniclaw.activeTask');
      else if (id !== undefined) localStorage.setItem('miniclaw.activeTask', id);
      return localStorage.getItem('miniclaw.activeTask');
    } catch { return null; }
  }

  // Returns saved tasks across bounded pages, removing duplicates and rejecting cursor loops.
  private async taskList() {
    const tasks = new Map<string, Task>();
    const seen = new Set<string>();
    let cursor: string | null = null;
    do {
      const query: string = cursor ? `?cursor=${encodeURIComponent(cursor)}` : '';
      const page: TaskPage = await this.api<TaskPage>(
        `/api/threads${query}`,
      );
      for (const task of page.threads) tasks.set(task.id, task);
      cursor = page.nextCursor ?? null;
      if (cursor && (seen.has(cursor) || seen.size >= 100)) {
        throw new Error('Could not finish loading saved tasks.');
      }
      if (cursor) seen.add(cursor);
    } while (cursor);
    return [...tasks.values()];
  }

  // Returns whether current work, reconnecting, or an unknown outcome blocks sending.
  get busy() {
    return this.responding || this.switching || this.computerPending || !!this.computerError
      || this.imageChanging || !!this.imageError
      || this.assigningMachine;
  }
  // Locks the draft during response work, reconnection or initial chat restoration.
  get responding() {
    return this.working || this.vmStarting || this.pending || !!this.turn ||
      this.uncertain || this.reconnecting ||
      (this.refreshing && !this.active && !this.composingNew);
  }
  // Returns whether the latest account state confirms ChatGPT sign-in.
  get ready() { return this.account.status === 'signed_in'; }
  // Blocks new work when the chosen profile is inactive or cannot be verified.
  get agentReady() {
    return this.projectReady && (!this.agentId || (!this.agentError &&
      this.agents.some(agent => agent.id === this.agentId && agent.active)));
  }
  // Returns the selected project settings, independently of the active conversation.
  get project() { return this.projects.items.find(p => p.id === this.projectId); }
  // Blocks project sends when settings are unavailable or the chosen agent was removed.
  get projectReady() {
    return !this.projectId || (!this.projects.error && !!this.project &&
      !!this.project.environment &&
      (!this.agentId || this.project.agentIds.includes(this.agentId)));
  }
  // Shows the permanent choice on saved chats and the draft choice on new ones.
  get machine() { return this.active?.environment?.kind ?? (this.active ? '' : this.newMachine); }
  // Returns the pinned VM identity, using the project's VM for a new project task.
  get vmTarget() { return this.active?.environment?.kind === 'vm'
    ? this.active.vmId ?? 'master'
    : this.project?.environment?.kind === 'vm'
      ? this.project.environment.vmId ?? 'master' : 'master'; }
  // Gives status and approval controls the current machine's short name.
  // Computer Use needs the helper of the machine this chat runs on.
  get computerAvailable() { return this.machine === 'vm' ? this.vmComputerReady : this.hostComputer; }
  get vmName() { return this.vmTarget === 'master' ? 'Master VM' : 'Project VM'; }
  // Requires old chats to be assigned before another message can start.
  get machineReady() { return !this.active || !!this.active.environment; }

  // Reads passive Master status without asking the supervisor to start it. Windows has no VMs, and
  // its backend answers their status with 503, so there the page does not ask; nor on Linux.
  async refreshVmStatus() {
    if (!words.mac) {
      this.vmStatus = 'unknown';
      this.vmReady = false;
      this.vmComputerReady = false;
      this.vmReason = '';
      return { state: 'unknown' as const, ready: false };
    }
    try {
      const status = await this.api<{ state: 'off' | 'starting' | 'running' | 'failed' | 'stopping' | 'unknown' | 'missing';
        ready: boolean; reason?: string; computerReady?: boolean }>(
        `/api/vms/${this.vmTarget}/status`);
      this.vmStatus = status.state;
      this.vmReady = status.ready === true;
      this.vmComputerReady = status.computerReady === true;
      this.vmReason = status.reason ?? '';
      // A task opened while its VM was off is only a copy kept on the Mac. Once the VM is
      // ready, load the real task, or the backend refuses a message for a task it never loaded.
      if (this.vmReady && this.active?.vmOffline && !this.vmReloading && !this.busy) {
        this.vmReloading = true;
        void this.select(this.active).catch(() => {}).finally(() => { this.vmReloading = false; });
      }
      return status;
    } catch (error) {
      this.vmStatus = 'unknown';
      this.vmReady = false;
      this.vmComputerReady = false;
      this.vmReason = '';
      throw error;
    }
  }

  // Discards a waiting send without starting Master or clearing its draft.
  declineVmStart() {
    const approvalId = this.vmApprovalId;
    const vmId = this.vmApprovalVmId;
    this.vmApproval = false;
    this.vmApprovalId = '';
    this.vmWaitingMessageId = '';
    this.vmWaitingText = '';
    if (approvalId) void this.api(`/api/vms/${vmId}/decline`, { approvalId }).catch(() => {});
  }

  // Starts the selected VM after an explicit click, then sends the same draft once ready.
  async approveVmStart() {
    if (!this.vmApproval || this.vmStarting || this.machine !== 'vm' || !this.draft.trim()) return;
    if (this.draft !== this.vmWaitingText) {
      this.declineVmStart();
      this.error = `The message changed. Press Send again before starting ${this.vmName}.`;
      return;
    }
    const text = this.draft;
    const approvalId = this.vmApprovalId;
    const vmId = this.vmApprovalVmId;
    const messageId = this.vmWaitingMessageId;
    const selection = this.selected;
    const identity = this.identity;
    this.vmApproval = false;
    this.vmStarting = true;
    this.error = '';
    try {
      await this.api(`/api/vms/${vmId}/start`,
        { approvalId, clientMessageId: messageId });
      for (let attempt = 0; attempt < 120; attempt++) {
        if (this.disposed || selection !== this.selected || identity !== this.identity) return;
        const status = await this.refreshVmStatus();
        if (status.state === 'failed' || status.state === 'off' || status.state === 'unknown') {
          throw new Error(status.reason ?? (status.state === 'off'
            ? `${this.vmName} stopped before the message was sent.`
            : `${this.vmName} status is unavailable. The message was not sent.`));
        }
        if (status.state === 'running' && status.ready) {
          if (this.draft !== text || this.machine !== 'vm') return;
          if (this.active?.vmOffline) {
            await this.select(this.active);
            if (this.active?.vmOffline || this.draft !== text) {
              throw new Error(`${this.vmName} is ready, but this task could not reload. The message was not sent.`);
            }
          }
          this.vmApproved = true;
          this.vmSendMessageId = messageId;
          this.vmStarting = false;
          await this.send();
          return;
        }
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
      throw new Error(`${this.vmName} did not become ready. The message was not sent.`);
    } catch (error) {
      if (selection === this.selected && identity === this.identity) this.fail(error);
    } finally {
      this.vmStarting = false;
      this.vmApproved = false;
      this.vmSendMessageId = '';
      this.declineVmStart();
    }
  }

  // Makes a new choice or pins an older chat to its existing worker once.
  async chooseMachine(value: string) {
    if (value !== 'host' && value !== 'vm') return;
    if (!this.active) {
      if (this.newMachine !== value) this.resetDraftCapabilities();
      this.newMachine = value; this.declineVmStart(); return;
    }
    if (this.active.environment || this.assigningMachine) return;
    const task = this.active;
    this.assigningMachine = true; this.error = '';
    try {
      const { environment } = await this.api<{ environment: { kind: 'host' | 'vm' } }>(
        `/api/threads/${task.id}/environment`, { kind: value });
      if (this.active?.id !== task.id || this.disposed) return;
      this.active = { ...this.active, environment, environmentRequired: false };
      this.declineVmStart();
      this.tasks = this.tasks.map(item => item.id === task.id
        ? { ...item, environment, environmentRequired: false } : item);
    } catch (error) { if (this.active?.id === task.id) this.fail(error); }
    finally { this.assigningMachine = false; }
  }
  // Lists globally active agents that this project's settings also permit.
  get availableAgents() {
    return this.agents.filter(a => a.active && (!this.projectId ||
      this.project?.agentIds.includes(a.id)));
  }

  // Deletes an observed profile revision; existing chats retain their pinned identity.
  async deleteAgent(id: string, revision: number) {
    if (!this.ready || this.agentSaving) throw new Error('Agent settings are busy.');
    const identity = this.identity;
    this.agentSaving = true;
    try {
      await this.api(`/api/agents/${id}/delete`, { revision });
      if (identity !== this.identity || this.disposed) throw new Error('Account changed.');
      this.agentWrite++;
      this.agents = this.agents.filter(a => a.id !== id);
      if (!this.active && this.agentId === id) this.agentId = '';
    } finally { if (identity === this.identity) this.agentSaving = false; }
  }

  // Drops a deleted project's conversations from the page and leaves one that was open.
  async afterProjectDeleted(projectId: string) {
    const gone = this.tasks.filter(task => task.projectId === projectId);
    if (this.active?.projectId === projectId || this.projectId === projectId) await this.newChat();
    this.taskWrite++;
    this.tasks = this.tasks.filter(task => task.projectId !== projectId);
    this.readyReplies = this.readyReplies.filter(id => !gone.some(task => task.id === id));
    for (const task of gone) {
      this.drafts.delete(task.id); this.choices.delete(task.id); this.seenReplies.delete(task.id);
    }
  }

  // Removes a chat only after acknowledgement, retaining other chats and drafts.
  // Gives a conversation the user's own name; an empty name brings back the one it had.
  async renameChat(task: Task, name: string) {
    const { thread } = await this.api<{ thread: Task }>(`/api/threads/${task.id}/name`, { name });
    this.tasks = this.tasks.map(item => item.id === task.id ? { ...item, name: thread.name, providerName: thread.providerName } : item);
    if (this.active?.id === task.id) this.active = { ...this.active, name: thread.name, providerName: thread.providerName };
  }

  // `deleteFolder` also moves the conversation's own folder to the Trash; otherwise it stays.
  // A folder the backend could not move is kept, and the user is told where it is.
  async deleteChat(task: Task, deleteFolder = false) {
    if (!this.ready || this.deleting || this.working || this.switching) throw new Error('Chat is busy.');
    const identity = this.identity;
    this.deleting = true;
    try {
      const answer = await this.api<{ folderKept?: boolean; folder?: string | null }>(
        `/api/threads/${task.id}/delete`, deleteFolder ? { deleteFolder: true } : {});
      if (identity !== this.identity || this.disposed) throw new Error('Account changed.');
      if (this.active?.id === task.id) await this.newChat(task.projectId);
      this.taskWrite++;
      this.tasks = this.tasks.filter(t => t.id !== task.id);
      this.readyReplies = this.readyReplies.filter(id => id !== task.id);
      this.seenReplies.delete(task.id);
      this.drafts.delete(task.id); this.choices.delete(task.id);
      if (answer?.folderKept) {
        const place = answer.folder ?? task.folder;
        this.error = `The conversation was deleted; its folder could not be moved to ${words.folderTrash}`
          + (place ? ` and was kept at ${place}.` : ' and was kept.');
      }
    } finally { if (identity === this.identity) this.deleting = false; }
  }
  // Returns the models this chat can use; an open chat stays with its provider.
  get modelChoices() {
    const active = this.active;
    return active ? this.models.filter(model => model.provider === active.provider) : this.models;
  }
  // Returns whether the open chat, or the model for a new one, runs through Claude Code.
  get claude() { return (this.active?.provider ?? this.selectedModel?.provider) === 'claude'; }
  /** Selects limits belonging to this conversation's provider only. */
  get usage() { return this.usageFor(this.claude ? 'claude' : 'codex'); }

  /** Hides cached readings as soon as that provider's identity or availability changes. */
  usageFor(id: string) {
    const provider = this.account.providers?.find(provider => provider.id === id);
    if (provider && (!provider.enabled || provider.status !== 'signed_in')) return unavailableUsage();
    if (provider && this.usageAccounts[id] !== this.usageKey(provider)) return unavailableUsage();
    return this.providerUsage[id] ?? unavailableUsage();
  }

  /** Identifies the account a quota reading belongs to without retaining credentials. */
  private usageKey(provider: { status: string; email?: string | null; enabled: boolean }) {
    return `${provider.status}:${provider.email ?? ''}:${provider.enabled}`;
  }
  // Returns the selected catalog entry, or undefined if it is no longer listed.
  get selectedModel() { return this.models.find(model => model.model === this.model); }
  // Returns whether the model and effort are valid, allowing unset model defaults.
  get validChoice() {
    if (!this.model) return true;
    const model = this.selectedModel;
    return !this.modelError && !!model &&
      (model.supportedReasoningEfforts.length === 0 ? this.effort === '' :
        model.supportedReasoningEfforts.some(e => e.reasoningEffort === this.effort)) &&
      (this.speed === '' || (model.serviceTiers ?? []).some(tier => tier.id === this.speed));
  }

  // The tiers the selected model offers, and the one in use.
  get speedChoices() { return this.selectedModel?.serviceTiers ?? []; }
  get speedName() { return this.speedChoices.find(tier => tier.id === this.speed)?.name ?? ''; }

  // Sets a speed the selected model offers, or standard, when the controls are unlocked.
  setSpeed(speed: string) {
    if (this.busy || !this.ready) return;
    if (speed === '' || this.speedChoices.some(tier => tier.id === speed)) this.speed = speed;
  }

  // Refreshes profile metadata without replacing the selected conversation or draft.
  async refreshAgents() {
    if (!this.ready || this.disposed || this.agentsLoading || this.agentSaving) return;
    const identity = this.identity;
    const version = this.agentWrite;
    this.agentsLoading = true;
    try {
      const result = await this.api<{ agents: Agent[] }>('/api/agents');
      if (this.disposed || identity !== this.identity || version !== this.agentWrite) return;
      if (!Array.isArray(result.agents)) throw new Error('Agents unavailable');
      this.agents = result.agents;
      this.agentError = '';
    } catch (error) {
      if (identity === this.identity && version === this.agentWrite) {
        this.agentError = error instanceof Error ? error.message : 'Agents unavailable';
      }
    } finally { if (identity === this.identity) this.agentsLoading = false; }
  }

  // Saves an explicit profile edit and returns it; failures preserve the form for retry.
  async saveAgent(id: string | undefined, draft: AgentDraft): Promise<Agent | undefined> {
    if (!this.ready || this.agentSaving) return;
    const identity = this.identity;
    this.agentSaving = true;
    this.agentError = '';
    try {
      const { agent } = await this.api<{ agent: Agent }>(
        id ? `/api/agents/${id}` : '/api/agents', draft,
      );
      if (this.disposed || identity !== this.identity) return;
      // A completed edit takes precedence over older list responses.
      this.agentWrite += 1;
      this.agents = [...this.agents.filter(value => value.id !== agent.id), agent];
      return agent;
    } catch (error) {
      if (identity === this.identity) {
        this.agentError = error instanceof Error ? error.message : 'Could not save agent';
      }
    } finally { if (identity === this.identity) this.agentSaving = false; }
  }

  // Chooses an activated agent for an empty chat without discarding its draft.
  chooseAgent(id: string) {
    if (this.active || this.busy || !this.ready) return;
    const agent = this.availableAgents.find(agent => agent.id === id);
    if (id && !agent) return;
    if (id !== this.agentId) this.resetDraftCapabilities();
    this.agentId = id;
    this.computerUse = false;
    if (agent?.model) this.restoreChoice({ model: agent.model, effort: agent.effort, speed: agent.speed });
  }

  // Refreshes model choices without discarding an existing selection; reports catalog
  // failure separately and ignores replies from an older account or request.
  async refreshModels(force = false) {
    if (!this.ready || this.disposed || this.modelsLoading) return;
    const request = ++this.catalogRead;
    const identity = this.identity;
    this.modelsLoading = true;
    try {
      const result = await this.api<{ models: Model[] }>(`/api/models${force ? '?refresh=true' : ''}`);
      if (this.disposed || identity !== this.identity || request !== this.catalogRead) return;
      if (!Array.isArray(result.models)) throw new Error('Invalid model catalog');
      this.models = result.models;
      this.modelError = '';
      // Keep an existing choice visible even if Codex no longer lists it.
      if (!this.model) {
        const model = this.models.find(model => model.isDefault) ?? this.models[0];
        if (model) this.restoreChoice({ model: model.model, effort: null });
      }
    } catch {
      if (!this.disposed && identity === this.identity && request === this.catalogRead) {
        this.modelError = 'Models unavailable';
      }
    } finally {
      if (request === this.catalogRead) this.modelsLoading = false;
    }
  }

  // Selects an available model and its default effort when the controls are unlocked.
  setModel(name: string) {
    if (this.busy || !this.ready || !this.models.some(model => model.model === name)) return;
    const wasClaude = this.claude;
    this.restoreChoice({ model: name, effort: null });
    if (!this.active && wasClaude !== this.claude) this.resetDraftCapabilities();
  }

  // Sets a supported effort for the selected model when the controls are unlocked.
  setEffort(effort: string) {
    if (this.busy || !this.ready) return;
    if (this.selectedModel?.supportedReasoningEfforts
      .some(option => option.reasoningEffort === effort)) this.effort = effort;
  }

  // Sets computer access for the next prompt; existing chats wait for backend confirmation.
  async setComputer(enabled: boolean) {
    if (enabled && this.agentId) return;
    if (this.responding || this.switching || this.computerPending ||
      !this.ready || (enabled && !this.computerAvailable)) return;
    if (!this.active) { this.computerUse = enabled; return; }
    const selection = this.selected;
    this.switching = true;
    this.error = '';
    try {
      const snapshot = await this.api<Snapshot>(
        `/api/threads/${this.active.id}/computer`, { enabled },
      );
      if (!this.disposed && selection === this.selected) this.applyComputer(snapshot);
    } catch (error) {
      if (selection === this.selected) this.fail(error);
    } finally { this.switching = false; }
  }

  // Applies only tool state so the backend reload leaves messages and drafts in place.
  private applyComputer(state: Pick<Snapshot, 'thread' | 'computerChanging' | 'computerError'>) {
    this.computerUse = state.thread.computerUse === true;
    this.computerPending = state.computerChanging ?? false;
    this.computerError = state.computerError ?? '';
    if (this.active) this.active = { ...this.active, computerUse: this.computerUse };
  }

  // Restores task settings, using the catalog's default effort when none was saved.
  private restoreChoice(choice: { model: string | null; effort?: string | null; speed?: string | null }) {
    this.model = choice.model ?? '';
    const model = this.selectedModel;
    this.effort = choice.effort ?? (model?.supportedReasoningEfforts.length
      ? model.defaultReasoningEffort : '');
    // Standard unless a saved speed is one the model still offers.
    const speed = choice.speed ?? '';
    this.speed = (model?.serviceTiers ?? []).some(tier => tier.id === speed) ? speed : '';
  }

  // Returns model, effort and speed fields for a request, omitting unset choices.
  private settings() {
    return this.model ? { model: this.model, effort: this.effort || undefined, speed: this.speed || undefined } : {};
  }

  // Adds files to the draft, up to ten, within the sizes the backend accepts.
  addFiles(files: Iterable<File>) {
    for (const file of files) {
      if (this.files.length >= 10) { this.error = 'A message can carry up to 10 files.'; return; }
      const picture = /^image\/(png|jpeg|gif|webp)$/.test(file.type);
      if (file.size === 0) { this.error = `${file.name} is empty.`; continue; }
      if (file.size > (picture ? 20 : 500) * 1024 * 1024) {
        this.error = picture ? `${file.name} is larger than 20 MB.` : `${file.name} is larger than 500 MB.`;
        continue;
      }
      this.files.push({ key: crypto.randomUUID(), file, preview: picture ? URL.createObjectURL(file) : null });
    }
  }

  // Takes one file out of the draft; an upload the backend already holds is removed there too.
  removeFile(key: string) {
    const found = this.files.find(item => item.key === key);
    if (!found) return;
    this.files = this.files.filter(item => item.key !== key);
    if (found.preview) URL.revokeObjectURL(found.preview);
    if (found.uploaded && this.active) {
      void fetch(`/api/threads/${this.active.id}/attachments/${found.uploaded.id}`, { method: 'DELETE',
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' }, body: '{}' }).catch(() => {});
    }
  }

  // Forgets the draft's files once their message was accepted.
  private clearFiles(sent: DraftFile[]) {
    for (const item of sent) if (item.preview) URL.revokeObjectURL(item.preview);
    this.files = this.files.filter(item => !sent.includes(item));
  }

  // Uploads the draft's files to a conversation and returns their ids, in order.
  private async uploadFiles(thread: string, files: DraftFile[]): Promise<string[]> {
    for (const item of files) {
      if (item.uploaded) continue;
      // A pasted picture has no useful name of its own.
      const name = item.file.name && !item.file.name.startsWith('.') ? item.file.name.replace(/[\/\\:]/g, '-') : 'pasted.png';
      let response: Response;
      try {
        response = await fetch(`/api/threads/${thread}/attachments?name=${encodeURIComponent(name)}`, { method: 'POST', cache: 'no-store',
          headers: { 'Content-Type': 'application/octet-stream', 'X-MiniClaw-Request': '1' }, body: item.file });
      } catch { throw new ApiError(`${name} could not be uploaded. The message was not sent.`, false); }
      const value = await response.json().catch(() => ({}));
      if (!response.ok) throw new ApiError(`${name}: ${value.error?.message ?? 'The upload failed.'}`, false, value.error?.code ?? '');
      item.uploaded = value.attachment as Attachment;
    }
    return files.map(item => item.uploaded!.id);
  }

  // Makes a same-origin GET or JSON POST and returns its parsed response.
  // Errors preserve whether a failed POST may already have been accepted.
  private async api<T>(path: string, body?: unknown): Promise<T> {
    try {
      const response = await fetch(path, {
        method: body === undefined ? 'GET' : 'POST', cache: 'no-store',
        headers: body === undefined ? {} : {
          'Content-Type': 'application/json', 'X-MiniClaw-Request': '1',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(35000),
      });
      const value = await response.json();
      if (!response.ok) {
        throw new ApiError(value.error?.message ?? 'Request failed.',
          value.error?.outcomeUnknown === true, value.error?.code ?? '');
      }
      return value as T;
    } catch (error) {
      if (error instanceof ApiError) throw error;
      // A failed POST may have reached the backend; preserve that uncertainty.
      throw new ApiError('Connection lost. Check the backend before retrying.',
        body !== undefined);
    }
  }

  // Loads saved permission sources for one settings scope without changing the chat.
  // Reads or changes the VM budget for the settings page; only these routes are reachable.
  virtualisation<T>(action: '' | 'limits' | 'reconcile' | 'forget', body?: unknown) {
    return this.api<T>(`/api/virtualisation${action ? `/${action}` : ''}`, body);
  }

  // Reads one VM's state for its controls, without asking it to start.
  vmState(vmId: string) {
    return this.api<{ state: string; ready: boolean; reason?: string }>(`/api/vms/${vmId}/status`);
  }

  // Starts, shuts down or shows one VM because the user pressed its control.
  vmPower(vmId: string, action: 'start' | 'stop' | 'show') {
    return this.api<{ state: string }>(`/api/vms/${vmId}/power`, { action });
  }

  // Reads or saves one VM's idle limit in minutes; zero is off.
  vmIdle(vmId: string, minutes?: number) {
    return this.api<{ minutes: number; choices: number[] }>(`/api/vms/${vmId}/idle`,
      minutes === undefined ? undefined : { minutes });
  }

  // Deletes a stopped VM that no project owns. The disk cannot be recovered.
  deleteVm(vmId: string) {
    return this.api<Record<string, never>>(`/api/vms/${vmId}/delete`, {});
  }

  // File transfer. The Mac side is chosen in the macOS panel the backend opens; the page only
  // ever holds the selection's identifier and the chosen names.
  transferPick(vmId: string, kind: 'files' | 'folders' | 'destination') {
    return this.api<{ cancelled: true } | { selectionId: string;
      items: { name: string; kind: 'file' | 'folder'; size: number | null }[] }>(
      `/api/vms/${vmId}/transfers/pick`, { kind });
  }
  transferFiles(vmId: string, folder: string, path: string) {
    return this.api<{ entries: { name: string; kind: 'file' | 'folder'; size: number | null }[] }>(
      `/api/vms/${vmId}/transfers/files?${new URLSearchParams({ folder, path })}`);
  }
  transferStart(vmId: string, body: { direction: 'to_vm' | 'to_mac'; selectionId: string;
    sources?: { folder: string; path: string }[] }) {
    return this.api<{ jobId: string }>(`/api/vms/${vmId}/transfers`, body);
  }
  transferView<T>(jobId: string) { return this.api<T>(`/api/transfers/${jobId}`); }
  transferResolve(jobId: string, choice: string, all: boolean) {
    return this.api<Record<string, never>>(`/api/transfers/${jobId}/resolve`, { choice, all });
  }
  transferCancel(jobId: string) { return this.api<Record<string, never>>(`/api/transfers/${jobId}/cancel`, {}); }

  // The right-hand panel: files and changes of the open conversation's working folder.
  workspace<T>(action: string, body?: unknown) {
    if (!this.active) return Promise.reject(new Error('Start the conversation to see its files and changes.'));
    return this.api<T>(`/api/threads/${this.active.id}/workspace/${action}`, body);
  }

  // Scheduled runs: reads the list, or posts to '' (new), '<id>' (edit), '<id>/run' or '<id>/delete'.
  schedules<T>(action?: string, body?: unknown) {
    return this.api<T>(`/api/schedules${action ? `/${action}` : ''}`, body);
  }

  // What this conversation can use: whether its agent allows each capability, and whether it is
  // on. Before a conversation exists the list follows the chosen agent and project.
  capabilities<T>() {
    const query = this.active ? `thread=${this.active.id}`
      : [this.agentId ? `agentId=${this.agentId}` : '', this.projectId ? `projectId=${this.projectId}` : '',
          `claude=${this.claude}`, `vm=${this.machine === 'vm'}`].filter(Boolean).join('&');
    return this.api<T>(`/api/capabilities?${query}`);
  }

  // Reads or saves one agent's defaults for new conversations.
  capabilityDefaults<T>(id: string, choice?: { key: string; on: boolean }) {
    return this.api<T>(`/api/agents/${id}/capability-defaults`, choice);
  }

  // Clears choices when starting another draft or changing its agent, provider or machine.
  private resetDraftCapabilities() {
    this.draftCapabilities = {};
    this.codingSession = false;
    this.fullAccess = false; this.folder = null;
  }

  // Sends the user's answers to the open question, or their choice to reply in the chat instead.
  async answerQuestion(id: string, body: unknown) {
    const question = this.pendingQuestion, task = this.active;
    if (!task || !question || question.id !== id || this.deciding) return;
    const identity = this.identity, selection = this.selected;
    this.deciding = id;
    try {
      await this.api(`/api/threads/${question.thread ?? task.id}/questions/${id}`, body);
      if (identity !== this.identity) return;
      this.answeredRequest(id);
      if (selection === this.selected && this.question?.id === id) this.question = null;
    } catch (error) { if (identity === this.identity) this.fail(error); }
    finally { if (this.deciding === id) this.deciding = ''; }
  }

  // Stages a draft switch locally, or saves it immediately for an existing conversation.
  async toggleCapability(key: string, on: boolean, durationSeconds?: number) {
    if (!this.ready || this.working || this.pending || this.switching) return false;
    if (key === 'unsandboxed_shell' && !this.active) return false;
    const selection = this.selected;
    try {
      if (this.active) {
        const result = await this.api<Snapshot>(`/api/threads/${this.active.id}/capabilities`, { key, on, ...(durationSeconds === undefined ? {} : { durationSeconds }) });
        if (key === 'image_generation') {
          this.imageChanging = result.imageChanging ?? false;
          this.imageError = result.imageError ?? '';
          this.imageRetryOn = result.imageRetryOn ?? null;
        }
      }
      else this.draftCapabilities = { ...this.draftCapabilities, [key]: on };
      if (this.disposed || selection !== this.selected) return false;
      if (key === 'coding_session') this.codingSession = on;
      if (key === 'full_access') this.fullAccess = on;
      return true;
    } catch (error) { if (selection === this.selected) this.fail(error); return false; }
  }

  // Retries an interrupted image-tool reload with the switch value the user requested.
  async retryImage() {
    if (!this.active) return;
    try {
      const capabilities = await this.capabilities<{ capabilities: { key: string; on: boolean }[] }>();
      const current = capabilities.capabilities.find(item => item.key === 'image_generation');
      if (current) await this.toggleCapability('image_generation', this.imageRetryOn ?? !current.on);
    } catch (error) { this.fail(error); }
  }

  // The main agent's default model and effort: read, or saved when a body is given.
  mainAgent<T>(body?: unknown) { return this.api<T>('/api/main-agent', body); }

  // What the composer's slash menu offers: built-in skills and the user's callable ones.
  slashCommands<T>() { return this.api<T>('/api/slash-commands'); }

  // A skill or an agent a model wrote, waiting for its form to open.
  skillProposal = $state<import('./message-markdown').SkillProposal | null>(null);
  agentProposal = $state<import('./message-markdown').AgentProposal | null>(null);
  // A server an agent proposed and the user chose to add; the server form opens filled in with it.
  mcpProposal = $state<import('./message-markdown').ServerProposal | null>(null);

  // Reads what MiniClaw recorded about its own use over a range of days.
  analytics<T>(days: number) {
    return this.api<T>(`/api/analytics?days=${days}`);
  }

  // MCP servers: reads the list or `sources?provider=`, or posts to `servers`, `servers/<id>`, `import`.
  mcp<T>(action: string, body?: unknown) {
    return this.api<T>(`/api/mcp/${action}`, body);
  }

  // Experimental folder backups: reads the view, or posts to `settings`, `run` or `<id>/restore`.
  vmBackups<T>(vmId: string, action = '', body?: unknown) {
    return this.api<T>(`/api/vms/${vmId}/backups${action ? `/${action}` : ''}`, body);
  }

  // Lists VMs about to shut down for being idle, and answers one warning.
  idleWarnings() {
    return this.api<{ warnings: { vmId: string; name: string; secondsLeft: number }[] }>('/api/vms/idle');
  }
  keepRunning(vmId: string) {
    return this.api<Record<string, never>>(`/api/vms/${vmId}/idle/keep`, {});
  }

  async permissionSettings(kind: 'agent' | 'project' | 'conversation', id: string) {
    return this.api<PermissionSettings>(`/api/permissions/${kind}/${id}`);
  }

  // Saves one exact rule; the backend remains responsible for revocation and scope checks.
  async savePermission(kind: 'agent' | 'project' | 'conversation', id: string,
    target: PermissionTarget, effect: 'allow' | 'block' | 'remove') {
    return this.api<Record<string, never>>(`/api/permissions/${kind}/${id}`, { target, effect });
  }

  // Requests review of an app name without creating a permission grant.
  async requestApplication(kind: 'agent' | 'project' | 'conversation', id: string,
    name: string) {
    return this.api<Record<string, never>>(`/api/permissions/${kind}/${id}/app-requests`,
      { name });
  }

  // Uses live state for the open chat and the backend's list for other rows.
  isWorking(id: string) {
    return this.active?.id === id
      ? this.working || this.pending || !!this.turn
      : this.workingTasks.includes(id);
  }

  // Returns whether another conversation has a finished answer waiting to be read.
  hasReadyReply(id: string) { return this.active?.id !== id && this.readyReplies.includes(id); }

  // Checks background work without fetching transcripts or overlapping requests.
  private async refreshWork() {
    if (this.checkingWork || this.disposed || !this.ready) return;
    this.checkingWork = true;
    const identity = this.identity, requestsRevision = this.requestsRevision;
    this.approvalNow = Date.now();
    try {
      const result = await this.api<{ threads: string[]; activity?: Record<string, number>;
        delegationPrompts?: GlobalDelegationPrompt[]; delegatedRequests?: DelegatedRequest[];
        replies?: Record<string, { turnId: string; completedAt: number }> }>('/api/threads/working');
      if (!this.disposed && identity === this.identity) {
        this.workingTasks = result.threads;
        if (requestsRevision === this.requestsRevision) {
          this.delegatedRequests = (result.delegatedRequests ?? []).filter(item => !this.answeredRequests.has(item.request.id));
        }
        this.backgroundDelegations = (result.delegationPrompts ?? [])
          .filter(prompt => !this.answeredDelegations.has(prompt.id));
        for (const [id, reply] of Object.entries(result.replies ?? {})) {
          if (this.seenReplies.get(id) !== reply.turnId && reply.completedAt >= this.openedAt
            && this.active?.id !== id && !this.readyReplies.includes(id)) {
            this.readyReplies = [...this.readyReplies, id];
          }
          this.seenReplies.set(id, reply.turnId);
        }
        this.tasks = this.tasks.map(task => {
          const updatedAt = result.activity?.[task.id];
          return updatedAt && updatedAt > (task.updatedAt ?? 0) ? { ...task, updatedAt } : task;
        });
      }
    } catch {
      if (!this.disposed && identity === this.identity) this.workingTasks = [];
    } finally { this.checkingWork = false; }
  }

  // Starts visible-page polling and focus refreshes; returns cleanup for timers,
  // listeners, and the active stream when the page unmounts.
  mount() {
    void this.refresh().then(() => this.refreshWork());
    const workTimer = setInterval(() => {
      if (!document.hidden) void this.refreshWork();
    }, 2000);
    const timer = setInterval(() => {
      if (!document.hidden) void this.refresh();
    }, 15000);
    // Refresh account and chat status when the browser regains focus.
    const focus = () => { void this.refresh(); void this.refreshWork(); };
    window.addEventListener('focus', focus);
    return () => {
      this.disposed = true;
      this.skills.clear();
      this.groups.clear();
      this.newSkillSelection = { skillIds: [], skillGroupIds: [], revision: 0 };
      this.projects.clear();
      this.selected++;
      clearInterval(timer);
      clearInterval(workTimer);
      window.removeEventListener('focus', focus);
      this.feed?.close();
    };
  }

  // Refreshes account, tasks, catalog, usage, and pending login state; restores selection
  // when needed. Temporary failures mark the account unavailable without clearing drafts.
  async refresh(forceModels = false) {
    if (this.refreshing || this.disposed) return;
    this.refreshing = true;
    try {
      const account = await this.api<Account>('/api/account');
      if (this.disposed) return;
      if (!['signed_in', 'signed_out', 'unsupported_auth'].includes(account.status)) {
        throw new Error('Invalid account response');
      }
      const changed = this.knownAccount && this.accountEmail !== account.email;
      this.account = account;
      if (!this.ready || changed) this.clear();
      this.knownAccount = this.ready;
      this.accountEmail = account.email;
      if (this.ready) {
        const selection = this.selected;
        try {
          const computer = await this.api<{ available: boolean; host?: boolean; reason?: string | null }>('/api/computer');
          this.hostComputer = (computer.host ?? computer.available) === true;
          this.hostComputerReason = typeof computer.reason === 'string' ? computer.reason : '';
        } catch { this.hostComputer = false; this.hostComputerReason = ''; }
        await this.refreshModels(forceModels);
        await this.refreshAgents();
        await this.projects.refresh();
        const taskWrite = this.taskWrite;
        const tasks = await this.taskList();
        if (this.error === this.recoveryError) this.error = '';
        this.recoveryError = '';
        if (this.disposed) return;
        // A refresh started before creation cannot remove the newly selected task.
        if (selection === this.selected && taskWrite === this.taskWrite &&
          !this.working && !this.deleting) {
          this.tasks = tasks;
          if (this.active && !this.tasks.some(task => task.id === this.active?.id)) {
            this.clear();
          }
          if (!this.active && this.tasks.length && !this.composingNew) {
            const saved = this.savedTask();
            await this.select(this.tasks.find(task => task.id === saved) ?? this.tasks[0]);
          }
          if (this.active && this.feed?.readyState === EventSource.CLOSED) {
            this.follow(this.active, this.selected);
          }
        }
        // Quota reads can take seconds; they must not hold up focus/stream recovery.
        void this.refreshUsage();
      }
      if (this.login.status === 'pending') {
        const login = await this.api<Login>('/api/auth/status');
        if (this.disposed) return;
        this.login = { ...this.login, ...login };
        if (login.status === 'failed') this.error = login.error ?? 'Sign-in failed. Try again.';
        // Another signed-in provider makes the chat ready; a Claude sign-in waits for Claude.
        const done = this.login.provider === 'claude'
          ? this.providers.some(provider => provider.id === 'claude' && provider.status === 'signed_in') : this.ready;
        if (done) this.login = { status: 'succeeded' };
      }
    } catch (error) {
      if (!this.disposed) {
        this.account = { status: 'unavailable' };
        const message = error instanceof Error ? error.message : 'Chat state is unavailable.';
        // Clear this refresh failure on recovery without replacing unrelated errors.
        if (!this.error || this.error === this.recoveryError) this.error = message;
        this.recoveryError = message;
      }
    } finally { this.refreshing = false; }
  }

  /** Refreshes each connected provider independently; failures cannot take chat offline. */
  async refreshUsage(force = false) {
    if (this.usageLoading || this.disposed) return;
    this.usageLoading = true;
    const identity = this.identity;
    const providers = this.account.providers ?? [{ id: 'codex', enabled: true, installed: true,
      status: this.account.status, email: this.account.email }];
    try {
      await Promise.all(providers.map(async provider => {
        const key = this.usageKey(provider);
        if (this.usageAccounts[provider.id] !== key) {
          this.providerUsage[provider.id] = unavailableUsage();
          this.usageAccounts[provider.id] = key;
        }
        if (!provider.enabled || !provider.installed || provider.status !== 'signed_in') return;
        const query = provider.id === 'codex' ? (force ? '?refresh=true' : '')
          : `?provider=${provider.id}${force ? '&refresh=true' : ''}`;
        const current = () => {
          const account = this.account.providers?.find(row => row.id === provider.id);
          return !this.disposed && identity === this.identity && (!account || this.usageKey(account) === key);
        };
        try {
          const reading = await this.api<Usage>(`/api/usage${query}`);
          if (current()) this.providerUsage[provider.id] = reading;
        } catch {
          if (!current()) return;
          const last = this.providerUsage[provider.id];
          this.providerUsage[provider.id] = last?.fetchedAt ? { ...last, status: 'stale' } : unavailableUsage();
        }
      }));
    } finally { this.usageLoading = false; }
  }

  // Clears account-specific UI state, drafts, and saved selection, then closes the stream.
  private clear() {
    // Late replies from the previous account must not repopulate the cleared UI.
    this.identity++;
    this.skills.clear();
    this.groups.clear();
    this.projects.clear();
    this.projectId = '';
    this.catalogRead++;
    this.models = [];
    this.model = '';
    this.effort = '';
    this.speed = '';
    this.modelError = '';
    this.modelsLoading = false;
    this.agents = [];
    this.agentId = '';
    this.agentsLoading = false;
    this.agentSaving = false;
    this.agentError = '';
    this.hostComputer = false;
    this.hostComputerReason = '';
    this.vmComputerReady = false;
    this.computerUse = false;
    this.computerPending = false;
    this.computerError = '';
    this.imageChanging = false; this.imageError = ''; this.imageRetryOn = null;
    this.composingNew = false;
    this.approval = null;
    this.scopedApproval = null;
    this.computerActivity = [];
    this.deciding = '';
    this.choices.clear();
    this.selected++;
    this.feed?.close();
    this.active = null;
    this.messages = [];
    this.startedTurns.clear();
    this.activities = [];
    this.delegations = []; this.delegationPrompt = null;
    this.review = null; this.codingSession = false; this.fullAccess = false; this.folder = null;
    this.resetDraftCapabilities();
    this.queue = { version: 0, items: [], paused: false, reason: null }; this.stopRequested = false; this.question = null;
    this.older = [];
    this.historyCursor = null;
    this.historyLoading = false;
    this.historyPaged = false;
    this.savedTask(null);
    this.tasks = [];
    this.workingTasks = [];
    this.backgroundDelegations = []; this.answeredDelegations.clear();
    this.delegatedRequests = []; this.answeredRequests.clear();
    this.delegationDeciding = false;
    this.readyReplies = [];
    this.seenReplies.clear();
    this.openedAt = Date.now();
    this.turn = null;
    this.pending = false;
    this.uncertain = false;
    this.reconnecting = false;
    this.drafts.clear();
    this.draft = '';
    for (const item of [...this.files]) this.removeFile(item.key);
    this.providerUsage = {};
  }

  // Starts Codex OAuth, opening the browser flow or showing device details in login state.
  // Failed starts close the reserved popup and expose an error.
  async signIn(method: 'browser' | 'device' = 'browser') {
    if (this.login.status === 'pending') return;
    this.error = '';
    this.login = { status: 'pending' };
    // Reserve the tab in the click handler before awaiting the authorization URL.
    const popup = method === 'browser' ? window.open('about:blank', '_blank') : null;
    if (popup) popup.opener = null;
    try {
      const login = await this.api<Login>('/api/auth/start', { method });
      if (this.disposed) { popup?.close(); return; }
      const url = login.authUrl ?? login.verificationUrl;
      if (!url || new URL(url).protocol !== 'https:') throw new Error('Invalid sign-in URL');
      this.login = login;
      if (popup) popup.location.href = url;
    } catch (error) {
      popup?.close();
      this.login = { status: 'failed' };
      this.error = error instanceof Error ? error.message : 'Sign-in failed.';
    }
  }

  // Returns every provider's saved choice and sign-in state from the last account read.
  get providers() { return this.account.providers ?? []; }

  // Starts the Claude CLI's sign-in, which returns its sign-in page, then polls until it finishes.
  async signInClaude() {
    if (this.login.status === 'pending') return;
    this.error = '';
    try {
      this.login = await this.api<Login>('/api/auth/start', { method: 'browser', provider: 'claude' });
      if (this.login.status === 'failed') {
        this.error = this.login.error ?? 'Claude sign-in failed. Try again.';
        return;
      }
      for (let attempt = 0; attempt < 90 && this.login.status === 'pending' && !this.disposed; attempt += 1) {
        await new Promise(resolve => setTimeout(resolve, 2000));
        await this.refresh();
      }
    } catch (error) {
      this.login = { status: 'failed' };
      this.error = error instanceof Error ? error.message : 'Sign-in failed.';
    }
  }

  // Sends the code Claude's page showed to the waiting sign-in; returns false and shows why when refused.
  async sendClaudeCode(code: string) {
    try {
      await this.api('/api/auth/code', { code: code.trim() });
      this.login = { ...this.login, notice: undefined };
      await this.refresh();
      return true;
    } catch (error) {
      this.login = { ...this.login, notice: error instanceof Error ? error.message : 'The code was not accepted.' };
      return false;
    }
  }

  // Saves one provider's enabled choice; the backend starts or stops it immediately.
  async setProvider(id: Provider['id'], enabled: boolean) {
    try {
      await this.api(`/api/providers/${id}`, { enabled });
      await this.refresh();
    } catch (error) { this.fail(error); }
  }

  // Cancels the current OAuth attempt and updates login state, or displays its error.
  async cancelLogin() {
    try {
      await this.api('/api/auth/cancel', {});
      this.login = { status: 'cancelled' };
    } catch (error) { this.fail(error); }
  }

  // Opens an empty composer; Codex gets a task only when its first prompt is sent.
  // Empty Codex tasks cannot be archived, so choose their initial tools at first Send.
  async newChat(projectId = '') {
    if (this.working || this.switching || !this.ready) return;
    if (this.active) {
      this.drafts.set(this.active.id, this.draft);
      this.choices.set(this.active.id, { model: this.model, effort: this.effort, speed: this.speed });
    }
    this.selected++;
    this.feed?.close();
    this.active = null;
    this.agentId = '';
    this.projectId = projectId;
    this.newMachine = this.projects.items.find(p => p.id === projectId)?.environment?.kind ?? 'host';
    this.newSkillSelection = { skillIds: [], skillGroupIds: [], revision: 0 };
    this.resetDraftCapabilities();
    if (projectId) this.projects.expand(projectId, true);
    this.composingNew = true;
    this.savedTask(null);
    this.messages = [];
    this.startedTurns.clear();
    this.activities = [];
    this.delegations = []; this.delegationPrompt = null;
    this.review = null; this.codingSession = false; this.fullAccess = false; this.folder = null;
    this.queue = { version: 0, items: [], paused: false, reason: null }; this.stopRequested = false; this.question = null;
    this.older = [];
    this.historyCursor = null;
    this.historyLoading = false;
    this.historyPaged = false;
    this.turn = null;
    this.pending = false;
    this.uncertain = false;
    this.reconnecting = false;
    this.computerUse = false;
    this.computerPending = false;
    this.computerError = '';
    this.imageChanging = false; this.imageError = ''; this.imageRetryOn = null;
    this.computerActivity = [];
    this.approval = null;
    this.scopedApproval = null;
    this.draft = '';
    for (const item of [...this.files]) this.removeFile(item.key);
    this.error = '';
    this.vmApproval = false;
    this.vmApproved = false;
    this.vmApprovalId = '';
  }

  // Requests a chat, adds its returned summary to the list, and opens its snapshot/feed.
  private async create(computerUse = false) {
    const identity = this.identity;
    const { thread } = await this.api<{ thread: Task }>('/api/threads', {
      ...this.settings(), ...(computerUse ? { computerUse: true } : {}),
      environment: { kind: this.newMachine },
      ...(this.agentId ? { agentId: this.agentId } : {}),
      ...(this.projectId ? { projectId: this.projectId } : {}),
      ...(Object.keys(this.draftCapabilities).length ? { capabilities: { ...this.draftCapabilities } } : {}),
      ...(this.newSkillSelection.skillIds.length || this.newSkillSelection.skillGroupIds.length
        ? { skillSelection: $state.snapshot(this.newSkillSelection) } : {}),
    });
    if (this.disposed || identity !== this.identity) return;
    this.tasks = [thread, ...this.tasks.filter(task => task.id !== thread.id)];
    await this.select(thread);
  }

  // Saves the previous chat's draft/settings, then loads and subscribes to this task.
  // Ignores stale replies and remembers the selection after its snapshot loads.
  async select(task: Task) {
    if (this.switching) return;
    this.resetDraftCapabilities();
    this.composingNew = false;
    if (this.active) {
      this.drafts.set(this.active.id, this.draft);
      this.choices.set(this.active.id, { model: this.model, effort: this.effort, speed: this.speed });
    }
    this.feed?.close();
    // Ignore the previous task's pending snapshot and queued stream events.
    const selection = ++this.selected;
    this.active = task;
    this.agentId = task.agentId ?? '';
    this.projectId = task.projectId ?? '';
    this.computerUse = false;
    this.computerPending = false;
    this.computerError = '';
    this.imageChanging = false; this.imageError = ''; this.imageRetryOn = null;
    this.messages = [];
    this.startedTurns.clear();
    this.activities = [];
    this.delegations = []; this.delegationPrompt = null;
    this.review = null; this.codingSession = false; this.fullAccess = false; this.folder = null;
    this.queue = { version: 0, items: [], paused: false, reason: null }; this.stopRequested = false; this.question = null;
    this.older = [];
    this.historyCursor = null;
    this.historyLoading = false;
    this.historyPaged = false;
    this.approval = null;
    this.scopedApproval = null;
    this.computerActivity = [];
    this.deciding = '';
    this.turn = null;
    this.pending = true;
    this.error = '';
    this.vmApproval = false;
    this.vmApproved = false;
    this.vmApprovalId = '';
    this.uncertain = false;
    this.draft = this.drafts.get(task.id) ?? '';
    try {
      // Load the current chat state, then follow its live events.
      const snapshot = await this.api<Snapshot>(`/api/threads/${task.id}`);
      if (this.disposed || selection !== this.selected) return;
      this.restoreChoice(this.choices.get(task.id) ?? snapshot.thread);
      this.apply(snapshot);
      this.readyReplies = this.readyReplies.filter(id => id !== task.id);
      this.savedTask(task.id);
      if (!snapshot.vmOffline) this.follow(task, selection);
    } catch (error) {
      if (selection === this.selected) { this.pending = false; this.fail(error); }
    }
  }

  // Opens a fresh snapshot stream without clearing history, drafts or model choices.
  private follow(task: Task, selection: number) {
    this.feed?.close();
    const feed = new EventSource(`/api/threads/${task.id}/events`);
    this.feed = feed;
    let opened = false;
    // Ignores queued events from replaced streams and previous chat selections.
    const on = (name: string, handle: (data: any) => void) => {
      feed.addEventListener(name, event => {
        if (this.disposed || selection !== this.selected || feed !== this.feed) return;
        try { handle(JSON.parse((event as MessageEvent).data)); }
        catch { this.error = 'Could not read the response stream.'; }
      });
    };
    on('snapshot', data => { this.apply(data); this.reconnecting = false; });
    on('computer.updated', data => this.applyComputer(data));
    on('resync.required', () => { this.reconnecting = true; });
    on('turn.started', data => { this.turn = data.turnId; this.nameTurn(data.messageId, data.turnId); });
    on('message.delta', data => this.append(data.itemId, data.delta, false, data.turnId));
    on('message.completed', data => this.append(data.itemId, data.text, true, data.turnId));
    on('activity', data => this.recordActivity(data));
    on('queue', data => this.takeQueue(data));
    on('delegation', data => {
      const index = this.delegations.findIndex(run => run.id === data.id);
      // A run update carries no live line; a running run keeps the one it had.
      const live = data.status === 'running' ? data.live ?? this.delegations[index]?.live : undefined;
      if (index < 0) this.delegations.push({ ...data, live }); else this.delegations[index] = { ...data, live };
    });
    on('delegation.live', data => {
      const run = this.delegations.find(run => run.id === data.id);
      if (run) run.live = data.line;
    });
    on('delegation.prompt', data => { this.delegationPrompt = data; });
    on('delegation.requests', data => {
      this.requestsRevision++;
      this.delegatedRequests = [...this.delegatedRequests.filter(item => item.runId !== data.runId),
        ...data.requests.filter((item: DelegatedRequest) => !this.answeredRequests.has(item.request.id))];
    });
    on('turn.completed', data => {
      this.approval = null;
      this.scopedApproval = null;
      this.finished.add(data.turnId);
      this.turn = null;
      this.question = null;
      this.stopRequested = false;
      this.pending = false;
      this.uncertain = false;
      if (data.error) this.error = data.error;
      void this.refresh();
    });
    on('turn.error', data => {
      this.error = data.message;
      this.uncertain = data.outcomeUnknown;
    });
    feed.addEventListener('error', () => {
      if (!this.disposed && selection === this.selected && feed === this.feed) {
        this.reconnecting = true;
        void this.refresh();
      }
    });
    feed.addEventListener('open', () => {
      if (this.disposed || selection !== this.selected || feed !== this.feed) return;
      // Automatic retries may replay nothing; a fresh feed must supply a snapshot.
      if (opened && this.reconnecting) this.follow(task, selection);
      opened = true;
    });
  }

  // Applies the latest chat snapshot while retaining previously loaded older messages.
  private apply(snapshot: Snapshot) {
    this.messages = this.merge(this.older, snapshot.messages);
    this.nameTurn(null, '');
    if (Array.isArray(snapshot.activities)) this.activities = snapshot.activities;
    if (Array.isArray(snapshot.delegations)) this.delegations = snapshot.delegations;
    this.delegationPrompt = snapshot.delegationPrompt ?? null;
    this.review = snapshot.review ?? null;
    if (typeof snapshot.codingSession === 'boolean') this.codingSession = snapshot.codingSession;
    if (typeof snapshot.fullAccess === 'boolean') this.fullAccess = snapshot.fullAccess;
    if (snapshot.folder !== undefined) this.folder = snapshot.folder ?? null;
    // The event stream's own snapshots carry no queue; only a full read replaces it.
    if (snapshot.queue) this.takeQueue(snapshot.queue);
    if (!this.historyPaged) this.historyCursor = snapshot.historyCursor ?? null;
    this.turn = snapshot.activeTurn?.id ?? null;
    this.pending = snapshot.pending;
    this.uncertain = snapshot.outcomeUnknown ?? false;
    this.active = snapshot.thread;
    this.agentId = snapshot.thread.agentId ?? '';
    this.projectId = snapshot.thread.projectId ?? '';
    this.tasks = this.tasks.map(t => t.id === snapshot.thread.id ? snapshot.thread : t);
    this.applyComputer(snapshot);
    if (snapshot.delegatedRequests) {
      this.requestsRevision++;
      this.delegatedRequests = [...this.delegatedRequests.filter(item => item.parent !== snapshot.thread.id),
        ...snapshot.delegatedRequests.filter(item => !this.answeredRequests.has(item.request.id))];
    }
    this.approval = snapshot.approval ?? null;
    this.scopedApproval = snapshot.scopedApproval ?? null;
    this.question = snapshot.question ?? null;
    this.computerActivity = snapshot.computerActivity ?? [];
    this.imageChanging = snapshot.imageChanging ?? false;
    this.imageError = snapshot.imageError ?? '';
    this.imageRetryOn = snapshot.imageRetryOn ?? null;
  }

  // Returns messages in first-seen order, using the newer copy when an ID appears twice.
  private merge(older: Message[], recent: Message[]) {
    return [...new Map([...older, ...recent].map(message => [message.id, message])).values()];
  }

  // Prepends older history and advances its cursor; ignores replies after a task switch.
  async loadOlder() {
    const task = this.active;
    const cursor = this.historyCursor;
    if (!task || !cursor || !this.ready || this.historyLoading) return;
    const selection = this.selected;
    this.historyLoading = true;
    try {
      const page = await this.api<{ messages: Message[]; nextCursor: string | null }>(
        `/api/threads/${task.id}/history?cursor=${encodeURIComponent(cursor)}`,
      );
      if (this.disposed || selection !== this.selected) return;
      if (page.nextCursor === cursor) throw new Error('History cursor did not advance.');
      this.older = this.merge(page.messages, this.older);
      this.messages = this.merge(this.older, this.messages);
      this.historyCursor = page.nextCursor;
      this.historyPaged = true;
    } catch (error) { if (selection === this.selected) this.fail(error); }
    finally { if (selection === this.selected) this.historyLoading = false; }
  }

  // Submits the pending app-approval decision and clears that prompt after acknowledgement.
  async decide(action: 'accept' | 'decline') {
    const approval = this.pendingApproval;
    const task = this.active;
    if (!approval || !task || this.deciding) return;
    const selection = this.selected, identity = this.identity;
    this.deciding = approval.id;
    try {
      await this.api(`/api/threads/${approval.thread ?? task.id}/approvals/${approval.id}`, { action });
      if (identity === this.identity) this.answeredRequest(approval.id);
      if (selection === this.selected && this.approval?.id === approval.id) {
        this.approval = null;
      }
    } catch (error) { if (selection === this.selected) this.fail(error); }
    finally { if (this.deciding === approval.id) this.deciding = ''; }
  }

  // Resolves one exact scoped request without changing lasting settings.
  async decideScoped(action: 'approve_once' | 'approve_session' | 'approve_session_domains' | 'approve_programs' | 'reject') {
    const approval = this.pendingScopedApproval;
    const task = this.active;
    if (!approval || !task || this.deciding) return;
    const selection = this.selected, identity = this.identity;
    this.deciding = approval.id;
    try {
      // A delegated agent's request is answered on the run's own thread.
      await this.api(`/api/threads/${approval.thread ?? task.id}/scoped-approvals/${approval.id}`, { action });
      if (identity === this.identity) this.answeredRequest(approval.id);
      if (selection === this.selected && this.scopedApproval?.id === approval.id) {
        this.scopedApproval = null;
      }
    } catch (error) { if (selection === this.selected) this.fail(error); }
    finally { if (this.deciding === approval.id) this.deciding = ''; }
  }

  // Updates an assistant item, appending deltas or replacing them with final text.
  // Adds one tool call to the list or updates it in place; entries arrive as `running`, then finished.
  private recordActivity(entry: ToolActivity) {
    const index = this.activities.findIndex(item => item.id === entry.id);
    if (index < 0) this.activities.push(entry); else this.activities[index] = entry;
  }

  // Decides the change under review. `text` is the user's own version; `note` goes to the agent.
  async decideReview(action: 'apply' | 'apply_all' | 'reject', text?: string, note?: string) {
    const review = this.pendingReview, task = this.active;
    if (!review || !task || this.deciding) return;
    const identity = this.identity, selection = this.selected;
    this.deciding = review.id;
    try {
      await this.api(`/api/threads/${review.thread ?? task.id}/reviews/${review.id}`,
        { action, ...(text !== undefined ? { text } : {}), ...(note?.trim() ? { note: note.trim() } : {}) });
      if (identity !== this.identity) return;
      this.answeredRequest(review.id);
      if (selection === this.selected && this.review?.id === review.id) this.review = null;
    } catch (error) { this.fail(error); }
    finally { if (this.deciding === review.id) this.deciding = ''; }
  }

  // Switches the coding session for this conversation: every change is then reviewed first.
  async setCodingSession(on: boolean) {
    const task = this.active;
    if (!task) return;
    try {
      await this.api(`/api/permissions/conversation/${task.id}`,
        { target: { target_type: 'feature', name: 'coding_session' }, effect: on ? 'allow' : 'block' });
      this.codingSession = on;
    } catch (error) { this.fail(error); }
  }

  // Answers the open delegation request: wait for the result, let it run, or decline.
  async answerDelegation(mode: 'wait' | 'background' | 'reject') {
    const prompt = this.pendingDelegation;
    if (!prompt || this.delegationDeciding) return;
    const identity = this.identity;
    this.delegationDeciding = true;
    try {
      await this.api(`/api/threads/${prompt.thread}/delegations/${prompt.id}`, { mode });
      if (this.disposed || identity !== this.identity) return;
      this.answeredDelegations.add(prompt.id);
      if (this.answeredDelegations.size > 256) this.answeredDelegations.delete(this.answeredDelegations.values().next().value!);
      this.backgroundDelegations = this.backgroundDelegations.filter(item => item.id !== prompt.id);
      if (this.delegationPrompt?.id === prompt.id) this.delegationPrompt = null;
    } catch (error) { if (identity === this.identity) this.fail(error); }
    finally { this.delegationDeciding = false; void this.refreshWork(); }
  }

  // Reads a run's own conversation for its expandable block.
  runTranscript(thread: string) {
    return this.api<Snapshot>(`/api/threads/${thread}`);
  }

  // Makes a run visible in the sidebar and opens the same conversation, even while it works.
  async continueWithRun(run: DelegatedRun) {
    const selection = this.selected;
    const { thread } = await this.api<{ thread: string }>(`/api/delegations/${run.id}/detach`, {});
    if (this.disposed || selection !== this.selected) return;
    const snapshot = await this.runTranscript(thread);
    if (this.disposed || selection !== this.selected) return;
    if (!this.tasks.some(task => task.id === thread)) this.tasks = [snapshot.thread, ...this.tasks];
    await this.select(snapshot.thread);
  }

  // Runs by the turn that asked for them, placed like that turn's activity.
  get runPlacement() {
    const byTurn = new Map<string, DelegatedRun[]>();
    for (const run of this.delegations) byTurn.set(run.turnId, [...(byTurn.get(run.turnId) ?? []), run]);
    // A background run goes on after its turn ended, so it stays in view while it works.
    return this.placeTurns(byTurn, runs => runs.some(run => run.status === 'running' || run.status === 'asking'));
  }

  // Groups tool calls by turn for the conversation view, placed by `placeTurns`.
  get activityPlacement() {
    const byTurn = new Map<string, ToolActivity[]>();
    // A delegated agent's calls are shown inside its run, not in the conversation's own list.
    for (const entry of this.activities.filter(entry => !entry.runId)) {
      byTurn.set(entry.turnId, [...(byTurn.get(entry.turnId) ?? []), entry]);
    }
    return this.placeTurns(byTurn);
  }

  // Places each turn's group where that turn is in the conversation, never by when it last changed:
  // `before` its first reply, or `after` its last message when it has no reply (Stop, a failure).
  // A turn with no loaded message is `pending` at the end when it is running, its group is still
  // `live`, or nothing older exists; otherwise it belongs to older history and appears once that
  // page is loaded.
  private placeTurns<T>(byTurn: Map<string, T[]>, live: (group: T[]) => boolean = () => false) {
    const reply = new Map<string, string>(), last = new Map<string, string>();
    for (const message of this.messages) {
      if (!message.turnId) continue;
      if (message.role === 'assistant' && !reply.has(message.turnId)) reply.set(message.turnId, message.id);
      last.set(message.turnId, message.id);
    }
    const before = new Map<string, T[]>(), after = new Map<string, T[]>(), pending: T[][] = [];
    for (const [turn, group] of byTurn) {
      if (reply.has(turn)) before.set(reply.get(turn)!, group);
      else if (last.has(turn)) after.set(last.get(turn)!, group);
      else if (turn === this.turn || !this.historyCursor || live(group)) pending.push(group);
    }
    return { before, after, pending };
  }

  // Writes the turn onto the user's message that started it, so a turn without a reply keeps its place.
  // The name is kept for this conversation, so a snapshot written before it was known cannot drop it.
  private nameTurn(messageId: unknown, turnId: string) {
    if (typeof messageId === 'string') this.startedTurns.set(messageId, turnId);
    for (const message of this.messages) {
      const turn = this.startedTurns.get(message.id);
      if (turn && message.role === 'user' && !message.turnId) message.turnId = turn;
    }
  }

  private append(id: string, text: string, complete = false, turnId: string | null = this.turn) {
    const item = this.messages.find(message => message.id === id);
    if (item) {
      // Final text replaces the partial message; late deltas cannot change it.
      if (complete) { item.text = text; item.complete = true; }
      else if (!item.complete) item.text += text;
    } else {
      this.messages.push({ id, role: 'assistant', text, complete, turnId });
    }
  }

  // Validates and submits the draft, creating a chat if needed; clears it only on acceptance.
  // Failures retain the text and block another send when the outcome is unknown.
  async send() {
    if (this.busy || !this.ready || !this.agentReady || (!this.draft.trim() && !this.files.length)
      || !this.validChoice || !this.machineReady) return;
    if (this.usage.ordinaryUsageAllowed === false) return;
    const text = this.draft;
    if (new TextEncoder().encode(text).length > 32768) {
      this.error = 'Messages must be at most 32 KiB.';
      return;
    }
    const approved = this.vmApproved;
    this.vmApproved = false;
    if (this.machine === 'vm' && !approved) {
      try {
        const status = await this.refreshVmStatus();
        if (status.state === 'off') {
          this.declineVmStart();
          const messageId = crypto.randomUUID();
          const vmId = this.vmTarget;
          const prepared = await this.api<{ approvalId: string }>(
            `/api/vms/${vmId}/prepare`, this.active
              ? { threadId: this.active.id, clientMessageId: messageId }
              : { environment: { kind: 'vm' },
                  ...(this.projectId ? { projectId: this.projectId } : {}),
                  clientMessageId: messageId });
          this.vmApprovalId = prepared.approvalId;
          this.vmApprovalVmId = vmId;
          this.vmWaitingMessageId = messageId;
          this.vmWaitingText = text;
          this.vmApproval = true;
          return;
        }
        if (status.state !== 'running' || !status.ready) {
          this.error = status.reason ?? `${this.vmName} is not ready. The message was not sent.`;
          return;
        }
      } catch (error) { this.fail(error); return; }
    }
    this.error = '';
    this.working = true;
    // Creating the first task can change selection; keep the submitted settings.
    const settings = this.settings();
    const choice = { model: this.model, effort: this.effort, speed: this.speed };
    const computerUse = this.computerUse;
    const identity = this.identity;
    try {
      if (!this.active) {
        await this.create(computerUse);
        if (identity !== this.identity || this.disposed) return;
        this.draft = text;
        this.restoreChoice(choice);
      }
      const task = this.active;
      if (!task || this.disposed) return;
      const selection = this.selected;
      const files = [...this.files];
      const attachments = await this.uploadFiles(task.id, files);
      const clientMessageId = this.vmSendMessageId || crypto.randomUUID();
      const reply = await this.api<{ turnId: string }>(
        `/api/threads/${task.id}/messages`, {
          text, clientMessageId, ...settings,
          ...(attachments.length ? { attachments } : {}),
        },
      );
      if (this.disposed || selection !== this.selected) return;
      this.draft = '';
      this.clearFiles(files);
      if (settings.model && this.active) {
        this.active = { ...this.active, model: settings.model,
          effort: settings.effort ?? null, speed: settings.speed ?? null };
      }
      if (!this.finished.has(reply.turnId)) this.turn = reply.turnId;
      this.nameTurn(clientMessageId, reply.turnId);
    } catch (error) {
      if (this.disposed || identity !== this.identity) return;
      this.draft = text;
      this.uncertain = error instanceof ApiError && error.unknown;
      this.fail(error);
    } finally { this.working = false; }
  }

  // Requests interruption; keeps the turn active until the stream confirms completion.
  // Keeps the newest view of the queue. An HTTP answer can arrive after a later event.
  private takeQueue(view: MessageQueue) {
    if (view.version >= this.queue.version) this.queue = view;
  }

  // A message can be queued while a known response runs and nothing about it is in doubt.
  get queueable() {
    return this.ready && !!this.active && !!this.turn && !this.stopRequested && !this.working && !this.pending
      && !this.uncertain && !this.reconnecting && !this.vmStarting;
  }

  // Queues the draft behind the running response, with the model and effort chosen now.
  async enqueue() {
    const task = this.active;
    const text = this.draft;
    if (!task || !this.queueable || (!text.trim() && !this.files.length) || !this.validChoice) return;
    if (new TextEncoder().encode(text).length > 32768) { this.error = 'Messages must be at most 32 KiB.'; return; }
    this.error = '';
    this.draft = '';
    const files = [...this.files];
    try {
      const attachments = await this.uploadFiles(task.id, files);
      await this.api(`/api/threads/${task.id}/queue`, { text, clientMessageId: crypto.randomUUID(), ...this.settings(),
        ...(attachments.length ? { attachments } : {}) });
      this.clearFiles(files);
    } catch (error) {
      // The words come back, so nothing the user typed is lost.
      if (this.active?.id === task.id && !this.draft) this.draft = text;
      this.fail(error);
    }
  }

  // Changes the queue: `<id>` with new text, `<id>/delete`, `<id>/send-now`, `clear`, `pause` or `resume`.
  async queueAction(action: string, body: unknown = {}) {
    if (!this.active) return;
    try {
      const result = await this.api<MessageQueue | { item?: QueuedMessage }>(`/api/threads/${this.active.id}/queue/${action}`, body);
      if (result && 'items' in result) this.takeQueue(result);
    } catch (error) { this.fail(error); }
  }

  async stop() {
    if (!this.active || !this.turn) return;
    this.stopRequested = true;
    // Keep the turn active until the stream confirms it has ended.
    try {
      await this.api(`/api/threads/${this.active.id}/interrupt`, { turnId: this.turn });
    } catch (error) { this.fail(error); }
  }

  // Sets the error banner from a caught error, with a fallback for non-Error values.
  private fail(error: unknown) {
    this.error = error instanceof Error ? error.message : 'Request failed.';
  }
}
