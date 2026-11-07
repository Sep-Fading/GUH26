export type ProjectEnvironment = { kind: 'host' } |
  { kind: 'vm'; dedicated: boolean; vmId?: string };
export type Project = { id: string; name: string; agentIds: string[]; skillIds: string[];
  skillGroupIds?: string[]; environment?: ProjectEnvironment; workspace?: string;
  // The user's standing instructions for every conversation of the project.
  instructions?: string; revision: number };
// One note of a project's shared memory.
export type MemoryNote = { name: string; description: string; text: string; updatedAt: number };
export type ProjectDraft = Omit<Project, 'id' | 'revision'> & { revision?: number };
export type VmSetup = { vmId?: string; state: 'off' | 'starting' | 'running' |
  'failed' | 'missing' | 'unknown'; stage?: 'preparing' | 'installing' |
    'awaiting_setup' | 'failed'; percent?: number; ready?: boolean;
  workerConnected?: boolean; reason?: string };
type Api = <T>(path: string, body?: unknown) => Promise<T>;

export class Projects {
  items = $state<Project[]>([]);
  loading = $state(false);
  saving = $state(false);
  error = $state('');
  collapsed = $state<Record<string, boolean>>({});
  groupOrder = $state<Record<string, string[]>>({});
  private epoch = 0;
  private writes = 0;

  // Shares guarded requests and restores only non-sensitive expansion preferences.
  constructor(private api: Api, private ready: () => boolean) {
    try {
      const ids = JSON.parse(localStorage.getItem('miniclaw.collapsedProjects') ?? '[]');
      if (Array.isArray(ids)) this.collapsed = Object.fromEntries(
        ids.slice(0, 64).filter(id => typeof id === 'string').map(id => [id, true]));
    } catch { /* Storage is optional; projects default to expanded. */ }
    try {
      const saved = JSON.parse(localStorage.getItem('miniclaw.agentGroupOrder') ?? '{}');
      if (saved && typeof saved === 'object' && !Array.isArray(saved)) {
        this.groupOrder = Object.fromEntries(Object.entries(saved).filter(([, ids]) =>
          Array.isArray(ids) && ids.every(id => typeof id === 'string'))) as Record<string, string[]>;
      }
    } catch { /* Groups default to alphabetical order. */ }
  }

  // Saves a project's visible group order without storing agent names or chat content.
  orderGroups(project: string, ids: string[]) {
    this.groupOrder[project] = ids;
    try { localStorage.setItem('miniclaw.agentGroupOrder', JSON.stringify(this.groupOrder)); }
    catch { /* The current page still retains the chosen order. */ }
  }

  // Invalidates pending replies and clears account-bound project settings.
  clear() {
    this.epoch++;
    this.items = []; this.error = ''; this.loading = false; this.saving = false;
  }

  // Remembers collapsed project or project:agent keys without storing names or instructions.
  expand(id: string, open: boolean) {
    this.collapsed[id] = !open;
    try { localStorage.setItem('miniclaw.collapsedProjects',
      JSON.stringify(Object.keys(this.collapsed).filter(id => this.collapsed[id]).slice(-64))); }
    catch { /* The current page still retains its expansion state. */ }
  }

  // Loads project settings without overwriting a newer acknowledged edit.
  async refresh() {
    if (!this.ready() || this.loading || this.saving) return;
    const epoch = this.epoch, writes = this.writes;
    this.loading = true;
    try {
      const { projects } = await this.api<{ projects: Project[] }>('/api/projects');
      if (epoch !== this.epoch || writes !== this.writes) return;
      if (!Array.isArray(projects)) throw new Error('Projects unavailable');
      this.items = projects; this.error = '';
    } catch (error) {
      if (epoch === this.epoch && writes === this.writes) {
        this.error = error instanceof Error ? error.message : 'Projects unavailable';
      }
    } finally { if (epoch === this.epoch) this.loading = false; }
  }

  // Saves one project revision and retains the draft on failure.
  async save(id: string | undefined, draft: ProjectDraft) {
    if (!this.ready() || this.saving) return;
    const epoch = this.epoch;
    this.saving = true; this.error = '';
    try {
      const { project } = await this.api<{ project: Project }>(
        id ? `/api/projects/${id}` : '/api/projects', draft);
      if (epoch !== this.epoch) return;
      this.writes++;
      this.items = [...this.items.filter(p => p.id !== project.id), project];
      return project;
    } catch (error) {
      if (epoch === this.epoch) this.error = error instanceof Error ? error.message : 'Could not save project';
    } finally { if (epoch === this.epoch) this.saving = false; }
  }

  // Deletes the observed project. With `deleteVm` the backend also deletes its stopped VM
  // for good, and only when the typed name matches. Errors reach the confirmation dialog.
  // Reads, saves and deletes the notes of a project's shared memory. `replaces` names the note being changed.
  async memory(id: string) { return (await this.api<{ notes: MemoryNote[] }>(`/api/projects/${id}/memory`)).notes; }
  saveMemory(id: string, note: { name: string; description: string; text: string }, replaces?: string) {
    return this.api(`/api/projects/${id}/memory${replaces ? `/${encodeURIComponent(replaces)}` : ''}`, note);
  }
  removeMemory(id: string, name: string) { return this.api(`/api/projects/${id}/memory/${encodeURIComponent(name)}/delete`, {}); }

  // Names the instruction files found in a project's folder, for its settings.
  rules(id: string) {
    return this.api<{ cut: boolean; files: { name: string; bytes: number }[] }>(`/api/projects/${id}/rules`);
  }

  async remove(project: Project, deleteVm = false, confirmName = '') {
    await this.api<Record<string, never>>(`/api/projects/${project.id}/delete`,
      deleteVm ? { revision: project.revision, deleteVm, confirmName } : { revision: project.revision });
    this.writes++;
    this.items = this.items.filter(item => item.id !== project.id);
  }

  // Starts one idempotent fresh install without saving a project first.
  async provisionVm(requestId: string) {
    return this.api<VmSetup & { vmId: string }>('/api/vms/projects', { requestId });
  }

  // Reads setup stages without requesting VM startup.
  async vmStatus(vmId: string) {
    return this.api<VmSetup>(`/api/vms/${vmId}/status`);
  }

  // Retries a retained failed install under the same VM identity.
  async retryVm(vmId: string) {
    return this.api<VmSetup>(`/api/vms/${vmId}/retry`, {});
  }

  // Cancels setup without deleting any partial guest files.
  async cancelVm(vmId: string) {
    return this.api<VmSetup>(`/api/vms/${vmId}/cancel`, {});
  }

  // Requests a Codex-managed device code from this VM's worker only.
  async startVmAuth(vmId: string) {
    return this.api<{ status: string; verificationUrl: string; userCode: string }>(
      `/api/vms/${vmId}/auth/start`, { method: 'device' });
  }
}
