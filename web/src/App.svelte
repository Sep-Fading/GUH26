<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Manages chat lifecycle and waits for rendering.
  import { onMount, tick } from 'svelte';
  // Provides accessible tooltips and pressed toggles.
  import { AlertDialog, Dialog, Tooltip } from 'bits-ui';
  // Icons for chat, history, and computer controls.
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import SquarePen from '@lucide/svelte/icons/square-pen';
  import Search from '@lucide/svelte/icons/search';
  // Marks the speed choice, which on a phone shows as this icon alone.
  import Zap from '@lucide/svelte/icons/zap';
  // Marks MiniClaw's notice that a delegated agent finished.
  import Users from '@lucide/svelte/icons/users';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import History from '@lucide/svelte/icons/history';
  import Monitor from '@lucide/svelte/icons/monitor';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import Shield from '@lucide/svelte/icons/shield';
  // Groups project and standalone conversation navigation.
  import TaskNav from './TaskNav.svelte';
  // Resizes the navigation pane and remembers width.
  import SidebarResize from './SidebarResize.svelte';
  // Navigates loaded exchanges with prompt response previews.
  import ConversationRail from './ConversationRail.svelte';
  // Searches history and opens its matching message.
  import ConversationSearch, { type Hit } from './ConversationSearch.svelte';
  // Renders the model and reasoning selectors.
  import Choice from './Choice.svelte';
  // Opens profile management beside chat selection.
  // Opens skill management beside agent profiles.
  // Shows provider choices and sign-in.
  import Settings from './Settings.svelte';
  // Shows a running Claude sign-in on the first screen.
  import ClaudeSignIn from './ClaudeSignIn.svelte';
  // Shares limit labels and reset formatting.
  import { usageRows, resetTime } from './usage';
  // Lists slash commands above the composer.
  import SlashMenu from './SlashMenu.svelte';
  // Everything a conversation can switch on or off, in one dropdown.
  import Capabilities from './Capabilities.svelte';
  // Shows the messages waiting for their turn.
  import MessageQueue from './MessageQueue.svelte';
  // Files of the draft, the attach command, and the files of a sent message.
  import DraftFiles from './DraftFiles.svelte';
  import AttachButton from './AttachButton.svelte';
  import MessageFiles from './MessageFiles.svelte';
  // Words a delegated run's time limit in hours and minutes.
  import { duration } from './delegation-words';
  // Shows a question the model asked, with options and the user's own answer.
  import QuestionDialog from './QuestionDialog.svelte';
  import ListPlus from '@lucide/svelte/icons/list-plus';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  // Swarm mission control: many agents, one goal.
  import SwarmView from './SwarmView.svelte';
  // Selects skills without reloading conversation content.
  import ConversationSkills from './ConversationSkills.svelte';
  // Shows saved consent sources for this conversation.
  import PermissionSettings from './PermissionSettings.svelte';
  // Icons for stopping, signing in, and approvals.
  import Square from '@lucide/svelte/icons/square';
  import LogIn from '@lucide/svelte/icons/log-in';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  // Icons for the VM start prompt.
  import Power from '@lucide/svelte/icons/power';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  // Shows backend status and its retry control.
  import Connection from './Connection.svelte';
  // Opens durable account-specific failure notices globally.
  import Notifications from './Notifications.svelte';
  // Renders assistant replies as inert Markdown.
  import MessageMarkdown from './MessageMarkdown.svelte';
  // Shows what MiniClaw's tool did in a turn, live and afterwards.
  import ActivityList from './ActivityList.svelte';
  // Shows a run delegated to another agent: its state, activity, named answer and transcript.
  import DelegationBlock from './DelegationBlock.svelte';
  // Holds each proposed change of a coding session until the user decides.
  import ReviewDialog from './ReviewDialog.svelte';
  // Announces a VM that is about to shut down for being idle.
  import IdleWarnings from './IdleWarnings.svelte';
  // Renders the passive guest desktop.
  import WorkspacePanel from './WorkspacePanel.svelte';
  // Owns reactive chat state and API calls.
  import { Chat } from './chat.svelte';

  const chat = new Chat();
  // How MiniClaw's own messages begin (`delegation.rs` NOTICE); the page shows them as a notice line.
  const NOTICE = '[MiniClaw notice] ';
  let conversation = $state<HTMLElement>();
  let follow = true;
  let jumping = false;
  let searchMatch = $state('');
  let navigationOpen = $state(false);
  let navigationWidth = $state<number>();
  let navigationActualWidth = $state(240);
  let settingsOpen = $state(false);
  // The swarm view replaces the conversation while it is open; choosing a conversation closes it.
  let swarmOpen = $state(false);
  let swarmView = $state<SwarmView>();
  let swarmTarget = $state<string | null>(null);
  let searchOpen = $state(false);
  let slashMenu = $state<SlashMenu>();
  // True while files are dragged over the composer.
  let dropping = $state(false);

  // A card in an answer asks to add a server: the form opens filled in, and the user saves it or not.
  $effect(() => {
    const add = (event: Event) => { chat.mcpProposal = (event as CustomEvent).detail; settingsOpen = true; };
    const skill = (event: Event) => { chat.skillProposal = (event as CustomEvent).detail; settingsOpen = true; };
    const agent = (event: Event) => { chat.agentProposal = (event as CustomEvent).detail; settingsOpen = true; };
    window.addEventListener('miniclaw:add-mcp-server', add);
    window.addEventListener('miniclaw:add-skill', skill);
    window.addEventListener('miniclaw:add-agent', agent);
    return () => {
      window.removeEventListener('miniclaw:add-mcp-server', add);
      window.removeEventListener('miniclaw:add-skill', skill);
      window.removeEventListener('miniclaw:add-agent', agent);
    };
  });
  onMount(() => chat.mount());
  // Follow new text only while the reader stays near the bottom.
  $effect(() => {
    const text = chat.messages.map(message => message.text).join('');
    if (text && follow) void tick().then(() => {
      if (conversation && follow) conversation.scrollTop = conversation.scrollHeight;
    });
  });
  // Refreshes the passive VM indicator when the selected machine or task changes.
  $effect(() => {
    void chat.machine;
    void chat.active?.id;
    void chat.vmTarget;
    void chat.refreshVmStatus().catch(() => {});
  });
  // Sends on Enter while leaving Shift+Enter and IME composition available for editing.
  function keydown(event: KeyboardEvent) {
    // An open slash menu takes the arrow keys, Enter, Tab and Escape first.
    if (slashMenu?.key(event)) return;
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      // While a response runs, Enter queues the message behind it.
      void (chat.queueable ? chat.enqueue() : chat.send());
    }
  }
  // Loads older messages and adjusts scroll position to keep the current text in view.
  async function loadOlder() {
    if (!conversation) return;
    const task = chat.active?.id;
    const height = conversation.scrollHeight;
    const top = conversation.scrollTop;
    follow = false;
    await chat.loadOlder();
    await tick();
    // Prepending history should not move the message being read.
    if (conversation && task === chat.active?.id) {
      conversation.scrollTop = top + conversation.scrollHeight - height;
    }
  }

  // Closes the mobile drawer before opening the shared search dialog.
  async function showSearch() {
    navigationOpen = false;
    await tick();
    searchOpen = true;
  }

  // Opens a result, loading older pages until its message is present, then focuses it.
  async function openSearch(hit: Hit) {
    follow = false;
    jumping = true;
    if (chat.active?.id !== hit.thread.id) await chat.select(hit.thread);
    if (chat.active?.id !== hit.thread.id || chat.error) throw new Error(chat.error || 'Could not open this conversation.');
    const seen = new Set<string>();
    while (!chat.messages.some(message => message.id === hit.messageId) && chat.historyCursor) {
      if (seen.has(chat.historyCursor)) break;
      seen.add(chat.historyCursor);
      await chat.loadOlder();
      if (chat.active?.id !== hit.thread.id || chat.error) break;
    }
    if (chat.active?.id !== hit.thread.id || !chat.messages.some(message => message.id === hit.messageId)) {
      jumping = false;
      throw new Error(chat.error || 'This message is no longer available. Refresh search and try again.');
    }
    searchMatch = hit.messageId;
    await tick();
    const target = conversation?.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(hit.messageId)}"]`);
    target?.scrollIntoView({ block: 'center', behavior: 'instant' });
    target?.focus({ preventScroll: true });
    jumping = false;
    follow = false;
  }

  // Resumes following only when manual scrolling returns to the latest messages.
  function scrolled() {
    if (conversation && !jumping) follow = conversation.scrollHeight - conversation.scrollTop
      - conversation.clientHeight < 100;
  }
  // Where each turn's tool activity sits among the messages.
  const placement = $derived(chat.activityPlacement);
  const runs = $derived(chat.runPlacement);
  const runEntries = (id: string) => chat.activities.filter(entry => entry.runId === id);
