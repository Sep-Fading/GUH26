<script lang="ts">
  // Names the file of a native path on either platform.
  import { fileName } from './paths';
  // The coding session's review: one proposed change, shown as a diff, decided by the user.
  // Nothing is written to the file until they choose.
  import { Dialog } from 'bits-ui';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import DiffView from './DiffView.svelte';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import type { Chat } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
  const review = $derived(chat.pendingReview!);
  const name = $derived(fileName(review.path) || review.path);
  const folder = $derived(review.path.slice(0, -name.length));

  // `look` reads the diff; `edit` rewrites the proposal; `reject` adds an optional note.
  let mode = $state<'look' | 'edit' | 'reject'>('look');
  let mine = $state(''), note = $state('');
  // A new review starts from its own proposal, never from the last one's edits.
  let shown = '';
  $effect(() => { if (review.id !== shown) { shown = review.id; mode = 'look'; mine = review.proposed; note = ''; } });

  const minutes = $derived(Math.max(1, Math.round(review.expiresInSeconds / 60)));

  // Cmd or Ctrl with Return applies, as in a code review tool.
  function keys(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && mode !== 'reject' && !chat.deciding) {
      event.preventDefault();
      void chat.decideReview(mode === 'edit' ? 'apply' : 'apply', mode === 'edit' ? mine : undefined);
    }
  }
</script>

<Dialog.Root open={true}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-50 bg-black/50" />
    <Dialog.Content aria-label={`Review change to ${name}`} escapeKeydownBehavior="ignore" interactOutsideBehavior="ignore"
      onkeydown={keys}
      class="review fixed left-1/2 top-1/2 z-[60] flex max-h-[90dvh] w-[min(1040px,calc(100vw-1.5rem))] -translate-x-1/2
        -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border bg-background shadow-2xl">
      <header class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-5 py-3.5">
        <FilePen size={18} class="shrink-0 text-brand" aria-hidden="true" />
        <div class="min-w-0 flex-1">
          <!-- The dialog is named by its title, so the title says what is being asked. -->
          <div class="flex min-w-0 items-baseline text-[15px] leading-6">
            <span class="truncate text-muted-foreground" style="direction: rtl">{folder}</span>
            <Dialog.Title class="shrink-0 font-semibold"><span class="sr-only">Review change to {name}</span><span aria-hidden="true">{name}</span></Dialog.Title>
          </div>
          <Dialog.Description class="text-xs text-muted-foreground">
            {#if review.agent}Requested by {review.agent}. {/if}
            {review.kind === 'create' ? 'The agent wants to create this file.' : review.kind === 'write' ? 'The agent wants to replace this file.' : 'The agent wants to change this file.'}
            Nothing is written until you decide. With no decision in {minutes} min it is rejected.
          </Dialog.Description>
        </div>
      </header>

      <div class="min-h-0 flex-1 overflow-auto bg-background">
        {#if mode === 'edit'}
          <div class="flex h-full flex-col gap-2 p-4">
            <label class="text-xs text-muted-foreground" for="review-mine">New content of {name}. What you apply here is what gets written; the agent is told it differs from its proposal.</label>
            <Textarea id="review-mine" aria-label={`New content of ${name}`} bind:value={mine} spellcheck={false}
              class="min-h-[45dvh] flex-1 resize-none font-mono text-[12.5px] leading-5" />
          </div>
        {:else}
          <DiffView diff={review.diff} path={review.path} />
        {/if}
      </div>

      {#if mode === 'reject'}
        <div class="border-t border-border px-5 py-3">
          <label class="text-xs text-muted-foreground" for="review-note">Why, for the agent (optional)</label>
          <Textarea id="review-note" aria-label="Why, for the agent (optional)" bind:value={note} rows={2} maxlength={2000}
            placeholder="For example: keep the public name, change only the body." class="mt-1 resize-none text-sm" />
        </div>
      {/if}

      <footer class="flex flex-wrap items-center gap-2 border-t border-border bg-muted/50 px-5 py-3">
        {#if mode === 'reject'}
          <Button variant="ghost" disabled={!!chat.deciding} onclick={() => { mode = 'look'; }}>Back</Button>
          <span class="flex-1"></span>
          <Button variant="destructive" disabled={!!chat.deciding} onclick={() => chat.decideReview('reject', undefined, note)}>Reject change</Button>
        {:else if mode === 'edit'}
          <Button variant="ghost" disabled={!!chat.deciding} onclick={() => { mode = 'look'; mine = review.proposed; }}>Discard my edits</Button>
          <span class="flex-1"></span>
          <Button disabled={!!chat.deciding || mine === review.proposed} onclick={() => chat.decideReview('apply', mine)}>Apply my version</Button>
        {:else}
          <Button variant="outline" class="text-destructive" disabled={!!chat.deciding} onclick={() => { mode = 'reject'; }}>Reject</Button>
          <Button variant="ghost" disabled={!!chat.deciding} onclick={() => { mode = 'edit'; }}>Edit before applying</Button>
          <span class="flex-1"></span>
          <Button variant="ghost" disabled={!!chat.deciding} title="Stops asking for the rest of this turn. The next turn asks again."
            onclick={() => chat.decideReview('apply_all')}>Apply all in this turn</Button>
          <Button disabled={!!chat.deciding} onclick={() => chat.decideReview('apply')}>Apply change
            <kbd class="ml-1 rounded border border-white/30 px-1 text-[10px] font-normal opacity-80">⌘↵</kbd></Button>
        {/if}
      </footer>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
