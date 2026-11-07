export type Skill = { id: string; name: string; description: string; instructions: string;
  active: boolean; slash?: boolean; revision: number };
export type SkillDraft = Omit<Skill, 'id' | 'revision'> & { revision?: number };
type Source = { id: string; name: string };
type Api = <T>(path: string, body?: unknown) => Promise<T>;

export class Skills {
  items = $state<Skill[]>([]);
  sources = $state<Source[]>([]);
  loading = $state(false);
  saving = $state(false);
  discovering = $state(false);
  error = $state('');
  private epoch = 0;
  private writes = 0;

  // Uses the chat's guarded API and current account readiness.
  constructor(private api: Api, private ready: () => boolean) {}

  // Clears account-bound state and invalidates outstanding replies.
  clear() {
    this.epoch++;
    this.items = []; this.sources = []; this.error = '';
    this.loading = false; this.saving = false; this.discovering = false;
  }

  // Loads copies without letting an older list overwrite a completed edit.
  async refresh() {
    if (!this.ready() || this.loading || this.saving) return;
    const epoch = this.epoch, writes = this.writes;
    this.loading = true;
    try {
      const { skills } = await this.api<{ skills: Skill[] }>('/api/skills');
      if (epoch !== this.epoch || writes !== this.writes) return;
      if (!Array.isArray(skills)) throw new Error('Skills unavailable');
      this.items = skills; this.error = '';
    } catch (error) {
      if (epoch === this.epoch && writes === this.writes) this.failure(error);
    } finally { if (epoch === this.epoch) this.loading = false; }
  }

  // Lists source names only after the user opens import controls.
  async discover(provider: 'codex' | 'claude' = 'codex') {
    if (!this.ready() || this.discovering) return;
    const epoch = this.epoch;
    this.discovering = true; this.error = '';
    try {
      this.sources = [];
      const { sources } = await this.api<{ sources: Source[] }>(`/api/skills/sources?provider=${provider}`);
      if (epoch === this.epoch) this.sources = sources;
    } catch (error) { if (epoch === this.epoch) this.failure(error); }
    finally { if (epoch === this.epoch) this.discovering = false; }
  }

  // Saves an explicit local edit and returns the acknowledged revision.
  async save(id: string | undefined, draft: SkillDraft) {
    return this.write(id ? `/api/skills/${id}` : '/api/skills', draft);
  }

  // Deletes one observed owned copy; installed sources and pinned snapshots remain untouched.
  async remove(id: string, revision: number) {
    if (!this.ready() || this.saving) throw new Error('Skill settings are busy.');
    const epoch = this.epoch;
    this.saving = true;
    try {
      await this.api(`/api/skills/${id}/delete`, { revision });
      if (epoch !== this.epoch) throw new Error('Account changed.');
      this.writes++;
      this.items = this.items.filter(s => s.id !== id);
    } finally { if (epoch === this.epoch) this.saving = false; }
  }

  // Imports the selected source as a new inactive copy.
  async import(source: string, provider: 'codex' | 'claude' = 'codex') {
    return this.write('/api/skills/import', { source, provider });
  }

  // Serializes mutations and rejects replies from a previous account.
  private async write(path: string, body: unknown): Promise<Skill | undefined> {
    if (!this.ready() || this.saving) return;
    const epoch = this.epoch;
    this.saving = true; this.error = '';
    try {
      const { skill } = await this.api<{ skill: Skill }>(path, body);
      if (epoch !== this.epoch) return;
      this.writes++;
      this.items = [...this.items.filter(s => s.id !== skill.id), skill];
      return skill;
    } catch (error) { if (epoch === this.epoch) this.failure(error); }
    finally { if (epoch === this.epoch) this.saving = false; }
  }

  // Keeps request failures visible without discarding the draft.
  private failure(error: unknown) {
    this.error = error instanceof Error ? error.message : 'Skills unavailable';
  }
}
