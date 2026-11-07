<script lang="ts">
  // Supplies the application tooltip context for tests.
  import { Tooltip } from 'bits-ui';
  // Exercises the production MCP servers panel directly.
  import McpServers from '../src/McpServers.svelte';
  import type { Chat } from '../src/chat.svelte';
  let { chat }: { chat: Chat } = $props();
</script>
<Tooltip.Provider><McpServers {chat} /></Tooltip.Provider>
