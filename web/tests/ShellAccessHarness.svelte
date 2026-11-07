<script lang="ts">
  // Supplies the application tooltip context for tests.
  import { Tooltip } from 'bits-ui';
  // Exercises the production capability control directly.
  import Capabilities from '../src/Capabilities.svelte';
  import type { Chat } from '../src/chat.svelte';
  let { chat }: { chat: Chat } = $props();
</script>
<Tooltip.Provider><Capabilities {chat} /></Tooltip.Provider>
