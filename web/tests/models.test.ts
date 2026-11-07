import { afterEach, expect, test, vi } from 'vitest';
import { Chat } from '../src/chat.svelte';
const models = [
  { model: 'fixture-model', displayName: 'Model A', description: 'First model',
    hidden: false, isDefault: true, defaultReasoningEffort: 'low',
    supportedReasoningEfforts: ['low', 'high'].map(reasoningEffort =>
      ({ reasoningEffort, description: reasoningEffort })) },
  { model: 'other-model', displayName: 'Model B', description: 'Second model',
    hidden: true, isDefault: false, defaultReasoningEffort: 'none',
    supportedReasoningEfforts: ['none', 'ultra'].map(reasoningEffort =>
      ({ reasoningEffort, description: reasoningEffort })) },
];

afterEach(() => vi.unstubAllGlobals());

test('model changes reset effort to its own default', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ models })));
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  await chat.refreshModels();
  expect(chat.model).toBe('fixture-model');
  chat.setEffort('high');
  chat.setModel('other-model');
  expect(chat.effort).toBe('none');
  chat.setEffort('ultra');
  expect(chat.effort).toBe('ultra');
  chat.setEffort('high');
  expect(chat.effort).toBe('ultra');
});

test('refresh preserves a valid selection and draft', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => Response.json({ models })));
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  await chat.refreshModels();
  chat.setModel('other-model');
  chat.setEffort('ultra');
  chat.draft = 'Keep this';
  await chat.refreshModels();
  expect(chat.model).toBe('other-model');
  expect(chat.effort).toBe('ultra');
  expect(chat.draft).toBe('Keep this');
});

test('no reasoning options means no explicit effort', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({
    models: [{ ...models[0], supportedReasoningEfforts: [] }],
  })));
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  await chat.refreshModels();
  expect(chat.effort).toBe('');
});

test('a removed model does not silently switch the draft', async () => {
  const fetch = vi.fn(async () => Response.json({ models }));
  vi.stubGlobal('fetch', fetch);
  const chat = new Chat();
  chat.account = { status: 'signed_in' };
  await chat.refreshModels();
  chat.setModel('other-model');
  fetch.mockResolvedValue(Response.json({ models: [models[0]] }));
  await chat.refreshModels();
  expect(chat.model).toBe('other-model');
  expect(chat.validChoice).toBe(false);
});
