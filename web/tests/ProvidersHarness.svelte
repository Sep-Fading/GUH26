<script lang="ts">
  // Renders the production provider settings with a test chat.
  import Providers from '../src/Providers.svelte';
  import type { Chat } from '../src/chat.svelte';
  let { chat }: { chat: Chat } = $props();
</script>
<Providers {chat} />
