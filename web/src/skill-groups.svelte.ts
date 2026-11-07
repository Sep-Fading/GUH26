export type Member = { skillId: string; enabled: boolean };
export type SkillGroup = { id: string; name: string; active: boolean; members: Member[]; revision: number };
export type GroupDraft = Omit<SkillGroup, 'id' | 'revision'> & { revision?: number };
export type SkillSelection = { skillIds: string[]; skillGroupIds: string[]; revision: number };
type Api = <T>(path: string, body?: unknown) => Promise<T>;

export class SkillGroups {
  items = $state<SkillGroup[]>([]);
  loading = $state(false);
  saving = $state(false);
  error = $state('');
  private epoch = 0;
  private writes = 0;

  // Shares authenticated local requests and account readiness with chat.
  constructor(private api: Api, private ready: () => boolean) {}

  // Clears account-bound groups and invalidates pending responses.
  clear() {
    this.epoch++; this.items = []; this.error = ''; this.loading = false; this.saving = false;
  }

  // Loads groups without allowing stale reads to overwrite completed edits.
  async refresh() {
    if (!this.ready() || this.loading || this.saving) return;
    const epoch = this.epoch, writes = this.writes;
    this.loading = true;
    try {
      const { groups } = await this.api<{ groups: SkillGroup[] }>('/api/skill-groups');
      if (epoch !== this.epoch || writes !== this.writes) return;
      if (!Array.isArray(groups)) throw new Error('Skill groups unavailable');
      this.items = groups; this.error = '';
    } catch (error) {
      if (epoch === this.epoch && writes === this.writes) this.error = String(error instanceof Error ? error.message : error);
    } finally { if (epoch === this.epoch) this.loading = false; }
  }

  // Saves one group revision and retains the editor draft on failure.
  async save(id: string | undefined, draft: GroupDraft) {
    if (!this.ready() || this.saving) return;
    const epoch = this.epoch;
    this.saving = true; this.error = '';
    try {
      const { group } = await this.api<{ group: SkillGroup }>(id ? `/api/skill-groups/${id}` : '/api/skill-groups', draft);
      if (epoch !== this.epoch) return;
      this.writes++; this.items = [...this.items.filter(g => g.id !== group.id), group];
      return group;
    } catch (error) {
      if (epoch === this.epoch) this.error = error instanceof Error ? error.message : 'Could not save group';
    } finally { if (epoch === this.epoch) this.saving = false; }
  }

  // Deletes the observed group without deleting its skills or other selections.
  async remove(id: string, revision: number) {
    if (!this.ready() || this.saving) throw new Error('Skill groups are busy.');
    const epoch = this.epoch;
    this.saving = true;
    try {
      await this.api(`/api/skill-groups/${id}/delete`, { revision });
      if (epoch !== this.epoch) throw new Error('Account changed.');
      this.writes++; this.items = this.items.filter(g => g.id !== id);
    } finally { if (epoch === this.epoch) this.saving = false; }
  }

  // Reads or saves a conversation selection, rejecting replies from an earlier account.
  async selection(id: string, body?: SkillSelection) {
    if (!this.ready()) throw new Error('Account unavailable.');
    const epoch = this.epoch;
    const { selection } = await this.api<{ selection: SkillSelection }>(`/api/threads/${id}/skills`, body);
    if (epoch !== this.epoch) throw new Error('Account changed.');
    return selection;
  }
}
