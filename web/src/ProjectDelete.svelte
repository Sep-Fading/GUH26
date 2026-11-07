<script lang="ts">
  // Keeps destructive confirmation focus and dismissal accessible.
  import { AlertDialog } from 'bits-ui';
  // Styled text field from the shared component library.
  import { Input } from '$lib/components/ui/input/index.js';
  // Marks deletion on the trigger.
  import Trash2 from '@lucide/svelte/icons/trash-2';
  // Deletes through the project registry and reads the VM's state.
  import type { Chat } from './chat.svelte';
  import type { Project } from './project-state.svelte';

  let { chat, project, ondeleted }: { chat: Chat; project: Project; ondeleted: () => void } = $props();
  let open = $state(false), pending = $state(false), error = $state('');
  let withVm = $state(false), typed = $state(''), vmState = $state('unknown');

  const vmId = $derived(project.environment?.kind === 'vm' && project.environment.dedicated
    ? project.environment.vmId : undefined);
  const stopped = $derived(['off', 'failed', 'missing'].includes(vmState));
  const ready = $derived(!withVm || typed === project.name);

  // Resets the choices on every opening and reads whether the VM is stopped.
  function opened(value: boolean) {
    if (pending) return;
    open = value; error = ''; withVm = false; typed = ''; vmState = 'unknown';
    if (value && vmId) void chat.vmState(vmId).then(status => { vmState = status.state; }, () => {});
  }

  // Closes only after acknowledged deletion; failures stay inside the confirmation.
  async function confirm() {
    if (pending || !ready) return;
    pending = true; error = '';
    try {
      await chat.projects.remove(project, withVm, typed);
      open = false; ondeleted();
      // The project's conversations are gone too; the task list must not keep showing them.
      await chat.afterProjectDeleted(project.id);
    }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Deletion failed.'; }
    finally { pending = false; }
  }
</script>

<AlertDialog.Root {open} onOpenChange={opened}>
  <AlertDialog.Trigger class="flex shrink-0 items-center gap-2 whitespace-nowrap rounded-md border
    border-destructive bg-background px-3 py-2 text-sm text-destructive disabled:opacity-40
    enabled:hover:bg-destructive enabled:hover:text-primary-foreground"
    aria-label="Delete project" disabled={chat.projects.saving}>
    <Trash2 size={16} aria-hidden="true" />Delete
  </AlertDialog.Trigger>
  <AlertDialog.Portal>
    <AlertDialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <AlertDialog.Content class="fixed left-1/2 top-1/2 z-[70] w-[min(420px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-background p-5 shadow-lg">
      <AlertDialog.Title class="text-base font-semibold">Delete project?</AlertDialog.Title>
      <AlertDialog.Description class="mt-2 break-words text-sm text-muted-foreground">
        Delete {project.name} with its settings, its permissions and every conversation in it.
        This cannot be undone.
      </AlertDialog.Description>
      {#if vmId}
        <label class="mt-4 flex items-start gap-2 text-sm">
          <input type="checkbox" class="mt-0.5" bind:checked={withVm} disabled={pending || !stopped} />
          <span>Also delete this project's VM</span>
        </label>
        {#if !stopped}
          <p class="mt-1 pl-6 text-xs text-muted-foreground">{vmState === 'unknown'
            ? 'Checking the VM...' : 'Shut the VM down first to delete it.'}</p>
        {:else if !withVm}
          <p class="mt-1 pl-6 text-xs text-muted-foreground">Kept, the VM is listed under Settings, Virtualisation.
            It must be running, so the conversations inside it can be deleted.</p>
        {/if}
        {#if withVm}
          <p class="mt-2 text-xs text-destructive">The VM's disk is deleted with every file, app and sign-in
            inside it. This cannot be undone and nothing goes to the Trash.</p>
          <label class="mt-3 block text-sm">Type the project name to confirm
            <Input class="mt-1" bind:value={typed} autocomplete="off" disabled={pending} />
          </label>
        {/if}
      {/if}
      {#if error}<p role="alert" class="mt-3 text-sm text-destructive">{error}</p>{/if}
      <div class="mt-5 flex justify-end gap-3">
        <AlertDialog.Cancel class="rounded-md border border-border px-3 py-2 text-sm" disabled={pending}>Cancel</AlertDialog.Cancel>
        <button type="button" class="rounded-md bg-destructive px-3 py-2 text-sm text-primary-foreground disabled:opacity-50"
          disabled={pending || !ready} onclick={confirm}>{withVm ? 'Delete project and VM' : 'Delete project'}</button>
      </div>
    </AlertDialog.Content>
  </AlertDialog.Portal>
</AlertDialog.Root>
