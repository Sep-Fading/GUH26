<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Styled text fields from the shared component library.
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  // Shares the existing request and chat state.
  import type { Chat, McpServer, PermissionSettings, PermissionTarget } from './chat.svelte';
  // Styled on/off control from the shared component library.
  import { Switch } from '$lib/components/ui/switch/index.js';
  // Says when Windows turned off network access for sandboxed tools, with a way to set it up again.
  import SandboxAccess from './SandboxAccess.svelte';

  type Kind = 'agent' | 'project' | 'conversation';
  // `capabilitiesOnly` is the main agent: it holds capabilities and no resource rules.
  let { chat, kind, id, capabilitiesOnly = false }: { chat: Chat; kind: Kind; id: string; capabilitiesOnly?: boolean } = $props();
  let settings = $state<PermissionSettings | null>(null);
  let loading = $state(false), saving = $state(false), error = $state('');
  let targetType = $state<'application' | 'file' | 'folder' | 'command' | 'host' | 'domain'>('application');
  let resource = $state(''), action = $state('control');
  let commandKind = $state<'shell' | 'applescript'>('shell');
  let cwd = $state('');
  let ruleEffect = $state<'allow' | 'block'>('allow');
  let requestName = $state('');
  let request = 0;
  // Switches are shown as switches, not as rows among the resource rules.
  const resources = <T extends { target: PermissionTarget }>(list: T[]) =>
    list.filter(entry => entry.target.target_type !== 'feature' && entry.target.target_type !== 'mcp_server');
  // The saved MCP servers, so each can be switched on for this scope and rules can show names, not ids.
  let servers = $state<McpServer[]>([]);
  const serverName = (id: string) => servers.find(server => server.id === id)?.name ?? 'deleted server';
  const rows = $derived(kind === 'conversation'
    ? resources(settings?.effective ?? []).map(row => ({ ...row }))
    : resources(settings?.rules ?? []).map(rule => {
      const appId = rule.target.target_type === 'application' ? rule.target.id : null;
      return { target: rule.target,
        decision: appId && rule.effect === 'allow' &&
          !settings?.appCatalog.some(app => app.id === appId)
          ? 'unavailable' : rule.effect, source: kind };
    }));

  // Loads only the currently selected settings scope and discards stale replies.
  async function load() {
    const sequence = ++request;
    if (!id) { settings = null; return; }
    loading = true; error = '';
    try {
      const result = await chat.permissionSettings(kind, id);
      // A failure here only hides the MCP switches; the rest of the dialog still works.
      const listed = await chat.mcp<{ servers: McpServer[] }>('servers').catch(() => ({ servers: [] }));
      if (sequence === request) { settings = result; servers = listed.servers; }
    } catch (cause) {
      if (sequence === request) error = cause instanceof Error ? cause.message : 'Could not load permissions.';
    } finally {
      if (sequence === request) loading = false;
    }
  }

  // Refreshes when the visible agent, project, or conversation changes.
  $effect(() => { if (id) void load(); else { request++; settings = null; } });

  // The main agent is the agent scope with the nil id; it is named as such in the notes.
  const MAIN = '00000000-0000-0000-0000-000000000000';
  const scopeName = $derived(kind === 'agent' && id === MAIN ? 'main agent' : kind);
  type Feature = 'unsandboxed_shell' | 'default_full_access' | 'network' | 'web_fetch' | 'subagents' | 'task_tracking' | 'image_generation' | 'plan_mode' | 'coding_session' | 'no_web_search';
  // The switches this dialog offers. Web search is on unless it is switched off, so its rule is
  // stored the other way round (`no_web_search`) and shown here as an ordinary switch.
  const switches: { name: Feature; label: string; help: string; inverse?: boolean }[] = $derived([
  { name: 'no_web_search', inverse: true, label: 'Web search',
    help: `Lets the model search the web through its provider: Codex live search, Claude web search. The search runs at the provider, not on ${words.machine}. On by default.` },
  { name: 'unsandboxed_shell', label: 'Unsandboxed shell',
    help: `Allows timed shell grants in host chats. Commands run as ${words.account}, with access to your files, credentials and network outside MiniClaw’s sandbox. Each chat requires an explicit duration and renewal.` },
  { name: 'image_generation', label: 'Image generation',
    help: `Codex chats on ${words.machine} only. Generates pictures using the signed-in ChatGPT subscription and its Codex usage limits.` },
  { name: 'network', label: 'Network for commands',
    help: 'Lets shell commands ask for HTTPS access to hosts you approve. Off means no network at all.' },
  { name: 'web_fetch', label: 'Web fetch',
    help: 'Claude chats only. Reads web pages from hosts and domains allowed below. Claude Code also permits its own list of documentation sites.' },
  { name: 'subagents', label: 'Subagents',
    help: 'Claude chats only. Delegates work to helper agents restricted to the tools enabled for this conversation.' },
  { name: 'task_tracking', label: 'Task tracking',
    help: 'Claude chats only. Creates and updates a task checklist while working. Plan mode is a separate switch.' },
  { name: 'default_full_access', label: 'Full access by default',
    help: 'Starts new conversations with Full access on. Allowed actions run without asking; Coding session still reviews file changes. You can turn it off in each conversation from Tools. Existing conversations keep their settings.' },
  { name: 'plan_mode', label: 'Plan mode',
    help: 'Claude chats only. The model plans and explains what it would do, and does not act.' },
  { name: 'coding_session', label: 'Coding session',
    help: 'Every file change waits for your review with its diff, and every command asks first, even where a rule allows it. Set it on a project to make it the default for its conversations; a conversation can switch it either way.' }]);

  // Reads one switch for this scope: whether it is on, and where that comes from.
  function switchState(feature: Feature, inverse = false) {
    const matches = (target: PermissionTarget) => target.target_type === 'feature' && target.name === feature;
    const word = (set: boolean) => (set !== inverse ? 'On' : 'Off');
    if (kind !== 'conversation') {
      const set = (settings?.rules ?? []).some(rule => matches(rule.target) && rule.effect === 'allow');
      const bounded = ['image_generation', 'network', 'web_fetch', 'subagents', 'task_tracking', 'no_web_search'].includes(feature);
      return { on: set !== inverse, own: set, note: bounded ? allowance(set !== inverse, set)
        : set ? `On by default in ${scopeWords} conversations` : 'Off by default' };
    }
    const entry = (settings?.effective ?? []).find(row => matches(row.target));
    const own = entry?.source === 'conversation';
    if (!entry || entry.decision === 'ask') return { on: inverse, own: false, note: `${word(false)} by default` };
    if (entry.decision === 'block') return { on: inverse, own: true, note: `${word(false)} in this conversation` };
    return { on: !inverse, own, note: own ? `${word(true)} in this conversation`
      : `${word(true)}, from this conversation's ${entry.source === 'agent' && !chat.agentId ? 'main agent' : entry.source}` };
  }

  // A conversation answers with its own rule either way; an agent or project only adds or
  // removes its allow, because no rule is already the default there.
  function setSwitch(feature: Feature, on: boolean, inverse = false) {
    const target: PermissionTarget = { target_type: 'feature', name: feature };
    const set = on !== inverse;
    return change(target, set ? 'allow' : kind === 'conversation' ? 'block' : 'remove');
  }

  // Says whether a configuration allows a capability. An agent decides; a project can only add.
  function allowance(allowed: boolean, set: boolean) {
    if (kind === 'project') return set ? (allowed ? 'Allowed by this project' : 'Switched off by this project') : 'Left to the agent';
    return `${allowed ? 'Allowed' : 'Not allowed'} for ${scopeName === 'main agent' ? 'the main agent' : 'this agent'}`;
  }
  const scopeWords = $derived(scopeName === 'main agent' ? "the main agent's" : `this ${kind}'s`);

  // Reads one server's switch for this scope, in the same words as the other switches.
  function serverState(server: string) {
    const matches = (target: PermissionTarget) => target.target_type === 'mcp_server' && target.server === server;
    if (kind !== 'conversation') {
      const on = (settings?.rules ?? []).some(rule => matches(rule.target) && rule.effect === 'allow');
      return { on, own: on, note: allowance(on, on) };
    }
    const entry = (settings?.effective ?? []).find(row => matches(row.target));
    const own = entry?.source === 'conversation';
    if (!entry || entry.decision === 'ask') return { on: false, own: false, note: 'Off by default' };
    if (entry.decision === 'block') return { on: false, own: true, note: 'Off in this conversation' };
    return { on: true, own, note: own ? 'On in this conversation' : `On, from this conversation's ${entry.source}` };
  }

  // Names a reviewed target without shortening command text.
  function name(target: PermissionTarget) {
    if (target.target_type === 'application') return target.id;
    if (target.target_type === 'file') return target.path;
    if (target.target_type === 'folder') return `${target.path}/ (folder)`;
    if (target.target_type === 'host') return target.host;
    if (target.target_type === 'domain') return `${target.domain} and its subdomains`;
    if (target.target_type === 'feature') return target.name;
    if (target.target_type === 'mcp_server') return serverName(target.server);
    if (target.target_type === 'mcp_tool') return `${serverName(target.server)}: ${target.tool}`;
    if (target.target_type === 'program') return `Program: ${target.name}`;
    return `${target.kind === 'shell' ? 'Shell' : 'AppleScript'}: ${target.text}`;
  }

  // Says what a rule allows, in the row's second line.
  function detail(target: PermissionTarget) {
    if (target.target_type === 'command') return target.cwd;
    if (target.target_type === 'host' || target.target_type === 'domain') return 'HTTPS for shell commands';
    if (target.target_type === 'feature' || target.target_type === 'mcp_server') return '';
    if (target.target_type === 'mcp_tool') return 'MCP tool, with any input';
    if (target.target_type === 'program') return 'Runs without asking in any plain command line of allowed programs';
    return target.action;
  }

  // Explains the effective source in the same words used by the controls.
  function status(decision: string, source: string) {
    if (decision === 'block') return 'Blocked in this conversation';
    if (decision === 'unavailable') return 'Unavailable until app review';
    if (decision === 'ask') return 'Approval required';
    if (source === 'agent') return 'Allowed by agent';
    if (source === 'project') return 'Allowed by project';
    if (source === 'session') return 'Allowed for this session';
    return 'Allowed in this conversation';
  }

  // Saves one reviewed target and reloads effective sources after acknowledgement.
  async function change(target: PermissionTarget, choice: 'allow' | 'block' | 'remove') {
    if (!id || saving) return;
    saving = true; error = '';
    try {
      await chat.savePermission(kind, id, target, choice);
      await load();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Could not save permission.';
    } finally { saving = false; }
  }

  // Builds the exact app, file, or command target from the visible fields.
  function target(): PermissionTarget | null {
    if (!resource.trim()) return null;
    if (targetType === 'application') return { target_type: 'application',
      id: resource.trim(), action: action === 'observe' ? 'observe' : 'control' };
    if (targetType === 'file' || targetType === 'folder') return { target_type: targetType,
      path: resource.trim(), action: action === 'write' ? 'write' : 'read' };
    if (targetType === 'host') return { target_type: 'host', host: resource.trim().toLowerCase() };
    if (targetType === 'domain') return { target_type: 'domain', domain: resource.trim().toLowerCase() };
    if (!cwd.trim()) return null;
    return { target_type: 'command', kind: commandKind, text: resource, cwd: cwd.trim() };
  }

  // Saves the form's exact rule without enabling currently disabled scoped tools.
  async function add() {
    const value = target();
    if (!value) return;
    await change(value, ruleEffect);
    if (!error) { resource = ''; cwd = ''; }
  }

  // Submits a review request; a request never authorizes the app.
  async function requestApp() {
    if (!id || saving || !requestName.trim()) return;
    saving = true; error = '';
    try {
      await chat.requestApplication(kind, id, requestName.trim());
      requestName = '';
      await load();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Could not request app review.';
    } finally { saving = false; }
  }
