<script lang="ts">
  // Opens as one sheet with a category sidebar.
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  // Shows the gear's name on hover.
  import { Tooltip } from 'bits-ui';
  // Icons for the gear and each category.
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import X from '@lucide/svelte/icons/x';
  import Palette from '@lucide/svelte/icons/palette';
  import Plug from '@lucide/svelte/icons/plug';
  import Users from '@lucide/svelte/icons/users';
  import BookOpen from '@lucide/svelte/icons/book-open';
  import Monitor from '@lucide/svelte/icons/monitor';
  // The panels, each of which loads its own data when shown.
  import AppearanceSettings from './AppearanceSettings.svelte';
  import Providers from './Providers.svelte';
  import Agents from './Agents.svelte';
  import Skills from './Skills.svelte';
  import Virtualisation from './Virtualisation.svelte';
  // Saved prompts that run by themselves.
  import Schedules from './Schedules.svelte';
  import CalendarClock from '@lucide/svelte/icons/calendar-clock';
  // The one list of MCP servers for both providers.
  import McpServers from './McpServers.svelte';
  import Cable from '@lucide/svelte/icons/cable';
  // What MiniClaw recorded about its own use.
  import Analytics from './Analytics.svelte';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  // Shares account readiness and the request helper.
  import type { Chat } from './chat.svelte';

  // The sheet has no button of its own: the sidebar and the phone drawer each open it.
  let { chat, open = $bindable(false) }: { chat: Chat; open?: boolean } = $props();
  const categories = [
    { id: 'appearance', label: 'Appearance', icon: Palette, needsAccount: false },
    { id: 'providers', label: 'Providers', icon: Plug, needsAccount: false },
    { id: 'agents', label: 'Agents', icon: Users, needsAccount: true },
    { id: 'skills', label: 'Skills', icon: BookOpen, needsAccount: true },
    { id: 'mcp', label: 'MCP servers', icon: Cable, needsAccount: true },
    { id: 'schedules', label: 'Schedules', icon: CalendarClock, needsAccount: true },
    { id: 'analytics', label: 'Analytics', icon: ChartNoAxesColumn, needsAccount: true },
    { id: 'virtualisation', label: 'Virtualisation', icon: Monitor, needsAccount: true },
  ] as const;
  type Category = typeof categories[number]['id'];
  let current = $state<Category>('appearance');
  // A proposed MCP server brings the sheet to its panel, where the form opens filled in.
  $effect(() => { if (chat.mcpProposal && open) current = 'mcp'; });
  $effect(() => { if (chat.skillProposal && open) current = 'skills'; });
  $effect(() => { if (chat.agentProposal && open) current = 'agents'; });
  // A form that is saving keeps the sheet open, as the separate dialogs did.
  const saving = $derived(chat.agentSaving || chat.skills.saving || chat.groups.saving);
</script>

<Dialog.Root {open} onOpenChange={value => { if (!saving) open = value; }}>
  <!-- Every panel's header is 48 px tall; the sheet's own close button sits on that centre line. -->
  <Dialog.Content aria-label="Settings" showCloseButton={false} class="flex h-[min(44rem,90dvh)] w-[min(60rem,calc(100%-24px))] max-w-none
    flex-col gap-0 overflow-hidden p-0 sm:max-w-none sm:flex-row">
    <Dialog.Close class="icon-button absolute right-2 top-2 z-10 bg-sidebar sm:right-2.5 sm:top-1.5 sm:bg-transparent" aria-label="Close"><X size={16} aria-hidden="true" /></Dialog.Close>
    <Dialog.Title class="sr-only">Settings</Dialog.Title>
    <Dialog.Description class="sr-only">Appearance, providers, agents, skills, MCP servers and virtual machines.</Dialog.Description>
    <div aria-label="Settings categories" role="tablist" aria-orientation="vertical"
      class="flex shrink-0 gap-1 overflow-x-auto border-b border-border bg-sidebar p-2 pr-12 sm:w-48 sm:flex-col
        sm:overflow-visible sm:border-b-0 sm:border-r sm:p-3">
      <p class="hidden px-2 pb-2 pt-1 text-xs font-medium text-muted-foreground sm:block">Settings</p>
      {#each categories as category (category.id)}
        {@const Icon = category.icon}
        <button type="button" role="tab" aria-selected={current === category.id}
          disabled={saving || (category.needsAccount && !chat.ready)}
          class={`flex shrink-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-sm disabled:opacity-40
            ${current === category.id ? 'bg-sidebar-accent font-medium' : 'hover:bg-sidebar-accent/60'}`}
          onclick={() => { current = category.id; }}>
          <Icon size={16} aria-hidden="true" />{category.label}
        </button>
      {/each}
    </div>
    <div role="tabpanel" class="flex min-h-0 min-w-0 flex-1 flex-col bg-background">
      {#if current === 'appearance'}<AppearanceSettings />
      {:else if current === 'providers'}<Providers {chat} />
      {:else if current === 'agents'}<Agents {chat} />
      {:else if current === 'skills'}<Skills {chat} />
      {:else if current === 'analytics'}<Analytics {chat} />
      {:else if current === 'mcp'}<McpServers {chat} />
      {:else if current === 'schedules'}<Schedules {chat} />
      {:else}<Virtualisation {chat} />{/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
