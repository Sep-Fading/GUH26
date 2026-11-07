// Checks registry races and source preservation.
import { expect, test } from 'vitest';
// Exercises copy state independently of dialogs.
import { Skills, type Skill } from '../src/skill-state.svelte';
// Configures editor state without mounting browser views.
import { Compartment } from '@codemirror/state';
// Checks bounded changes and literal source retention.
import { editorState } from '../src/instruction-editor';

const skill: Skill = { id: 'fixture', revision: 2, active: false,
  name: 'Clarity', description: '', instructions: '# Keep exact terms' };

test('late list cannot replace a saved skill revision', async () => {
  let release!: (value: unknown) => void;
  const registry = new Skills(async <T>(path: string, body?: unknown): Promise<T> => {
    if (body) return { skill } as T;
    return new Promise(resolve => { release = value => resolve(value as T); });
  }, () => true);
  const loading = registry.refresh();
  const { id, ...draft } = skill;
  await registry.save(id, draft);
  release({ skills: [{ ...skill, active: true, revision: 1 }] });
  await loading;
  expect(registry.items).toEqual([skill]);
});

test('account reset discards pending skill writes', async () => {
  let release!: (value: unknown) => void;
  const registry = new Skills(<T>() => new Promise<T>(resolve => {
    release = value => resolve(value as T);
  }), () => true);
  const saving = registry.save(undefined, skill);
  registry.clear();
  release({ skill });
  expect(await saving).toBeUndefined();
  expect(registry.items).toEqual([]);
  expect(registry.saving).toBe(false);
});

test('editor changes retain CRLF, code and frontmatter', () => {
  const source = '---\r\nname: exact\r\n---\r\n\r\n**bold** and `literal`';
  const state = editorState(source, 'test', new Compartment(), false, () => {});
  expect(state.sliceDoc()).toBe(source);
  const updated = state.update({ changes: { from: state.doc.length, insert: ' end' } }).state;
  expect(updated.sliceDoc()).toBe(source + ' end');
});

test('editor rejects changes over the instruction limit', () => {
  const source = 'x'.repeat(16384);
  const state = editorState(source, 'test', new Compartment(), false, () => {});
  expect(state.update({ changes: { from: state.doc.length, insert: 'x' } }).state.sliceDoc()).toBe(source);
});
