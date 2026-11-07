// Verifies renewal requires another explicit user action.
import { expect, test, vi } from 'vitest';
// Renders the real composer capability control.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
// Supplies the real control and conversation type.
import Capabilities from './ShellAccessHarness.svelte';
import type { Chat } from '../src/chat.svelte';

test('expired access prompts without renewing itself', async () => {
  const toggleCapability = vi.fn(async () => true);
  const chat = {
    ready: true, active: { id: 'chat' }, agentId: 'agent', projectId: '', machine: 'host',
    capabilities: vi.fn(async () => ({ capabilities: [{ key: 'unsandboxed_shell',
      name: 'Unsandboxed shell', allowed: true, on: false, expired: true, expiresAt: Date.now() - 1 }] })),
    toggleCapability, draftCapabilities: {},
  } as unknown as Chat;
  render(Capabilities, { chat });
  expect(await screen.findByRole('alert')).toHaveTextContent('access expired and is off');
  expect(toggleCapability).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByText('Review and renew access'));
  const duration = await screen.findByLabelText('Shell access duration in minutes');
  await fireEvent.input(duration, { target: { value: '7' } });
  await fireEvent.click(screen.getByRole('switch', { name: 'Unsandboxed shell' }));
  await waitFor(() => expect(toggleCapability).toHaveBeenCalledWith('unsandboxed_shell', true, 420));
});
