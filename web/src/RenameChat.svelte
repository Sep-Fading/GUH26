<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Holds focus inside the naming dialog and explains its button.
  import { Dialog, Tooltip } from 'bits-ui';
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  // Marks the rename command.
  import Pencil from '@lucide/svelte/icons/pencil';
  // Saves the name through the backend.
  import type { Chat, Task } from './chat.svelte';

  let { chat, task, disabled = false }: { chat: Chat; task: Task; disabled?: boolean } = $props();
  let open = $state(false), pending = $state(false), error = $state(''), name = $state('');

  // Saves the typed name, or with `''` gives the conversation back the name it had.
  async function save(value: string) {
    if (pending) return;
    pending = true; error = '';
    try { await chat.renameChat(task, value); open = false; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The name was not saved.'; }
    finally { pending = false; }
  }
</script>

<Dialog.Root {open} onOpenChange={value => { if (!pending) { open = value; error = ''; if (value) name = task.name; } }}>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Dialog.Trigger {...props} class="icon-button shrink-0 text-muted-foreground hover:text-foreground"
          aria-label={`Rename chat ${task.name}`} {disabled}>
          <Pencil size={15} aria-hidden="true" />
        </Dialog.Trigger>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content class="tooltip" sideOffset={8}>Rename chat</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-1/2 z-[70] w-[min(400px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-background p-5 shadow-lg">
      <Dialog.Title class="text-base font-semibold">Rename chat</Dialog.Title>
      <Dialog.Description class="mt-2 text-sm text-muted-foreground">
        Your name replaces the one taken from the first message. It is kept on {words.machine}.
      </Dialog.Description>
      <form class="mt-4" onsubmit={event => { event.preventDefault(); void save(name); }}>
        <Input aria-label="Chat name" maxlength={120} bind:value={name} disabled={pending} />
        {#if error}<p role="alert" class="mt-3 text-sm text-destructive">{error}</p>{/if}
        <div class="mt-5 flex flex-wrap justify-end gap-3">
          {#if task.providerName}
            <Button type="button" variant="ghost" class="mr-auto" disabled={pending} onclick={() => save('')}>Use original name</Button>
          {/if}
          <Dialog.Close class="rounded-md border border-border px-3 py-2 text-sm" disabled={pending}>Cancel</Dialog.Close>
          <Button type="submit" disabled={pending || !name.trim() || name.trim() === task.name}>Save</Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