</script>

{#snippet newChatRow()}
  <!-- The new conversation button is at the top of the sidebar at every width. On a narrow window
       the sidebar is a drawer, which closes so the new conversation is in view. -->
  <div class="px-4">
    <button class="new-chat" disabled={!chat.ready || chat.working || chat.switching}
      onclick={() => { navigationOpen = false; swarmOpen = false; void chat.newChat(); }}>
      <SquarePen size={17} aria-hidden="true" />
      New chat
    </button>
    <button class="mt-1 flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm
      text-muted-foreground hover:bg-foreground/5 hover:text-foreground disabled:opacity-50"
      aria-label="Search" title="Search (⌘K / Ctrl+K)"
      disabled={!chat.ready || chat.switching} onclick={showSearch}>
      <Search size={17} aria-hidden="true" />Search
    </button>
    <button class={['flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm hover:bg-foreground/5 hover:text-foreground',
      swarmOpen ? 'bg-foreground/5 text-foreground' : 'text-muted-foreground']} aria-pressed={swarmOpen}
      onclick={() => { navigationOpen = false; swarmTarget = null; if (swarmOpen) swarmView?.reset(); swarmOpen = true; }}>
      <Users size={17} aria-hidden="true" />{swarmOpen ? 'New swarm' : 'Swarm'}
    </button>
  </div>
{/snippet}

{#snippet settingsRow()}
  <!-- Settings sits at the foot of the sidebar. Opening it closes the phone drawer first. -->
  <div class="border-t border-border p-2">
    <button class="flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm text-muted-foreground
      hover:bg-foreground/5 hover:text-foreground" aria-label="Settings"
      onclick={() => { navigationOpen = false; settingsOpen = true; }}>
      <SettingsIcon size={16} aria-hidden="true" />Settings
    </button>
  </div>
{/snippet}

<Tooltip.Provider delayDuration={300}>
<IdleWarnings {chat} />
<Settings {chat} bind:open={settingsOpen} />
<ConversationSearch enabled={chat.ready && !chat.switching} bind:open={searchOpen}
  account={JSON.stringify(chat.account.providers ?? [chat.account])} onselect={openSearch} />
  <div class="shell" style:--navigation-width={navigationWidth === undefined ? undefined : `${navigationWidth}px`}>
    <aside
      class="relative hidden min-h-0 min-w-0 flex-col border-r border-border bg-muted md:flex"
      id="task-navigation" bind:offsetWidth={navigationActualWidth} aria-label="Task navigation"
    >
      <div class="flex h-16 items-center gap-2.5 px-5">
        <MessageSquare size={20} class="text-brand" aria-hidden="true" />
        <span class="text-[17px] font-semibold">MiniClaw</span>
      </div>

      {@render newChatRow()}

      <TaskNav {chat} onselect={() => swarmOpen = false} activeSwarm={swarmOpen ? swarmTarget : null}
        onswarm={id => { swarmTarget = id; swarmOpen = true; }} />

      {@render settingsRow()}
      <SidebarResize bind:width={navigationWidth} actual={navigationActualWidth} />
    </aside>

    <main class="flex min-h-0 min-w-0 flex-col">
      {#if swarmOpen}<SwarmView bind:this={swarmView} {chat} target={swarmTarget} onshow={id => swarmTarget = id} />{:else}
      <header
        class="flex min-h-14 items-center justify-between gap-2 border-b border-border
          px-2 py-1.5 sm:min-h-16 sm:gap-3 sm:px-6 sm:py-2"
      >
        <!-- One row at every width: on a phone the tools scroll sideways instead of wrapping. -->
        <div class="flex min-w-0 items-center gap-0.5 max-sm:overflow-x-auto [scrollbar-width:none] sm:gap-2">
          <Dialog.Root bind:open={navigationOpen}>
            <Dialog.Trigger class="icon-button md:hidden" aria-label="Open navigation">
              <PanelLeft size={18} aria-hidden="true" />
            </Dialog.Trigger>
            <Dialog.Portal>
              <Dialog.Overlay class="fixed inset-0 z-20 bg-black/30" />
              <Dialog.Content class="fixed inset-y-0 left-0 z-30 flex w-[min(300px,90vw)] flex-col border-r border-border bg-muted">
                <div class="flex h-16 items-center justify-between px-5">
                  <Dialog.Title class="font-semibold">MiniClaw</Dialog.Title>
                  <Dialog.Close class="icon-button" aria-label="Close navigation"><X size={18} aria-hidden="true" /></Dialog.Close>
                </div>
                {@render newChatRow()}
                <TaskNav {chat} onselect={() => { navigationOpen = false; swarmOpen = false; }} activeSwarm={swarmOpen ? swarmTarget : null}
                  onswarm={id => { navigationOpen = false; swarmTarget = id; swarmOpen = true; }} />
                {@render settingsRow()}
              </Dialog.Content>
            </Dialog.Portal>
          </Dialog.Root>
          <span class="text-lg font-semibold max-sm:hidden md:hidden">MiniClaw</span>
          <Choice label="Agent" value={chat.agentId || 'miniclaw'}
            placeholder={chat.active?.agentName ?? 'MiniClaw'}
            options={chat.active ? [{ value: chat.agentId || 'miniclaw',
              label: chat.active.agentName ?? 'MiniClaw' }] : [
              { value: 'miniclaw', label: 'MiniClaw' },
              ...chat.availableAgents.map(agent =>
                ({ value: agent.id, label: agent.name }))]}
            disabled={!chat.ready || !!chat.active || chat.busy || chat.agentsLoading}
            onchange={value => chat.chooseAgent(value === 'miniclaw' ? '' : value)} />
          {#if chat.active?.id}
            <Dialog.Root>
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <Dialog.Trigger {...props} class="icon-button"
                      aria-label="Conversation permissions" disabled={!chat.ready}>
                      <Shield size={18} aria-hidden="true" />
                    </Dialog.Trigger>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Portal><Tooltip.Content class="tooltip" sideOffset={8}>
                  Conversation permissions
                </Tooltip.Content></Tooltip.Portal>
              </Tooltip.Root>
              <Dialog.Portal>
                <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
                <Dialog.Content class="fixed left-1/2 top-1/2 z-50 flex max-h-[90dvh]
                  w-[min(560px,calc(100%-24px))] -translate-x-1/2 -translate-y-1/2
                  flex-col rounded-lg border border-border bg-background shadow-lg">
                  <div class="flex items-center justify-between border-b border-border px-5 py-3">
                    <Dialog.Title class="text-base font-semibold">Conversation permissions</Dialog.Title>
                    <Dialog.Close class="icon-button" aria-label="Close conversation permissions">
                      <X size={18} aria-hidden="true" />
                    </Dialog.Close>
                  </div>
                  <div class="min-h-0 overflow-y-auto p-5">
                    <PermissionSettings {chat} kind="conversation" id={chat.active.id} />
                  </div>
                </Dialog.Content>
              </Dialog.Portal>
            </Dialog.Root>
          {/if}
          <Notifications account={chat.account.status === 'signed_in' ? chat.account.email : undefined}
            onconversation={id => chat.select(chat.tasks.find(task => task.id === id)
              ?? { id, name: 'Conversation', model: null })} />
        </div>
        <div class="flex shrink-0 items-center justify-end gap-4">
          {#if chat.ready}
            <span class="text-xs capitalize text-muted-foreground max-md:hidden">{chat.account.planType}</span>
          {/if}
          <Connection />
        </div>
      </header>

      {#if chat.projectId}
        <div class="border-b border-border px-5 py-2 text-xs text-muted-foreground sm:px-8">
          <span class="block truncate">{chat.project?.name ?? chat.active?.projectName ?? 'Project unavailable'}</span>
        </div>
      {:else if chat.folder}
        <!-- A conversation with no project shows its own folder once it has one. -->
        <div role="note" aria-label="Conversation folder" class="border-b border-border px-5 py-2 text-xs text-muted-foreground sm:px-8">
          <span class="block truncate" title={chat.folder}>Folder <span class="font-mono">{chat.folder}</span></span>
        </div>
      {/if}

      {#if chat.tasks.length}
        <div class="border-b border-border px-5 py-2 md:hidden">
          <select aria-label="Chat" class="w-full bg-transparent text-sm"
            value={chat.active?.id ?? ''} disabled={chat.working || chat.switching}
            onchange={event => {
              const task = chat.tasks.find(task => task.id === event.currentTarget.value);
              if (task) void chat.select(task);
            }}>
            <option value="" disabled>Select chat</option>
            {#each chat.tasks as task (task.id)}
              <option value={task.id}>{task.name}</option>
            {/each}
          </select>
        </div>
      {/if}

      <!-- The VM panel is a full-height column beside the chat and its composer. -->
      <div class="flex min-h-0 flex-1 flex-col lg:flex-row">
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <div class="relative flex min-h-0 flex-1 flex-row">
      <ConversationRail messages={chat.messages} viewport={conversation}
        onjump={() => { follow = false; jumping = true; }} />
      <section
        class="conversation-scroll min-h-0 min-w-0 flex-1 overflow-y-auto py-6 sm:py-8"
        aria-label="Conversation"
        bind:this={conversation}
        onscroll={scrolled}
        onscrollend={() => { jumping = false; scrolled(); }}
        onwheel={() => jumping = false} ontouchstart={() => jumping = false}
      >
        {#if chat.historyCursor}
          <button class="mx-auto mb-6 flex items-center gap-2 text-sm text-muted-foreground
            disabled:opacity-50" disabled={chat.historyLoading || !chat.ready}
            aria-label="Load older messages" onclick={loadOlder}>
            <History size={16} aria-hidden="true" />
            {chat.historyLoading ? 'Loading...' : 'Load older messages'}
          </button>
        {/if}
        {#if chat.messages.length || placement.pending.length || runs.pending.length}
          <div class="mx-auto max-w-2xl space-y-8" data-conversation-content>
            {#each chat.messages as message (message.id)}
              <div data-message-id={message.id} data-search-match={searchMatch === message.id ? 'true' : undefined}
                tabindex="-1"
                class={searchMatch === message.id ? 'rounded-md ring-2 ring-brand/60 ring-offset-4 ring-offset-background' : ''}
                data-prompt-id={message.role === 'user' && !message.text.startsWith(NOTICE) ? message.id : undefined}>
                {#if placement.before.has(message.id)}<ActivityList entries={placement.before.get(message.id)!} />{/if}
                {#each runs.before.get(message.id) ?? [] as run (run.id)}<DelegationBlock {chat} {run} entries={runEntries(run.id)} />{/each}
                {#if message.role === 'assistant'}
                  <p class="mb-2 text-xs font-medium text-muted-foreground">
                    {chat.active?.agentName ?? (chat.claude ? 'Claude' : 'Codex')}
                  </p>
                {/if}
                {#if message.attachments?.length && chat.active}<MessageFiles thread={chat.active.id} files={message.attachments}
                  compact={message.role === 'assistant'} label={message.imageStatus ? 'Generated images' : 'Attached files'} />{/if}
                {#if message.imageStatus === 'generating'}<p role="status" class="text-sm text-muted-foreground">Generating image...</p>{/if}
                <!-- MiniClaw's own message, such as a finished agent's result carrying the parent on, is a line, not the user's words. -->
                {#if message.role === 'user' && message.text.startsWith(NOTICE)}
                  <p role="note" class="my-3 flex items-center justify-center gap-1.5 text-center text-xs text-muted-foreground">
                    <Users size={12} class="shrink-0" aria-hidden="true" />{message.text.slice(NOTICE.length).split('. The result is in')[0]}</p>
                {:else if message.role === 'assistant' || message.text.trim()}
                <article aria-label={message.role === 'assistant'
                  ? 'Assistant message' : 'Your message'}
                  class={['break-words text-base leading-7 [overflow-wrap:anywhere]',
                    message.role === 'user' && 'bubble-user whitespace-pre-wrap']}
                >{#if message.role === 'assistant'}<MessageMarkdown text={message.text} />{:else}{message.text}{/if}</article>
                {/if}
                <!-- A turn that ended without a reply keeps its steps and runs under its own message. -->
                {#if placement.after.has(message.id) || runs.after.has(message.id)}
                  <div class="mt-3">
                    {#if placement.after.has(message.id)}<ActivityList entries={placement.after.get(message.id)!} />{/if}
                    {#each runs.after.get(message.id) ?? [] as run (run.id)}<DelegationBlock {chat} {run} entries={runEntries(run.id)} />{/each}
                  </div>
                {/if}
              </div>
            {/each}
            {#each placement.pending as entries (entries[0].id)}<ActivityList {entries} />{/each}
            {#each runs.pending.flat() as run (run.id)}<DelegationBlock {chat} {run} entries={runEntries(run.id)} />{/each}
            {#if chat.responding}
              <p aria-live="polite" class="text-sm text-muted-foreground">
                {chat.reconnecting ? 'Reconnecting...' :
                  chat.uncertain ? 'Response status unknown' : 'Responding...'}
              </p>
            {/if}
          </div>
        {:else}
        <div class="flex min-h-full items-center justify-center">
        <div class="max-w-sm text-center">
          {#if chat.active?.vmOffline}
            <Monitor size={36} stroke-width={1.5} class="mx-auto mb-5 text-brand"
              aria-hidden="true" />
            {#if chat.vmStatus === 'starting' || chat.vmStatus === 'running'}
              <h1 class="text-2xl font-medium">{chat.vmName} is starting</h1>
              <p class="mt-3 text-sm text-muted-foreground">This conversation's history loads when it is ready.</p>
            {:else}
              <h1 class="text-2xl font-medium">{chat.vmName} is off</h1>
              <p class="mt-3 text-sm text-muted-foreground">History loads when {chat.vmName} starts.</p>
            {/if}
          {:else}
          <MessageSquare
            size={36}
            stroke-width={1.5}
            class="mx-auto mb-5 text-brand"
            aria-hidden="true"
          />
          <h1 class="text-2xl font-medium">No conversation yet</h1>
          <p class="mt-3 text-sm text-muted-foreground">
            {chat.ready ? 'Ready when you are' : chat.account.status === 'checking'
              ? 'Connecting' : 'No provider connected'}
          </p>
          {#if chat.account.status === 'signed_out'}
            {#if chat.login.status === 'pending' && chat.login.provider === 'claude'}
              <div class="mt-5"><ClaudeSignIn {chat} /></div>
            {:else if chat.login.status === 'pending'}
              <p class="mt-5 text-sm text-muted-foreground">Waiting for sign-in</p>
              {#if chat.login.authUrl || chat.login.verificationUrl}
                <a class="mt-3 inline-block text-sm text-brand underline"
                  href={chat.login.authUrl ?? chat.login.verificationUrl}
                  target="_blank" rel="noopener noreferrer">Continue sign-in</a>
              {/if}
              {#if chat.login.userCode}
                <p class="mt-3 font-mono text-lg">{chat.login.userCode}</p>
              {/if}
              <div class="mt-3 flex justify-center gap-4 text-sm">
                <button onclick={() => chat.refresh()}>Check sign-in</button>
                <button onclick={() => chat.cancelLogin()}>Cancel</button>
              </div>
            {:else}
              {#if !chat.providers.length || chat.providers.some(provider =>
                provider.id === 'codex' && provider.status === 'signed_out')}
                <button class="mx-auto mt-6 flex items-center gap-2 rounded-md
                  bg-primary px-4 py-3 text-sm text-primary-foreground" aria-label="Sign in"
                  onclick={() => chat.signIn()}>
                  <LogIn size={16} aria-hidden="true" />Sign in with ChatGPT
                </button>
                <button class="mt-4 text-xs text-muted-foreground underline"
                  onclick={() => chat.signIn('device')}>Use device code</button>
              {/if}
              {#if chat.providers.some(provider =>
                provider.id === 'claude' && provider.status === 'signed_out')}
                <button class="mx-auto mt-6 flex items-center gap-2 rounded-md
                  bg-primary px-4 py-3 text-sm text-primary-foreground" aria-label="Sign in with Claude"
                  onclick={() => chat.signInClaude()}>
                  <LogIn size={16} aria-hidden="true" />Sign in with Claude
                </button>
              {/if}
            {/if}
          {:else if chat.account.status === 'unavailable'}
            <button class="mx-auto mt-5 flex items-center gap-2 text-sm text-brand"
              onclick={() => chat.refresh()}>
              <RefreshCw size={15} aria-hidden="true" />Retry connection
            </button>
          {:else if chat.account.status === 'unsupported_auth'}
            <p class="mt-4 text-sm text-muted-foreground">A ChatGPT subscription sign-in is required.</p>
          {/if}
          {/if}
        </div>
        </div>
        {/if}
      </section>
      </div>

      <footer class="mx-auto w-full max-w-3xl px-5 pb-5 sm:px-8 sm:pb-7">
        {#if chat.computerActivity.length}
          <!-- A failure opens the list by itself: its reason is what the user needs to see. -->
          <details class="mb-3 text-xs text-muted-foreground" open={chat.computerActivity.some(activity => activity.error)}>
            <summary class="cursor-pointer py-1">Computer activity</summary>
            <ul class="max-h-32 space-y-1 overflow-y-auto py-2">
              {#each chat.computerActivity as activity (activity.id)}
                <li class="flex flex-wrap justify-between gap-x-3 [overflow-wrap:anywhere]">
                  <span>{activity.tool.replaceAll('_', ' ')}{activity.app ? `: ${activity.app}` : ''}</span>
                  <span>{activity.status === 'inProgress' ? 'Running' : activity.status}</span>
                  {#if activity.error}
                    <p class="basis-full text-destructive">{activity.error}</p>
                    {#if activity.hint}<p class="basis-full text-foreground">{activity.hint}</p>{/if}
                  {/if}
                </li>
              {/each}
            </ul>
          </details>
        {/if}
        {#if chat.pendingApproval}
          <section aria-label="Computer approval" class="mb-3 rounded-lg border border-border bg-background p-3">
            {#if chat.pendingApproval.agent}<p class="mb-2 text-xs text-muted-foreground">Requested by {chat.pendingApproval.agent}</p>{/if}
            <p class="max-h-32 overflow-y-auto text-sm font-medium [overflow-wrap:anywhere]">
              {chat.pendingApproval.message}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">Screenshots from approved apps are sent to Codex.</p>
            {#if chat.pendingApproval.warning}<p role="note" class="mt-2 rounded-md border border-border bg-muted px-2 py-1.5 text-xs">{chat.pendingApproval.warning}</p>{/if}
            <div class="mt-3 flex flex-wrap gap-3">
              <Button variant="secondary"
                disabled={!!chat.deciding} onclick={() => chat.decide('decline')}>
                <X size={16} aria-hidden="true" />Decline
              </Button>
              <Button
                disabled={!!chat.deciding} onclick={() => chat.decide('accept')}>
                <Check size={16} aria-hidden="true" />Allow once
              </Button>
            </div>
          </section>
        {/if}
        {#if chat.pendingReview}<ReviewDialog {chat} />{/if}
        {#if chat.pendingDelegation}
          <AlertDialog.Root open={true}>
            <AlertDialog.Portal>
              <AlertDialog.Overlay class="fixed inset-0 z-[80] bg-black/45" />
              <AlertDialog.Content escapeKeydownBehavior="ignore" interactOutsideBehavior="ignore"
                class="fixed left-1/2 top-1/2 z-[90] w-[min(32rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2
                  rounded-xl border border-border bg-background p-5 shadow-xl sm:p-6">
                <AlertDialog.Title class="text-base font-semibold">Delegate to {chat.pendingDelegation.agent}?</AlertDialog.Title>
                <AlertDialog.Description class="mt-2 text-sm text-muted-foreground">
                  The agent in “{chat.pendingDelegation.threadName}” wants {chat.pendingDelegation.agent} to do a task. Nothing starts until you choose.
                  With no answer in {chat.pendingDelegation.expiresInSeconds} s it is declined.
                </AlertDialog.Description>
                {#if chat.pendingDelegation.summary}<p class="mt-4 text-sm font-medium [overflow-wrap:anywhere]">{chat.pendingDelegation.summary}</p>{/if}
                <p class="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap rounded-md border border-border p-3 text-sm
                  [overflow-wrap:anywhere]">{chat.pendingDelegation.task}</p>
                {#if chat.pendingDelegation.minutes && chat.pendingDelegation.waitMinutes}
                  <p class="mt-3 text-xs text-muted-foreground">{chat.pendingDelegation.waitMinutes < chat.pendingDelegation.minutes
                    ? `In the background it may take up to ${duration(chat.pendingDelegation.minutes)}. If you wait, after ${duration(chat.pendingDelegation.waitMinutes)} it continues in the background.`
                    : `It may take up to ${duration(chat.pendingDelegation.minutes)}.`}</p>
                {/if}
                {#if chat.pendingDelegation.attachments?.length}
                  <p class="mt-3 text-xs text-muted-foreground">It hands over {chat.pendingDelegation.attachments.length === 1 ? 'this file' : 'these files'}:</p>
                  <div class="mt-2"><MessageFiles thread={chat.pendingDelegation.thread} files={chat.pendingDelegation.attachments} compact label="Files handed over" /></div>
                {/if}
                <div class="mt-5 flex flex-wrap justify-end gap-2">
                  <Button disabled={chat.delegationDeciding} variant="outline" onclick={() => chat.answerDelegation('reject')}>Decline</Button>
                  <Button disabled={chat.delegationDeciding} variant="secondary" title="This conversation carries on. The answer appears later, under the agent's name."
                    onclick={() => chat.answerDelegation('background')}>Run in background</Button>
                  <Button disabled={chat.delegationDeciding} title="This conversation waits for the answer and then carries on with it."
                    onclick={() => chat.answerDelegation('wait')}>Wait for it</Button>
                </div>
              </AlertDialog.Content>
            </AlertDialog.Portal>
          </AlertDialog.Root>
        {/if}
        {#if chat.pendingQuestion && !chat.pendingScopedApproval}
          {#key chat.pendingQuestion.id}<QuestionDialog {chat} question={chat.pendingQuestion} />{/key}
        {/if}
        {#if chat.pendingScopedApproval}
          <Dialog.Root open={true}>
            <Dialog.Portal>
              <Dialog.Overlay class="fixed inset-0 z-50 bg-black/45" />
              <Dialog.Content
                aria-label="Scoped approval"
                escapeKeydownBehavior="ignore"
                interactOutsideBehavior="ignore"
                class="fixed left-1/2 top-1/2 z-[60] w-[min(34rem,calc(100vw-2rem))]
                  -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border
                  bg-muted p-5 shadow-xl sm:p-6">
                <Dialog.Title class="text-base font-semibold">Approve agent access</Dialog.Title>
                <Dialog.Description class="mt-2 text-sm text-muted-foreground">
                  {#if chat.pendingScopedApproval.agent}
                    The delegated agent <span class="font-medium text-foreground">{chat.pendingScopedApproval.agent}</span> requested
                    one exact action. It uses its own permissions, not this conversation's.
                  {:else}This agent requested one exact action. Check the resource before approving.{/if}
                </Dialog.Description>
                <div class="mt-4 max-h-64 overflow-y-auto rounded-md border border-border p-3 text-sm">
                  {#if chat.pendingScopedApproval.target.target_type === 'application'}
                    <p>App: {chat.pendingScopedApproval.target.id}</p>
                    <p>Action: {chat.pendingScopedApproval.target.action}</p>
                  {:else if chat.pendingScopedApproval.target.target_type === 'file'
                    || chat.pendingScopedApproval.target.target_type === 'folder'}
                    <p>{chat.pendingScopedApproval.target.target_type === 'folder' ? 'Folder' : 'File'}:
                      <span class="break-all">{chat.pendingScopedApproval.target.path}</span></p>
                    <p>Action: {chat.pendingScopedApproval.target.action}</p>
                  {:else if chat.pendingScopedApproval.target.target_type === 'host'}
                    <p>Host: <span class="break-all">{chat.pendingScopedApproval.target.host}</span></p>
                    <p>A shell command may reach it over HTTPS only, port 443.</p>
                  {:else if chat.pendingScopedApproval.target.target_type === 'command'}
                    <p>{chat.pendingScopedApproval.target.kind === 'shell' ? 'Shell' : 'AppleScript'} command</p>
                    <pre class="mt-2 whitespace-pre-wrap break-all font-mono text-xs">{chat.pendingScopedApproval.target.text}</pre>
                    {#if chat.pendingScopedApproval.unrestricted}
                      <p role="alert" class="mt-3 text-xs text-destructive">This command runs outside MiniClaw’s sandbox with {words.account}’s file and network access. It stops when your timed grant expires.</p>
                    {/if}
                    <p class="mt-2 break-all">Working directory: {chat.pendingScopedApproval.target.cwd}</p>
                  {:else if chat.pendingScopedApproval.target.target_type === 'mcp_tool'}
                    <p>MCP server: {chat.pendingScopedApproval.serverName ?? chat.pendingScopedApproval.target.server}</p>
                    <p>Tool: {chat.pendingScopedApproval.target.tool}</p>
                    <p class="mt-2 text-xs text-muted-foreground">It will be sent exactly this:</p>
                    <pre class="mt-1 whitespace-pre-wrap break-all font-mono text-xs">{JSON.stringify(chat.pendingScopedApproval.arguments ?? {}, null, 2)}</pre>
                    {#if chat.pendingScopedApproval.unrestricted}
                      <p role="alert" class="mt-3 text-xs text-destructive">This server runs outside the sandbox. It can read and
                        change anything you can.</p>
                    {:else if chat.pendingScopedApproval.remote}
                      <p class="mt-3 text-xs text-muted-foreground">This is a remote server. What is sent leaves {words.machine}.</p>
                    {/if}
                  {/if}
                  {#if chat.pendingScopedApproval.hosts?.length}
                    <p class="mt-3 font-medium">Network it asks for, HTTPS only:</p>
                    <ul aria-label="Hosts this command may reach" class="mt-1 list-disc pl-5">
                      {#each chat.pendingScopedApproval.hosts as host (host)}<li class="break-all">{host}</li>{/each}
                    </ul>
                    <p class="mt-2 text-xs text-muted-foreground">"Session, whole domains" would cover
                      {chat.pendingScopedApproval.domains?.join(', ')} and everything below them.</p>
                  {/if}
                </div>
                <p class="mt-3 text-xs text-muted-foreground">{chat.pendingScopedApproval.target.target_type === 'mcp_tool'
                  ? 'Session approval lasts eight hours in this conversation and covers this tool with any input. A coding session asks every time.'
                  : 'Session approval lasts eight hours in this conversation. A changed command or working directory needs another approval.'}</p>
                <div class="mt-5 flex flex-wrap justify-end gap-2">
                  <Button variant="secondary"
                    disabled={!!chat.deciding} onclick={() => chat.decideScoped('reject')}>Reject</Button>
                  <Button variant="secondary"
                    disabled={!!chat.deciding} onclick={() => chat.decideScoped('approve_once')}>Approve Once</Button>
                  {#if chat.pendingScopedApproval.hosts?.length}
                    <Button variant="secondary" disabled={!!chat.deciding}
                      onclick={() => chat.decideScoped('approve_session_domains')}>Session, whole domains</Button>
                  {/if}
                  <!-- A plain command line names its programs; allowing them is a lasting rule on the agent. -->
                  {#if chat.pendingScopedApproval.programs?.length}
                    <Button variant="secondary" disabled={!!chat.deciding}
                      onclick={() => chat.decideScoped('approve_programs')}>Always allow {chat.pendingScopedApproval.programs.join(', ')}</Button>
                  {/if}
                  <Button
                    disabled={!!chat.deciding} onclick={() => chat.decideScoped('approve_session')}>Approve for this session</Button>
                </div>
              </Dialog.Content>
            </Dialog.Portal>
          </Dialog.Root>
        {/if}
        {#if chat.error || chat.computerError || chat.imageError}
          <div class="mb-3 flex items-center gap-2 text-sm text-destructive">
            <p role="alert" class="min-w-0 break-words">{chat.imageError || chat.computerError || chat.error}</p>
            {#if chat.imageError}
              <button class="icon-button" aria-label="Retry image generation setting"
                disabled={chat.switching || chat.imageChanging || !chat.ready}
                onclick={() => void chat.retryImage()}>
                <RefreshCw size={16} aria-hidden="true" />
              </button>
            {/if}
            {#if chat.computerError}
              <button class="icon-button" aria-label="Retry Computer Use"
                disabled={chat.switching || chat.computerPending || !chat.ready}
                onclick={() => chat.setComputer(!chat.computerUse)}>
                <RefreshCw size={16} aria-hidden="true" />
              </button>
            {/if}
          </div>
        {/if}
        {#if chat.agentId && !chat.agentReady}
          <p role="alert" class="mb-3 text-sm text-destructive">
            {chat.agentError || (!chat.projectReady ? 'This agent is not available in this project.' : 'This agent is inactive or deleted.')}
          </p>
        {/if}
        {#if chat.projectId && !chat.agentId && !chat.projectReady}
          <p role="alert" class="mb-3 text-sm text-destructive">Choose this project's machine in Project settings before sending.</p>
        {/if}
        {#if chat.active && !chat.machineReady}
          <p class="mb-3 text-sm text-muted-foreground">Choose this conversation's machine before sending.</p>
        {/if}
        {#if chat.vmApproval || chat.vmStarting}
          <!-- The start prompt below already says the VM is off. -->
        {:else if chat.vmStatus === 'off' && !chat.active?.vmOffline}
          <p class="mb-3 text-xs text-muted-foreground">{chat.vmName} is off</p>
        {:else if chat.vmReason}
          <p role="status" class="mb-3 text-xs text-destructive">{chat.vmReason}</p>
        {:else if chat.machine === 'vm' && chat.vmStatus === 'starting' && !chat.vmStarting}
          <p role="status" class="mb-3 text-sm text-muted-foreground">Starting {chat.vmName}</p>
        {/if}
        {#if chat.vmApproval}
          <div role="group" aria-label={`Start ${chat.vmName}`}
            class="mb-3 flex flex-wrap items-center gap-3 rounded-lg border border-border bg-background p-3">
            <span class="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
              <Monitor size={18} aria-hidden="true" />
            </span>
            <div class="min-w-0 flex-1 basis-48">
              <p class="text-sm font-medium">Start {chat.vmName}?</p>
              <p class="text-xs text-muted-foreground">It is off. It starts in the background, which takes about a minute.
                Your message is sent as soon as it is ready.</p>
            </div>
            <div class="flex gap-2">
              <Button variant="secondary" onclick={() => chat.declineVmStart()}>Cancel</Button>
              <Button disabled={chat.vmStarting}
                onclick={() => void chat.approveVmStart()}>
                <Power size={15} aria-hidden="true" />Start {chat.vmName} and send
              </Button>
            </div>
          </div>
        {/if}
        {#if chat.vmStarting}
          <p role="status" class="mb-3 flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle size={15} class="animate-spin" aria-hidden="true" />Starting {chat.vmName}
          </p>
        {/if}
        {#if chat.ready && (chat.modelError || !chat.validChoice)}
          <div class="mb-2 flex flex-wrap items-center gap-2 text-xs text-destructive">
            <span>{chat.modelError || 'Selected model or effort is unavailable'}</span>
            <button aria-label="Retry models" class="icon-button"
              disabled={chat.modelsLoading} onclick={() => chat.refreshModels()}>
              <RefreshCw size={15} aria-hidden="true" />
            </button>
          </div>
        {/if}
        <MessageQueue {chat} />
        <!-- Files can be dropped anywhere on the composer or pasted into the message. -->
        <div class={['composer relative', dropping && 'ring-2 ring-ring']} role="group" aria-label="Message composer"
          ondragover={event => { if (event.dataTransfer?.types.includes('Files')) { event.preventDefault(); dropping = true; } }}
          ondragleave={() => dropping = false}
          ondrop={event => { dropping = false; if (event.dataTransfer?.files.length) { event.preventDefault(); chat.addFiles(event.dataTransfer.files); } }}>
          <DraftFiles {chat} />
          <SlashMenu bind:this={slashMenu} {chat} bind:text={chat.draft} />
          <textarea
            aria-label="Message"
            aria-controls={slashMenu?.controls()}
            aria-activedescendant={slashMenu?.activeOption()}
            placeholder={chat.queueable ? 'Queue a message for after this response' : chat.claude ? 'Message Claude' : 'Message Codex'}
            rows="1"
            bind:value={chat.draft}
            onkeydown={keydown}
            onpaste={event => { if (event.clipboardData?.files.length) { event.preventDefault(); chat.addFiles(event.clipboardData.files); } }}
            disabled={!chat.ready || (chat.responding && !chat.queueable)}
            class="composer-input placeholder:text-muted-foreground focus:outline-none"
          ></textarea>
          <div class="mt-2 flex items-center gap-2">
            <div class="flex min-w-0 items-center gap-0.5">
              <Choice label="Machine" value={chat.machine}
                options={[{ value: 'host', label: 'Mac' },
                  { value: 'vm', label: chat.vmName }]}
                placeholder="Choose machine"
                disabled={!chat.ready || chat.busy || (!!chat.active?.environment)}
                onchange={value => void chat.chooseMachine(value)} />
              <Choice label="Model" value={chat.model}
                options={chat.modelChoices.map(model => ({ value: model.model, label: model.displayName }))}
                placeholder={chat.modelsLoading ? 'Loading models...' : chat.model || 'Default model'}
                disabled={!chat.ready || chat.busy || !!chat.modelError || !chat.models.length}
                onchange={value => chat.setModel(value)} />
              <div class="shrink-0">
                <Choice label="Reasoning effort" value={chat.effort}
                  options={(chat.selectedModel?.supportedReasoningEfforts ?? [])
                    .map(effort => ({ value: effort.reasoningEffort, label: effort.reasoningEffort }))}
                  placeholder="Default effort"
                  disabled={!chat.ready || chat.busy || !!chat.modelError ||
                    !chat.selectedModel?.supportedReasoningEfforts.length}
                  onchange={value => chat.setEffort(value)} />
              </div>
              <!-- Codex's speed tiers, in Codex's own words, since Fast uses more of the plan. The closed control shows the tier's name alone. -->
              <Choice label="Speed" value={chat.speed || 'standard'}
                options={[{ value: 'standard', label: 'Standard' }, ...chat.speedChoices
                  .map(tier => ({ value: tier.id, name: tier.name, label: tier.description ? `${tier.name}: ${tier.description}` : tier.name }))]}
                placeholder="Standard" icon={Zap} quiet={!chat.speed}
                disabled={!chat.ready || chat.busy || !!chat.modelError || !chat.speedChoices.length}
                onchange={value => chat.setSpeed(value === 'standard' ? '' : value)} />
              <Capabilities {chat} />
              {#key chat.active?.id ?? 'new'}<ConversationSkills {chat} />{/key}
              <AttachButton {chat} disabled={!chat.ready || chat.working || (chat.responding && !chat.queueable)} />
            </div>
            <!-- With words in the box during a response, the main button queues them and Stop moves beside it. -->
            {#if chat.turn && chat.queueable && (chat.draft.trim() || chat.files.length)}
              <Tooltip.Root>
                <Tooltip.Trigger class="icon-button ml-auto shrink-0" aria-label="Stop" onclick={() => chat.stop()}>
                  <Square size={15} aria-hidden="true" />
                </Tooltip.Trigger>
                <Tooltip.Portal><Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>Stop response</Tooltip.Content></Tooltip.Portal>
              </Tooltip.Root>
              <Tooltip.Root>
                <Tooltip.Trigger class="send-button shrink-0" aria-label="Queue message"
                  disabled={!chat.validChoice} onclick={() => chat.enqueue()}>
                  <ListPlus size={18} aria-hidden="true" />
                </Tooltip.Trigger>
                <Tooltip.Portal><Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>Queue for after this response</Tooltip.Content></Tooltip.Portal>
              </Tooltip.Root>
            {:else}
            <Tooltip.Root>
              <Tooltip.Trigger class="send-button ml-auto shrink-0"
                aria-label={chat.turn ? 'Stop' : 'Send'}
                disabled={chat.turn ? false : !chat.ready || !chat.agentReady || !chat.machineReady || chat.busy ||
                  (!chat.draft.trim() && !chat.files.length) || !chat.validChoice || chat.usage.ordinaryUsageAllowed === false}
                onclick={() => chat.turn ? chat.stop() : chat.send()}>
                {#if chat.turn}<Square size={16} aria-hidden="true" />
                {:else}<ArrowUp size={19} aria-hidden="true" />{/if}
              </Tooltip.Trigger>
              <Tooltip.Portal>
                <Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>
                  {chat.turn ? 'Stop response' : 'Send message'}
                </Tooltip.Content>
              </Tooltip.Portal>
            </Tooltip.Root>
            {/if}
          </div>
        </div>
        <section
          aria-label={chat.claude ? 'Claude usage' : 'Codex usage'}
          class="mt-3 flex min-h-5 flex-wrap justify-between gap-2 text-xs text-muted-foreground"
        >
          <span class="flex items-center gap-2">{chat.claude ? 'Claude Code' : 'Codex'}
            <!-- Loud on purpose: while it is on, nothing the agent is allowed will ask. -->
            {#if chat.fullAccess}<span role="status" class="rounded-full bg-destructive px-2 py-0.5 font-medium text-primary-foreground">Full access</span>{/if}
            {#if chat.speedName}<span role="status" class="rounded-full bg-muted px-2 py-0.5 font-medium text-foreground">{chat.speedName}</span>{/if}
          </span>
          {#if chat.usage.ordinaryUsageAllowed === false}
            <span class="text-destructive">Usage limit reached</span>
          {:else if !usageRows(chat.usage).length}
            <span>Usage unavailable</span>
          {:else}
            <div class="flex flex-wrap justify-end gap-x-4 gap-y-2">
              {#each usageRows(chat.usage) as window (window.id)}
                <span title={resetTime(window.resetsAt)}>
                  {window.label}: {window.usedPercent ?? '?'}% used
                </span>
              {/each}
              {#if chat.usage.status === 'stale'}<span>Out of date</span>{/if}
            </div>
          {/if}
        </section>
      </footer>
      </div>
      {#if chat.machine === 'vm' || chat.active}<WorkspacePanel {chat} />{/if}
      </div>
      {/if}
    </main>
  </div>
</Tooltip.Provider>
