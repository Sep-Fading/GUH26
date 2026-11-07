<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // One compact dropdown for everything a conversation can switch on or off.
  import { Popover, Tooltip } from 'bits-ui';
  import { Switch } from '$lib/components/ui/switch/index.js';
  // Identifies each capability and the trigger.
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import Globe from '@lucide/svelte/icons/globe';
  import Network from '@lucide/svelte/icons/network';
  import Telescope from '@lucide/svelte/icons/telescope';
  import Users from '@lucide/svelte/icons/users';
  import ClipboardList from '@lucide/svelte/icons/clipboard-list';
  import Cable from '@lucide/svelte/icons/cable';
  import Monitor from '@lucide/svelte/icons/monitor';
  import ListChecks from '@lucide/svelte/icons/list-checks';
  import TerminalSquare from '@lucide/svelte/icons/square-terminal';
  import ShieldOff from '@lucide/svelte/icons/shield-off';
  import Brain from '@lucide/svelte/icons/brain';
  import MessageCircleQuestion from '@lucide/svelte/icons/message-circle-question-mark';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  // Reads the capabilities and switches them through the backend.
  import type { Chat } from './chat.svelte';

  type Capability = { key: string; name: string; allowed: boolean; on: boolean; expiresAt?: number; expired?: boolean };
  type Row = Capability & { icon: typeof Globe; note: string; disabled: boolean; why: string };

  let { chat }: { chat: Chat } = $props();
  let open = $state(false), listed = $state<Capability[]>([]), busy = $state(false);

  let shellMinutes = $state(15), now = $state(Date.now());
  const shell = $derived(listed.find(item => item.key === 'unsandboxed_shell'));

  const icons: Record<string, typeof Globe> = { web_search: Globe, network: Network, unsandboxed_shell: ShieldOff,
    web_fetch: Telescope, subagents: Users, task_tracking: ClipboardList,
    plan_mode: ListChecks, coding_session: GitPullRequestArrow, questions: MessageCircleQuestion, project_memory: Brain,
    ask_commands: TerminalSquare, full_access: ShieldOff };
  const notes: Record<string, string> = $derived({
    web_search: `Searches the web at the provider, not on ${words.machine}`,
    unsandboxed_shell: 'Runs as you, with access to files, credentials and network outside the sandbox',
    network: 'Commands may ask for HTTPS to hosts you approve',
    web_fetch: 'Claude: read web pages from approved sites',
    subagents: 'Claude: delegate work to restricted helper agents',
    task_tracking: 'Claude: create and update a task checklist',
    plan_mode: 'Claude: plans and explains, does not act',
    coding_session: 'You review every file change before it is written',
    questions: 'The agent may ask you, with options and your own answer',
    project_memory: 'Reads and saves notes shared by this project\'s conversations',
    ask_commands: 'Off: read-only commands such as ls, cat and git status run without asking',
    full_access: 'Nothing the agent is allowed asks. With Coding session, only file changes do',
  });
  const who = $derived(chat.agentId ? 'this agent' : 'the main agent');

  // The conversation, agent or project the list belongs to; a change reloads it.
  const scope = $derived(`${chat.active?.id ?? ''}|${chat.agentId}|${chat.projectId}|${chat.claude}|${chat.machine}|${chat.ready}`);
  let request = 0;
  // Reads the chosen scope; ignores responses for an older selection or account.
  async function load() {
    const current = ++request;
    const selected = scope;
    if (!chat.ready) { listed = []; return; }
    // Anything other than a list reads as nothing allowed; the rows below never see an undefined list.
    try {
      const answer = await chat.capabilities<{ capabilities?: Capability[] }>();
      if (current === request && selected === scope) {
        listed = Array.isArray(answer?.capabilities) ? answer.capabilities : [];
        if (!chat.active) chat.fullAccess = chat.draftCapabilities.full_access
          ?? listed.find(item => item.key === 'full_access')?.on ?? false;
      }
    } catch { if (current === request && selected === scope) listed = []; }
  }
  $effect(() => { void scope; void load(); });
  $effect(() => { if (open) void load(); });

  // Refreshes expiry while the popup is closed; backend time remains authoritative.
  $effect(() => {
    if (!chat.ready || !chat.active) return;
    const timer = setInterval(() => { now = Date.now(); void load(); }, 1000);
    return () => clearInterval(timer);
  });

  // Why a row cannot be switched right now, in the words of its tooltip; empty when it can.
  function blocked(item: Capability) {
    if (item.key === 'unsandboxed_shell' && !chat.active) return 'Start a conversation before granting timed access';
    if (!item.allowed) return `Not available for ${who}. Allow it in Settings, Agents.`;
    if (['web_fetch', 'subagents', 'task_tracking', 'plan_mode'].includes(item.key) && !chat.claude) return 'Claude conversations only';
    // A switch flips during a response too: the backend reads it at the next tool call.
    if (chat.working || chat.pending || chat.switching) return 'Wait for the settings to finish saving';
    return '';
  }

  const computerWhy = $derived(chat.agentId ? 'Not available for this agent.' : chat.claude ? 'Not available with Claude'
    : !chat.computerAvailable && !chat.computerUse ? (chat.machine === 'vm'
      ? `Not installed in ${chat.vmName}. Run: miniclaw update-vm --computer-use`
      // Windows finds the helper chosen at setup in the installed Codex app; the backend says why it cannot.
      : words.windows ? chat.hostComputerReason || 'The Computer Use helper is not set up. Run miniclaw setup again; miniclaw guide computer-use shows where it is.'
        : 'The Computer Use helper is unavailable')
    : chat.responding || chat.switching || chat.computerPending ? 'Wait for the response to end' : '');

  // Plan mode and the coding session need nobody's permission, so they are offered even when the list could not be read.
  const OWN = ['questions', 'plan_mode', 'coding_session', 'ask_commands', 'full_access'];
  const names: Record<string, string> = { questions: 'Ask me questions', plan_mode: 'Plan mode', coding_session: 'Coding session',
    ask_commands: 'Ask about every command', full_access: 'Full access' };
  const restrictions = $derived<Capability[]>(OWN.map(key => listed.find(item => item.key === key)
    ?? { key, name: names[key], allowed: true, on: key === 'questions' }));
  // Draft choices take precedence over defaults until the backend creates the conversation.
  function chosen(item: Capability) {
    return !chat.active && item.allowed ? chat.draftCapabilities[item.key] ?? item.on : item.on;
  }
  const rows = $derived<Row[]>([
    ...listed.filter(item => !OWN.includes(item.key)).map(item => ({ ...item,
      on: chosen(item),
      icon: item.key.startsWith('mcp:') ? Cable : icons[item.key] ?? SlidersHorizontal,
      note: item.key.startsWith('mcp:') ? 'MCP server. Each tool asks before it runs' : notes[item.key] ?? '',
      why: blocked(item), disabled: !!blocked(item) })),
    { key: 'computer_use', name: 'Computer Use', allowed: !chat.agentId && !chat.claude, on: chat.computerUse, icon: Monitor,
      note: 'Sees and controls apps on this machine', why: computerWhy, disabled: !chat.ready || !!computerWhy },
    // The coding session also changes from outside this dropdown, so its state comes from the conversation.
    ...restrictions.map(item => ({ ...item, on: item.key === 'coding_session' ? chat.codingSession : item.key === 'full_access' ? chat.fullAccess : chosen(item), icon: icons[item.key],
      note: notes[item.key], why: blocked(item), disabled: !!blocked(item) })),
  ]);
  const active = $derived(rows.filter(row => row.on).length);

  // Applies a switch and reloads saved state only after a conversation exists.
  async function set(row: Row, on: boolean) {
    if (row.disabled || busy) return;
    if (row.key === 'unsandboxed_shell' && on && (!Number.isInteger(shellMinutes) || shellMinutes < 1 || shellMinutes > 1440)) return;
    busy = true;
    try {
      if (row.key === 'computer_use') await chat.setComputer(on);
      else if (await chat.toggleCapability(row.key, on,
        row.key === 'unsandboxed_shell' && on ? shellMinutes * 60 : undefined) && chat.active) await load();
    } finally { busy = false; }
  }
