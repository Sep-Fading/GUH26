<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Keeps recovery confirmation focus and dismissal accessible.
  import { Dialog } from 'bits-ui';
  // Marks the explicit recovery dialog close control.
  import X from '@lucide/svelte/icons/x';

  type Preview = { token: string; unknownCount: number; knownCount: number;
    quarantined: boolean; preservedBaselineCount: number };
  let { account }: { account?: string } = $props();
  let open = $state(false);
  let preview = $state<Preview | null>(null);
  let confirmed = $state(false);
  let loading = $state(false);
  let pending = $state(false);
  let error = $state('');
  let success = $state('');
  let generation = 0;
  let request = 0;

  $effect(() => {
    account;
    generation += 1;
    open = false; preview = null; confirmed = false;
    error = ''; success = ''; loading = false; pending = false;
  });

  // Rejects malformed counts or tokens before allowing a confirmation action.
  function valid(value: unknown): value is Preview {
    if (!value || typeof value !== 'object') return false;
    const state = value as Preview;
    return typeof state.token === 'string' && /^[0-9a-f]{64}$/.test(state.token) &&
      Number.isInteger(state.unknownCount) && state.unknownCount >= 0 &&
      Number.isInteger(state.knownCount) && state.knownCount >= 0 &&
      state.unknownCount + state.knownCount <= 64 && typeof state.quarantined === 'boolean' &&
      Number.isInteger(state.preservedBaselineCount) && state.preservedBaselineCount >= 0 &&
      state.preservedBaselineCount <= 4096;
  }

  // Reads a fresh confirmation snapshot without changing any creation records.
  async function refresh() {
    if (!account || pending) return;
    const current = generation;
    const version = ++request;
    loading = true; confirmed = false; preview = null; error = ''; success = '';
    try {
      const response = await fetch('/api/creation-recovery', { signal: AbortSignal.timeout(8000) });
      if (!response.ok) throw new Error();
      const result = await response.json();
      if (!valid(result)) throw new Error();
      if (current === generation && version === request) preview = result;
    } catch {
      if (current === generation && version === request) error = 'Could not load recovery status. Try again.';
    } finally {
      if (current === generation && version === request) loading = false;
    }
  }

  // Opens the recovery preview without confirming or resending any work.
  function opened(value: boolean) {
    if (pending) return;
    open = value;
    if (value) void refresh();
  }

  // Confirms only the observed journal state and preserves unknown-chat denial.
  async function recover() {
    if (!preview || !confirmed || pending || loading || !preview.unknownCount) return;
    const current = generation;
    const token = preview.token;
    pending = true; error = '';
    try {
      const response = await fetch('/api/creation-recovery', {
        method: 'POST', signal: AbortSignal.timeout(40000),
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
        body: JSON.stringify({ token, confirmed: true }),
      });
      if (current !== generation) return;
      if (response.status === 409) {
        error = 'Recovery state changed. Refresh the preview and confirm again.';
        return;
      }
      if (!response.ok) throw new Error();
      const result = await response.json();
      if (!Number.isInteger(result?.released) || result.released < 1 || result.released > 64) throw new Error();
      if (current === generation) success = 'Creation capacity was freed. Unconfirmed chats remain blocked. Your message was not resent.';
    } catch {
      if (current === generation) error = 'Could not confirm recovery. Refresh the preview before trying again.';
    } finally {
      if (current === generation) { pending = false; confirmed = false; preview = null; }
    }
  }
</script>

<Dialog.Root {open} onOpenChange={opened}>
  <Dialog.Trigger class="mt-4 self-start text-sm underline" disabled={!account}>Creation recovery</Dialog.Trigger>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-1/2 z-[70] max-h-[85dvh] w-[min(440px,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-border bg-muted p-5 shadow-xl">
      <div class="flex items-center justify-between gap-3">
        <Dialog.Title class="font-semibold">Creation recovery</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close creation recovery" disabled={pending}><X size={18} aria-hidden="true" /></Dialog.Close>
      </div>
      <Dialog.Description class="my-3 text-sm text-muted-foreground">Free capacity to create new chats. Chats whose creation cannot be matched safely will stay blocked. This does not delete Codex chats or resend messages.</Dialog.Description>
      {#if error}<p role="alert" class="my-3 text-sm text-destructive">{error}</p>{/if}
      {#if success}<p role="status" class="my-3 text-sm">{success}</p>{/if}
      {#if loading}<p class="my-3 text-sm text-muted-foreground">Loading recovery status…</p>{/if}
      {#if preview}
        <p class="my-3 text-sm">{preview.unknownCount} unconfirmed creations. {preview.knownCount} known receipts are preserved.</p>
        {#if preview.unknownCount}
          <label class="my-4 flex items-start gap-2 text-sm">
            <input type="checkbox" bind:checked={confirmed} disabled={pending} class="mt-1" />
            <span>I understand that unknown chats will stay blocked permanently.</span>
          </label>
          <Button type="submit" 
      disabled={!confirmed || pending || loading} onclick={recover}>Free creation capacity</Button>
        {:else}<p class="my-3 text-sm text-muted-foreground">There are no unconfirmed creations to recover.</p>{/if}
      {/if}
      <button class="mt-4 block text-sm underline" disabled={loading || pending} onclick={refresh}>Refresh preview</button>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
