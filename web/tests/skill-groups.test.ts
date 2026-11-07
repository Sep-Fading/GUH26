// Checks stale responses and isolated group edits.
import { expect, test } from 'vitest';
// Exercises group state without mounting dialogs.
import { SkillGroups, type SkillGroup } from '../src/skill-groups.svelte';

const group: SkillGroup = { id: 'group', name: 'Writing', active: true,
  revision: 2, members: [{ skillId: 'skill', enabled: false }] };

test('late group lists cannot replace an acknowledged edit', async () => {
  let release!: (value: unknown) => void;
  const registry = new SkillGroups(async <T>(_path: string, body?: unknown): Promise<T> => {
    if (body) return { group } as T;
    return new Promise(resolve => { release = value => resolve(value as T); });
  }, () => true);
  const loading = registry.refresh();
  const { id, ...draft } = group;
  await registry.save(id, draft);
  release({ groups: [{ ...group, revision: 1, members: [] }] });
  await loading;
  expect(registry.items).toEqual([group]);
});

test('account changes discard group writes and selection responses', async () => {
  let release!: (value: unknown) => void;
  const registry = new SkillGroups(<T>() => new Promise<T>(resolve => {
    release = value => resolve(value as T);
  }), () => true);
  const saving = registry.save(undefined, group);
  registry.clear(); release({ group });
  expect(await saving).toBeUndefined();
  expect(registry.items).toEqual([]);
  const loading = registry.selection('thread');
  registry.clear(); release({ selection: { skillIds: [], skillGroupIds: [], revision: 1 } });
  await expect(loading).rejects.toThrow('Account changed.');
});

test('failed group edits retain stored members and expose the error', async () => {
  const registry = new SkillGroups(async () => { throw new Error('Revision changed.'); }, () => true);
  registry.items = [group];
  expect(await registry.save(group.id, { ...group, members: [] })).toBeUndefined();
  expect(registry.items[0].members).toEqual([{ skillId: 'skill', enabled: false }]);
  expect(registry.error).toBe('Revision changed.');
  expect(registry.saving).toBe(false);
});
