<script lang="ts">
  // A modal the user cannot dismiss by accident: only an answer, "Answer in chat" or Stop ends it.
  import { Dialog } from 'bits-ui';
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  // Marks the dialog and the option the user picked.
  import MessageCircleQuestion from '@lucide/svelte/icons/message-circle-question-mark';
  import Check from '@lucide/svelte/icons/check';
  // Reads the open question and sends the answers.
  import type { Chat, PendingQuestion } from './chat.svelte';

  let { chat, question }: { chat: Chat; question: PendingQuestion } = $props();
  // One answer per question: the labels picked and the user's own words. The dialog is created
  // anew for each question id, so the first value of `question` is the only one it ever has.
  // svelte-ignore state_referenced_locally
  let picked = $state<string[][]>(question.questions.map(() => []));
  // svelte-ignore state_referenced_locally
  let own = $state<string[]>(question.questions.map(() => ''));
  let busy = $state(false);

  const answered = $derived(question.questions.every((_, index) => picked[index].length > 0 || own[index].trim().length > 0));

  // One pick replaces the last unless the question allows several.
  function pick(index: number, label: string, several: boolean) {
    const has = picked[index].includes(label);
    picked[index] = several ? (has ? picked[index].filter(item => item !== label) : [...picked[index], label]) : (has ? [] : [label]);
  }

  async function send() {
    if (!answered || busy) return;
    busy = true;
    await chat.answerQuestion(question.id, { answers: question.questions.map((_, index) => ({ selected: $state.snapshot(picked[index]),
      ...(own[index].trim() ? { other: own[index].trim() } : {}) })) });
    busy = false;
  }

  async function inChat() {
    if (busy) return;
    busy = true;
    await chat.answerQuestion(question.id, { action: 'chat' });
    busy = false;
  }
</script>

<Dialog.Root open={true}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-50 bg-black/45" />
    <Dialog.Content escapeKeydownBehavior="ignore" interactOutsideBehavior="ignore"
      class="fixed left-1/2 top-1/2 z-[60] flex max-h-[min(44rem,92dvh)] w-[min(38rem,calc(100vw-1.5rem))] -translate-x-1/2 -translate-y-1/2
        flex-col rounded-2xl border border-border bg-background shadow-xl">
      <div class="flex items-start gap-3 border-b border-border px-5 py-4">
        <span class="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-brand">
          <MessageCircleQuestion size={17} aria-hidden="true" /></span>
        <div class="min-w-0">
          <Dialog.Title class="text-base font-semibold">The agent has a question</Dialog.Title>
          <Dialog.Description class="mt-0.5 text-xs text-muted-foreground">{#if question.agent}{question.agent} is waiting for your answer.{:else}It waits for your answer.{/if} Pick from the options, write your own, or both.</Dialog.Description>
        </div>
      </div>
      <div class="min-h-0 flex-1 space-y-6 overflow-y-auto px-5 py-4">
        {#each question.questions as item, index (index)}
          <fieldset class="space-y-2.5" disabled={busy}>
            <legend class="w-full">
              {#if item.header}<span class="mb-1.5 inline-block rounded-full bg-muted px-2 py-0.5 text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground" aria-hidden="true">{item.header}</span>{/if}
              <span class="block text-sm font-medium">{item.question}</span>
            </legend>
            {#if item.multiSelect}<p class="text-xs text-muted-foreground">Choose any that apply.</p>{/if}
            <div class="space-y-1.5">
              {#each item.options as option (option.label)}
                {@const on = picked[index].includes(option.label)}
                <label class={`relative flex cursor-pointer items-start gap-3 rounded-xl border px-3 py-2.5 transition-colors
                  ${on ? 'border-brand bg-brand/5' : 'border-border hover:bg-muted/60'}`}>
                  <input type={item.multiSelect ? 'checkbox' : 'radio'} name={`question-${index}`} class="peer absolute inset-0 cursor-pointer opacity-0" checked={on}
                    onclick={() => pick(index, option.label, item.multiSelect)} />
                  <span class={`mt-0.5 flex size-4 shrink-0 items-center justify-center border peer-focus-visible:ring-2 peer-focus-visible:ring-ring
                    ${item.multiSelect ? 'rounded-[5px]' : 'rounded-full'} ${on ? 'border-brand bg-brand text-brand-foreground' : 'border-border'}`} aria-hidden="true">
                    {#if on}<Check size={11} stroke-width={3} />{/if}</span>
                  <span class="min-w-0">
                    <span class="block text-sm font-medium">{option.label}</span>
                    {#if option.description}<span class="mt-0.5 block text-xs text-muted-foreground">{option.description}</span>{/if}
                  </span>
                </label>
              {/each}
            </div>
            <Textarea aria-label="Your own answer" rows={1} placeholder="Or write your own answer" maxlength={4000} bind:value={own[index]}
              class="min-h-9 resize-none text-sm" />
          </fieldset>
        {/each}
      </div>
      <div class="flex flex-wrap items-center justify-between gap-2 border-t border-border px-5 py-3">
        {#if !question.thread}<Button variant="ghost" size="sm" disabled={busy} title="Stops this response so you can reply with a message instead"
          onclick={inChat}>Answer in chat</Button>{/if}
        <Button disabled={!answered || busy} onclick={send}>Send answers</Button>
      </div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
