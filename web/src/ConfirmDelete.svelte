<script lang="ts">
  // Keeps destructive confirmation focus and dismissal accessible.
  import { AlertDialog, Tooltip } from 'bits-ui';
  // Marks deletion in compact and labeled controls.
  import Trash2 from '@lucide/svelte/icons/trash-2';

  // `option` adds one box the user may tick, unticked each time the dialog opens; the action is told
  // whether it was ticked. Deleting a conversation's own folder is such a choice.
  let { label, actionLabel, description, triggerText = '', disabled = false, option, action }: {
    label: string; actionLabel: string; description: string; disabled?: boolean;
    triggerText?: string;
    option?: { label: string; detail: string };
    action: (ticked: boolean) => Promise<void>;
  } = $props();
  let open = $state(false), pending = $state(false), error = $state(''), ticked = $state(false);

  // Closes only after acknowledged deletion; failures stay inside the confirmation.
  async function confirm() {
    if (pending) return;
    pending = true; error = '';
    try { await action(ticked); open = false; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Deletion failed.'; }
    finally { pending = false; }
  }
</script>

<AlertDialog.Root {open} onOpenChange={value => { if (!pending) { open = value; error = ''; ticked = false; } }}>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <AlertDialog.Trigger {...props} class={triggerText
          ? `flex shrink-0 items-center gap-2 whitespace-nowrap rounded-md border
            border-destructive bg-background px-3 py-2 text-sm text-destructive disabled:opacity-40
            enabled:hover:bg-destructive enabled:hover:text-primary-foreground`
          : 'icon-button shrink-0 text-muted-foreground hover:text-destructive'}
          aria-label={label} {disabled}>
          <Trash2 size={triggerText ? 16 : 15} aria-hidden="true" />{triggerText}
        </AlertDialog.Trigger>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content class="tooltip" sideOffset={8}>{actionLabel}</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
  <AlertDialog.Portal>
    <AlertDialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <AlertDialog.Content class="fixed left-1/2 top-1/2 z-[70] w-[min(400px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-background p-5 shadow-lg">
      <AlertDialog.Title class="text-base font-semibold">{actionLabel}?</AlertDialog.Title>
      <AlertDialog.Description class="mt-2 break-words text-sm text-muted-foreground">{description}</AlertDialog.Description>
      {#if option}
        <label class="mt-3 flex items-start gap-2 text-sm">
          <input type="checkbox" class="mt-0.5" bind:checked={ticked} disabled={pending} />
          <span><span class="block">{option.label}</span>
            <span class="block break-all font-mono text-xs text-muted-foreground">{option.detail}</span></span>
        </label>
      {/if}
      {#if error}<p role="alert" class="mt-3 text-sm text-destructive">{error}</p>{/if}
      <div class="mt-5 flex justify-end gap-3">
        <AlertDialog.Cancel class="rounded-md border border-border px-3 py-2 text-sm" disabled={pending}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action class="rounded-md bg-destructive px-3 py-2 text-sm text-primary-foreground disabled:opacity-50"
          disabled={pending} onclick={confirm}>{actionLabel}</AlertDialog.Action>
      </div>
    </AlertDialog.Content>
  </AlertDialog.Portal>
</AlertDialog.Root>
