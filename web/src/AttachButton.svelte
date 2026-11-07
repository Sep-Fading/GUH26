<script lang="ts">
  // Explains the button on hover and focus.
  import { Tooltip } from 'bits-ui';
  // Marks the attach command.
  import Paperclip from '@lucide/svelte/icons/paperclip';
  // Receives the chosen files.
  import type { Chat } from './chat.svelte';

  let { chat, disabled = false }: { chat: Chat; disabled?: boolean } = $props();
  let picker = $state<HTMLInputElement>();

  // Hands the chosen files to the draft and empties the picker, so the same file can be chosen again.
  function chosen() {
    if (picker?.files) chat.addFiles(picker.files);
    if (picker) picker.value = '';
  }
</script>

<input bind:this={picker} type="file" multiple class="hidden" tabindex="-1" aria-hidden="true" onchange={chosen} data-attach-input />
<Tooltip.Root>
  <Tooltip.Trigger class="icon-button shrink-0" aria-label="Attach files" {disabled} onclick={() => picker?.click()}>
    <Paperclip size={16} aria-hidden="true" />
  </Tooltip.Trigger>
  <Tooltip.Portal>
    <Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>Attach files or pictures. You can also drop or paste them.</Tooltip.Content>
  </Tooltip.Portal>
</Tooltip.Root>