</script>

{#if shell?.expired}
  <div role="alert" class="text-xs text-destructive">
    Unsandboxed shell access expired and is off.
    <button type="button" class="underline" onclick={() => open = true}>Review and renew access</button>
    <button type="button" class="ml-2 underline" onclick={async () => {
      if (await chat.toggleCapability('unsandboxed_shell', false)) await load();
    }}>Dismiss</button>
  </div>
{/if}
<Popover.Root bind:open>
  <Popover.Trigger aria-label="Tools" disabled={!chat.ready}
    class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-xs text-muted-foreground hover:bg-muted hover:text-foreground
      disabled:opacity-40 data-[state=open]:bg-muted data-[state=open]:text-foreground">
    <SlidersHorizontal size={16} aria-hidden="true" /><span>Tools</span>
    {#if active}<span class="rounded-full bg-muted px-1.5 text-[0.6875rem] tabular-nums text-foreground" aria-label={`${active} on`}>{active}</span>{/if}
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content side="top" align="start" sideOffset={8} role="dialog" aria-label="Tools for this conversation"
      class="z-50 w-[min(22rem,calc(100vw-2rem))] rounded-xl border border-border bg-background p-1.5 shadow-lg">
      <!-- Ten rows fit at both tested sizes; on a shorter window the list scrolls. A fade was tried
           and dimmed the last row even when nothing scrolled, so the edge stays plain. -->
      <ul class="max-h-[min(46rem,72dvh)] overflow-y-auto [overscroll-behavior:contain]">
        {#each rows as row (row.key)}
          {@const Icon = row.icon}
          <li>
            <Tooltip.Root disabled={!row.why}>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <!-- A row that cannot be switched stays focusable, so its reason can be read by keyboard too. -->
                  <div {...props} class={`flex items-center gap-3 rounded-lg px-2.5 py-2 ${row.disabled ? 'opacity-45' : 'hover:bg-muted'}`}
                    aria-disabled={row.disabled}>
                    <Icon size={17} class="shrink-0 text-muted-foreground" aria-hidden="true" />
                    <div class="min-w-0 flex-1">
                      <p class="truncate text-sm font-medium">{row.name}</p>
                      <p class="line-clamp-2 text-xs text-muted-foreground">{row.note}</p>
                      {#if row.key === 'unsandboxed_shell'}
                        {#if row.on && row.expiresAt}
                          <p class="text-xs tabular-nums" role="status">Expires in {Math.max(0, Math.ceil((row.expiresAt - now) / 1000))} seconds</p>
                        {:else}
                          <label class="mt-1 block text-xs">Access duration (minutes)
                            <input type="number" min="1" max="1440" step="1" bind:value={shellMinutes}
                              class="ml-2 w-16 rounded border bg-background px-1" aria-label="Shell access duration in minutes" />
                          </label>
                        {/if}
                      {/if}
                      {#if row.why}<p id={`why-${row.key}`} class="sr-only">{row.why}</p>{/if}
                    </div>
                    <Switch class="data-[state=checked]:bg-success" aria-label={row.name} aria-describedby={row.why ? `why-${row.key}` : undefined}
                      checked={row.on} disabled={row.disabled || busy} onCheckedChange={on => set(row, on)} />
                  </div>
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Portal><Tooltip.Content role="tooltip" class="tooltip z-[70]" side="left" sideOffset={8}>{row.why}</Tooltip.Content></Tooltip.Portal>
            </Tooltip.Root>
          </li>
        {/each}
      </ul>
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>