</script>

<section aria-label="Permission settings" class="space-y-4 border-t border-border pt-4 text-sm">
  {#if kind === 'conversation'}
    <p class="text-xs text-muted-foreground">Capabilities are switched from Tools, beside the message box. What is offered there is what this conversation's agent allows.</p>
  {/if}
  <div class:hidden={capabilitiesOnly}>
    {#if kind !== 'conversation'}<h3 class="font-semibold">Permissions</h3>{/if}
    <p class="mt-1 text-xs text-muted-foreground">These rules govern MiniClaw's file and command tool in agent and project conversations. Shell commands run in {words.windows ? "MiniClaw's Windows sandbox, limited" : words.linux ? 'a bubblewrap sandbox limited' : 'a Seatbelt sandbox limited'} to the allowed files and folders. They have no network unless it is switched on here and the hosts are approved. They do not restrict Computer Use.</p>
  </div>
  <SandboxAccess />
  {#if loading && !settings}<p class="text-xs text-muted-foreground">Loading permissions...</p>{/if}
  {#if settings && kind !== 'conversation'}
   <div role="group" aria-label="Capabilities" class="space-y-3">
    <div>
      <h3 class="font-semibold">Capabilities</h3>
      <p class="mt-1 text-xs text-muted-foreground">{kind === 'project'
        ? 'A project can allow a capability for its conversations, on top of what their agent allows.'
        : 'What this agent may use at all. In each conversation, what is allowed here is switched on or off from Tools, beside the message box.'}</p>
    </div>
    <div class="space-y-3 rounded-md border border-border p-3">
      {#each switches.filter(item => item.name !== 'default_full_access' || kind === 'agent') as item (item.name)}
        {@const state = switchState(item.name, item.inverse)}
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <p class="font-medium" id={`switch-${item.name}`}>{item.label}</p>
            <p class="mt-0.5 text-xs text-muted-foreground">{item.help}</p>
            <p class="mt-1 text-xs">{state.note}</p>
          </div>
          <Switch class="mt-0.5 data-[state=checked]:bg-success" aria-label={item.label}
            checked={state.on} disabled={saving}
            onCheckedChange={on => void setSwitch(item.name, on, item.inverse)} />
        </div>
      {/each}
    </div>
    {#if servers.length}
    <div class="space-y-3 rounded-md border border-border p-3">
      <div>
        <p class="font-medium">MCP servers</p>
        <p class="mt-0.5 text-xs text-muted-foreground">A server that is on can be reached by the agent. Each of its tools still asks before it runs.</p>
      </div>
      {#each servers as server (server.id)}
        {@const state = serverState(server.id)}
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <p class="font-medium">{server.name}</p>
            <p class="mt-0.5 text-xs text-muted-foreground">{server.kind === 'http' ? 'Remote address' : server.unrestricted
              ? 'Local command, outside the sandbox' : 'Local command, sandboxed'} · {state.note}</p>
          </div>
          <Switch class="mt-0.5 data-[state=checked]:bg-success" aria-label={`MCP server ${server.name}`}
            checked={state.on} disabled={saving}
            onCheckedChange={on => void change({ target_type: 'mcp_server', server: server.id }, on ? 'allow' : 'remove')} />
        </div>
      {/each}
    </div>
    {/if}
   </div>
  {/if}
  {#if settings && !capabilitiesOnly}
    {#if rows.length}
      <ul class="divide-y divide-line border-y border-border">
        {#each rows as row (JSON.stringify(row.target))}
          <li class="flex flex-wrap items-start justify-between gap-2 py-3">
            <div class="min-w-0 flex-1">
              <p class="break-words font-medium [overflow-wrap:anywhere]">{name(row.target)}</p>
              <p class="mt-0.5 text-xs text-muted-foreground">
                {detail(row.target)}
              </p>
              <p class="mt-1 text-xs" class:text-destructive={row.decision === 'block'}>
                {status(row.decision, row.source)}
              </p>
            </div>
            {#if kind === 'conversation'}
              {#if row.source === 'conversation'}
                <Button variant="outline" size="sm" type="button" 
         disabled={saving} onclick={() => change(row.target, 'remove')}>
                  Remove conversation rule
                </Button>
              {:else if row.decision === 'allow'}
                <Button variant="outline" size="sm" type="button" class="border-destructive text-destructive hover:text-destructive"
         disabled={saving} onclick={() => change(row.target, 'block')}>
                  Block in this conversation
                </Button>
              {/if}
            {:else}
              <Button variant="outline" size="sm" type="button" 
        disabled={saving} onclick={() => change(row.target, 'remove')}>Remove permission</Button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="border-y border-border py-3 text-xs text-muted-foreground">No saved permissions for this scope.</p>
    {/if}
  {/if}
  {#if !capabilitiesOnly}
  <div class="space-y-3">
    <p class="font-medium">Add an exact rule</p>
    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
      <label class="block">Type
        <select class="agent-field mt-1" aria-label="Permission type" bind:value={targetType}
          onchange={() => { resource = '';
            action = targetType === 'file' || targetType === 'folder' ? 'read' : 'control'; }}>
          <option value="application">Application</option>
          <option value="file">File</option>
          <option value="folder">Folder</option>
          <option value="command">Command</option>
          <option value="host">Host</option>
          <option value="domain">Domain and subdomains</option>
        </select>
      </label>
      {#if targetType === 'host' || targetType === 'domain'}
        <p class="self-end pb-2 text-xs text-muted-foreground">HTTPS only, port 443.</p>
      {:else if targetType !== 'command'}
        <label class="block">Action
          <select class="agent-field mt-1" aria-label="Permission action" bind:value={action}>
            {#if targetType === 'application'}
              <option value="observe">Observe</option><option value="control">Control</option>
            {:else}
              <option value="read">Read</option><option value="write">Write</option>
            {/if}
          </select>
        </label>
      {:else}
        <label class="block">Command type
          <select class="agent-field mt-1" aria-label="Command type" bind:value={commandKind}>
            <option value="shell">Shell</option>{#if words.appleScript}<option value="applescript">AppleScript</option>{/if}
          </select>
        </label>
      {/if}
    </div>
    {#if targetType === 'application'}
      <label class="block">Vetted app
        <select class="agent-field mt-1" aria-label="Vetted app" bind:value={resource}
          disabled={saving}>
          <option value="">Choose an app</option>
          {#each settings?.appCatalog ?? [] as app (app.id)}
            <option value={app.id}>{app.name}</option>
          {/each}
        </select>
      </label>
    {:else}
    <label class="block">{targetType === 'file' ? 'Absolute file path'
      : targetType === 'folder' ? 'Absolute folder path'
      : targetType === 'host' ? 'Exact host name, for example registry.npmjs.org'
      : targetType === 'domain' ? 'Domain, for example npmjs.org' : 'Exact command'}
      {#if targetType === 'command'}
        <Textarea class="mt-1 min-h-20" bind:value={resource} maxlength={16384}
          disabled={saving} />
      {:else}
        <Input class="mt-1" bind:value={resource} maxlength={4096}
          disabled={saving} />
      {/if}
    </label>
    {/if}
    {#if targetType === 'command'}
      <label class="block">Working directory
        <Input class="mt-1" bind:value={cwd} maxlength={4096}
          disabled={saving} />
      </label>
    {/if}
    {#if kind === 'conversation'}
      <label class="block">Decision
        <select class="agent-field mt-1" aria-label="Permission decision" bind:value={ruleEffect}>
          <option value="allow">Allow</option><option value="block">Block</option>
        </select>
      </label>
    {/if}
    <Button type="button" onclick={add}
   
   disabled={saving || loading || !id || !resource.trim() ||
    (targetType === 'command' && !cwd.trim())}>Save permission</Button>
  </div>
  <div class="space-y-2 border-t border-border pt-4">
    <p class="font-medium">Other apps</p>
    <p class="text-xs text-muted-foreground">A request does not grant access. The app stays unavailable until it is reviewed and added to the vetted list.</p>
    <label class="block">Request another app
      <Input class="mt-1" bind:value={requestName} maxlength={120}
        disabled={saving} />
    </label>
    <button type="button" onclick={requestApp}
      class="rounded-md border border-border px-3 py-2 text-sm disabled:opacity-40"
      disabled={saving || loading || !id || !requestName.trim()}>Request app review</button>
    {#if settings?.appRequests.length}
      <ul class="space-y-1 text-xs text-muted-foreground" aria-label="App review requests">
        {#each settings.appRequests as item (item.name)}
          <li>{item.name}: pending review</li>
        {/each}
      </ul>
    {/if}
  </div>
  {/if}
  {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
</section>
