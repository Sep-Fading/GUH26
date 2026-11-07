<script lang="ts">
  // Provides keyboard navigation between settings pages.
  import { Tabs } from 'bits-ui';
  // Keeps permission editing in its existing component.
  import PermissionSettings from './PermissionSettings.svelte';
  // Loads and edits future conversation switches.
  import CapabilityDefaults from './CapabilityDefaults.svelte';
  // Shares the application's authenticated request helper.
  import type { Chat } from './chat.svelte';

  let { chat, id, capabilitiesOnly = false }: { chat: Chat; id: string; capabilitiesOnly?: boolean } = $props();
  let tab = $state('permissions');
</script>

{#key id}
  <Tabs.Root bind:value={tab} class="min-w-0 space-y-4">
    <Tabs.List aria-label="Agent capabilities" class="flex flex-wrap gap-1 border-b border-border pb-2">
      <Tabs.Trigger value="permissions" class="rounded-md px-3 py-2 text-sm text-muted-foreground hover:bg-muted data-[state=active]:bg-muted data-[state=active]:text-foreground">Permissions</Tabs.Trigger>
      <Tabs.Trigger value="defaults" class="rounded-md px-3 py-2 text-sm text-muted-foreground hover:bg-muted data-[state=active]:bg-muted data-[state=active]:text-foreground">Enabled by default</Tabs.Trigger>
    </Tabs.List>
    <Tabs.Content value="permissions"><PermissionSettings {chat} kind="agent" {id} {capabilitiesOnly} /></Tabs.Content>
    <Tabs.Content value="defaults"><CapabilityDefaults {chat} {id} active={tab === 'defaults'} /></Tabs.Content>
  </Tabs.Root>
{/key}
