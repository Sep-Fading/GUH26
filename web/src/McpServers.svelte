<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Loads the servers when Settings shows this panel.
  import { onMount } from 'svelte';
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  // Marks the commands and states of this panel.
  import Plus from '@lucide/svelte/icons/plus';
  import Download from '@lucide/svelte/icons/download';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import PlugZap from '@lucide/svelte/icons/plug-zap';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import ShieldAlert from '@lucide/svelte/icons/shield-alert';
  import X from '@lucide/svelte/icons/x';
  // Opens the numbered guide for a value or a known server.
  import SetupGuide from './SetupGuide.svelte';
  // Knows which values and servers have a guide, and what a right value looks like.
  import { guideForServer, guideForValue, valueProblem } from '../../scripts/setup/guides.mjs';
  // Confirms deletion of a server.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Chooses which provider a server is for.
  import Choice from './Choice.svelte';
  // Reaches the MCP routes through the app's request helper.
  import type { Chat, McpServer } from './chat.svelte';

  type Tool = { name: string; title: string | null; description: string | null };
  type Checked = { server?: { name: string; version: string }; tools?: Tool[]; note?: string; error?: string; busy?: boolean };
  type Source = { name: string; kind: 'stdio' | 'http'; command: string | null; args: string[]; url: string | null;
    env: string[]; needsToken: boolean };
  // Arguments and hosts are edited one per line; a secret's value exists here only until it is saved.
  type Draft = { name: string; kind: 'stdio' | 'http'; command: string; args: string; url: string; folder: string; runtimePaths: string;
    hosts: string; ports: string; listen: string; unrestricted: boolean; provider: 'both' | 'codex' | 'claude'; startup: string; tool: string;
    enabledTools: string; disabledTools: string; keptHeaders: string[]; addedHeaders: Record<string, string>; kept: string[]; added: Record<string, string>; token: string; clearToken: boolean };

  let { chat }: { chat: Chat } = $props();
  let items = $state<McpServer[]>([]);
  let draft = $state<Draft | null>(null);
  let editing = $state<McpServer>();
  let checks = $state<Record<string, Checked>>({});
  let importing = $state(false), provider = $state<'codex' | 'claude'>('codex'), sources = $state<Source[]>([]);
  let valueName = $state(''), valueText = $state(''), valueError = $state('');
  let busy = $state(false), error = $state('');

  const lines = (text: string) => text.split('\n').map(line => line.trim()).filter(Boolean);
  const blank = (): Draft => ({ name: '', kind: 'stdio', command: '', args: '', url: '', folder: '', runtimePaths: '', hosts: '', ports: '', listen: '',
    provider: 'both', startup: '15', tool: '300', enabledTools: '', disabledTools: '', keptHeaders: [], addedHeaders: {},
    unrestricted: false, kept: [], added: {}, token: '', clearToken: false });

  // Runs one request, then reloads the list; a failure is shown in words.
  async function attempt(work: () => Promise<unknown>) {
    busy = true; error = '';
    try { await work(); items = (await chat.mcp<{ servers: McpServer[] }>('servers')).servers; return true; }
    catch (cause) { error = cause instanceof Error ? cause.message : 'The request failed.'; return false; }
    finally { busy = false; }
  }

  // Builds the request: a kept value is sent as null, a new one with its text, a removed one not at all.
  function body(draft: Draft) {
    // A field the chosen provider's configuration does not have keeps its default, so nothing hidden is sent.
    const shared = { name: draft.name.trim(), kind: draft.kind, provider: draft.provider, toolSeconds: Number(draft.tool) || 300,
      ...(fields.startup ? { startupSeconds: Number(draft.startup) || 15 } : {}),
      ...(fields.toolLists ? { enabledTools: lines(draft.enabledTools), disabledTools: lines(draft.disabledTools) } : {}),
      ...(editing ? { revision: editing.revision } : {}) };
    if (draft.kind === 'http') {
      const headers: Record<string, string | null> = { ...Object.fromEntries(draft.keptHeaders.map(name => [name, null])), ...draft.addedHeaders };
      return { ...shared, url: draft.url.trim(), headers, ...(draft.token ? { token: draft.token } : draft.clearToken ? { token: null } : {}) };
    }
    const env: Record<string, string | null> = { ...Object.fromEntries(draft.kept.map(name => [name, null])), ...draft.added };
    // Ports are typed with commas or spaces; anything that is not a number is left for the backend to refuse.
    const ports = draft.ports.split(/[\s,]+/).filter(Boolean).map(Number);
    const listen = draft.listen.split(/[\s,]+/).filter(Boolean).map(Number);
    return { ...shared, command: draft.command.trim(), args: lines(draft.args), runtimePaths: lines(draft.runtimePaths), hosts: lines(draft.hosts), ports, listen,
      ...(draft.folder.trim() ? { folder: draft.folder.trim() } : {}), unrestricted: draft.unrestricted, env,
      // Proposed names the user has not given a value yet are remembered as still to enter.
      ...(!editing && proposedNames.length ? { needs: $state.snapshot(proposedNames) } : {}) };
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!draft) return;
    const path = editing ? `servers/${editing.id}` : 'servers';
    const request = body($state.snapshot(draft) as Draft);
    if (await attempt(() => chat.mcp(path, request))) {
      if (editing) delete checks[editing.id];
      draft = null; editing = undefined; proposedNames = [];
    }
  }

  function edit(server: McpServer) {
    editing = server; importing = false;
    draft = { ...blank(), name: server.name, kind: server.kind, command: server.command ?? '', args: server.args.join('\n'),
      url: server.url ?? '', folder: server.folder ?? '', runtimePaths: (server.runtimePaths ?? []).join('\n'), hosts: server.hosts.join('\n'), ports: (server.ports ?? []).join(', '), listen: (server.listen ?? []).join(', '),
      unrestricted: server.unrestricted, provider: server.provider ?? 'both', startup: String(server.startupSeconds ?? 15),
      tool: String(server.toolSeconds ?? 300), enabledTools: (server.enabledTools ?? []).join('\n'),
      disabledTools: (server.disabledTools ?? []).join('\n'), keptHeaders: [...(server.headers ?? [])],
      kept: [...server.env] };
  }

  // Which of the provider-specific fields the chosen provider's own configuration has, and how it names them.
  // MiniClaw honours every one itself, since it is the client; the choice decides what is shown and sent.
  const providers = [{ value: 'both', label: 'Codex and Claude' }, { value: 'codex', label: 'Codex' }, { value: 'claude', label: 'Claude' }];
  const fields = $derived({ startup: draft?.provider !== 'claude', toolLists: draft?.provider !== 'claude' });
  const naming = $derived(draft?.provider === 'codex'
    ? (draft.kind === 'http' ? 'Codex names these url, bearer_token_env_var, http_headers, startup_timeout_sec, tool_timeout_sec, enabled_tools and disabled_tools.'
      : 'Codex names these command, args, env, cwd, startup_timeout_sec, tool_timeout_sec, enabled_tools and disabled_tools.')
    : draft?.provider === 'claude'
      ? (draft.kind === 'http' ? 'Claude Code names these type, url and headers.' : 'Claude Code names these type, command, args and env.')
      : 'One server for both providers. MiniClaw connects to it itself, so neither provider needs its own entry.');
  let headerName = $state(''), headerValue = $state('');

  function addHeader() {
    if (!draft || !/^[A-Za-z0-9_-]+$/.test(headerName) || !headerValue) return;
    draft.keptHeaders = draft.keptHeaders.filter(name => name !== headerName);
    draft.addedHeaders = { ...draft.addedHeaders, [headerName]: headerValue };
    headerName = ''; headerValue = '';
  }

  function addValue() {
    if (!draft || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(valueName) || !valueText) return;
    // A known value in a plainly wrong format is refused with what a right one looks like; the value is never repeated.
    valueError = valueProblem(valueName, valueText) ?? '';
    if (valueError) return;
    draft.kept = draft.kept.filter(name => name !== valueName);
    draft.added = { ...draft.added, [valueName]: valueText };
    // The next proposed name that still has no value is offered next.
    proposedNames = proposedNames.filter(name => name !== valueName);
    valueName = proposedNames[0] ?? ''; valueText = '';
  }

  // Connects to the server and lists its tools. Nothing is switched on by this.
  async function check(server: McpServer) {
    checks[server.id] = { busy: true };
    try { checks[server.id] = await chat.mcp<Checked>(`servers/${server.id}/check`, {}); }
    catch (cause) { checks[server.id] = { error: cause instanceof Error ? cause.message : 'The server could not be reached.' }; }
  }

  async function discover(from: 'codex' | 'claude') {
    provider = from; busy = true; error = '';
    try { sources = (await chat.mcp<{ sources: Source[] }>(`sources?provider=${from}`)).sources; }
    catch (cause) { sources = []; error = cause instanceof Error ? cause.message : 'The request failed.'; }
    finally { busy = false; }
  }

  // What the user still has to enter after an import, in words.
  function missing(server: McpServer) {
    const values = server.needs.filter(name => name !== 'token');
    return [server.needs.includes('token') ? 'Enter its token' : '', values.length ? `Enter ${values.join(', ')}` : '']
      .filter(Boolean).join(' · ');
  }

  // The guides worth offering for a server: one per value it still needs, else the server's own setup guide.
  function guides(server: McpServer) {
    const values = server.needs.filter(name => guideForValue(name));
    if (values.length) return values.map(name => ({ topic: guideForValue(name)!, label: `How do I find ${name}?` }));
    const topic = guideForServer(server.name);
    return topic ? [{ topic, label: `How do I set up ${server.name}?` }] : [];
  }
  // A server an agent proposed opens as a new draft. Its secrets are names only: the first one is
  // put in the name field so the user enters its value, and the rest are said in words.
  let proposedNames = $state<string[]>([]);
  $effect(() => {
    const proposal = chat.mcpProposal;
    if (!proposal) return;
    chat.mcpProposal = null;
    editing = undefined; importing = false;
    draft = { ...blank(), name: proposal.name, command: proposal.command, args: proposal.args.join('\n'),
      folder: proposal.folder ?? '', hosts: proposal.hosts.join('\n'), ports: proposal.ports.join(', '), listen: proposal.listen.join(', ') };
    proposedNames = proposal.env;
    valueName = proposal.env[0] ?? ''; valueText = '';
  });

  // Values in the open form that have a guide, each once.
  const formGuides = $derived(draft ? [...new Set([...proposedNames, ...draft.kept, ...Object.keys(draft.added), valueName.trim()])]
    .filter(name => guideForValue(name)) : []);

  onMount(() => { void attempt(async () => {}); });
</script>

<section aria-label="MCP servers" class="flex min-h-0 flex-1 flex-col overflow-y-auto">
  <!-- The right padding keeps the button clear of the sheet's own close button. -->
  <div class="sticky top-0 z-[5] flex h-12 shrink-0 items-center justify-between border-b border-border bg-background pl-5 pr-14">
    <h2 class="text-base font-semibold">MCP servers</h2>
    <button class="icon-button" aria-label="Reload servers" disabled={busy} onclick={() => attempt(async () => {})}>
      <RefreshCw size={16} aria-hidden="true" /></button>
  </div>
  <div class="space-y-4 p-5 text-sm">
    <p class="text-xs text-muted-foreground">One list for Codex and Claude conversations. MiniClaw connects to a server itself, so
      neither provider is given its command, address or secrets. A saved server is switched on nowhere: allow it for an agent in
      Agents (the main agent covers plain conversations), then switch it per conversation from Tools, beside the message box.
      Each tool asks before it runs.</p>
    {#each items as item (item.id)}
      {@const checked = checks[item.id]}
      <div role="group" aria-label={item.name} class="rounded-lg border border-border p-3">
        <div class="flex flex-wrap items-center gap-3">
          <div class="min-w-0 flex-1">
            <p class="truncate font-medium">{item.name}</p>
            <p class="flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
              {#if item.provider && item.provider !== 'both'}<span class="rounded-full bg-muted px-1.5 text-foreground">{item.provider === 'codex' ? 'Codex only' : 'Claude only'}</span>{/if}
              {#if item.kind === 'http'}Remote address · {item.hasToken ? 'Token saved' : 'No token'}
              {:else}Local command ·
                {#if item.unrestricted}<span class="inline-flex items-center gap-1 text-destructive"><ShieldAlert size={12} aria-hidden="true" />Unrestricted</span>
                {:else}<span class="inline-flex items-center gap-1"><ShieldCheck size={12} aria-hidden="true" />Sandboxed</span>{/if}
              {/if}
            </p>
          </div>
          <Button variant="secondary" size="sm" disabled={busy || checked?.busy} onclick={() => check(item)}>
            <PlugZap size={14} aria-hidden="true" />Check connection</Button>
          <Button variant="ghost" size="sm" disabled={busy} onclick={() => edit(item)}>Edit</Button>
          <ConfirmDelete label="Delete server" actionLabel="Delete server" disabled={busy}
            description={`Delete ${item.name}? Its saved secrets are removed from ${words.secretStoreShort}, and it is switched off everywhere.`}
            action={async () => { await chat.mcp(`servers/${item.id}/delete`, { revision: item.revision }); delete checks[item.id];
              await attempt(async () => {}); }} />
        </div>
        <p class="mt-2 break-all font-mono text-xs text-muted-foreground">{item.kind === 'http' ? item.url : [item.command, ...item.args].join(' ')}</p>
        {#if item.runs}<p class="mt-1 text-xs text-muted-foreground">{item.runs}</p>{/if}
        {#if item.env.length}<p class="mt-1 text-xs text-muted-foreground">Saved values: {item.env.join(', ')}</p>{/if}
        {#if item.headers?.length}<p class="mt-1 text-xs text-muted-foreground">Headers: {item.headers.join(', ')}</p>{/if}
        {#if item.ports?.length}<p class="mt-1 text-xs text-muted-foreground">Local ports: {item.ports.join(', ')}</p>{/if}
        {#if item.listen?.length}<p class="mt-1 text-xs text-muted-foreground">Listens on: {item.listen.join(', ')}</p>{/if}
        {#if missing(item)}<p class="mt-1 text-xs text-destructive">{missing(item)}</p>{/if}
        {#each guides(item) as help (help.label)}<div class="mt-1"><SetupGuide topic={help.topic} label={help.label} /></div>{/each}
        {#if checked?.busy}<p class="mt-2 text-xs text-muted-foreground">Connecting...</p>{/if}
        {#if checked?.error}<p role="alert" class="mt-2 text-xs text-destructive">{checked.error}</p>{/if}
        {#if checked?.tools}
          <div class="mt-2 border-t border-border pt-2">
            <p class="text-xs text-muted-foreground">{checked.server?.name || 'Server'} {checked.server?.version} ·
              {checked.tools.length} {checked.tools.length === 1 ? 'tool' : 'tools'}</p>
            {#if checked.note}<p role="status" class="mt-1 text-xs">{checked.note}</p>{/if}
            <ul aria-label={`Tools of ${item.name}`} class="mt-1 space-y-1">
              {#each checked.tools as tool (tool.name)}
                <li class="flex flex-wrap gap-x-2 text-xs"><span class="font-mono font-medium">{tool.name}</span>
                  {#if tool.description}<span class="min-w-0 text-muted-foreground">{tool.description}</span>{/if}</li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>
    {:else}
      {#if !draft && !importing}<p class="text-muted-foreground">No MCP servers yet.</p>{/if}
    {/each}

    {#if draft}
      <form class="space-y-3 rounded-lg border border-border p-3" onsubmit={save}>
        <label class="block">Name<Input class="mt-1" bind:value={draft.name} maxlength={40} required
          pattern="[a-z0-9][a-z0-9_\-]*" title="Lowercase letters, digits, - and _" /></label>
        <div>
          <div class="flex flex-wrap items-center gap-2">
            <span class="text-sm" id="mcp-provider-label">Provider</span>
            <Choice label="Provider" value={draft.provider} placeholder="Codex and Claude" options={providers}
              onchange={value => { draft!.provider = value as Draft['provider']; }} />
          </div>
          <p class="mt-1 text-xs text-muted-foreground">Whose conversations may use this server. {naming}</p>
        </div>
        {#if !editing}
          <div role="radiogroup" aria-label="Kind" class="flex w-fit gap-1 rounded-lg bg-muted p-1">
            {#each [['stdio', 'Local command'], ['http', 'Remote address']] as [value, label] (value)}
              <button type="button" role="radio" aria-checked={draft.kind === value} class="rounded-md px-3 py-1"
                class:bg-background={draft.kind === value} class:shadow-sm={draft.kind === value}
                onclick={() => { draft!.kind = value as Draft['kind']; }}>{label}</button>
            {/each}
          </div>
        {/if}
        {#if draft.kind === 'stdio'}
          <label class="block">Command<Input class="mt-1 font-mono" bind:value={draft.command} placeholder={words.commandPlaceholder} required /></label>
          <label class="block">Arguments<Textarea class="mt-1 font-mono" rows={3} bind:value={draft.args} placeholder="One per line" /></label>
          <div>
            <label class="block">Folder<Input class="mt-1 font-mono" bind:value={draft.folder} placeholder={words.folderPlaceholder}
              aria-describedby="mcp-folder-help" /></label>
            <p id="mcp-folder-help" class="mt-1 text-xs text-muted-foreground">The one folder a sandboxed server may read and write. It is also its working folder.</p>
            <label class="mt-3 block">Read-only runtime folders<Textarea class="mt-1 font-mono" bind:value={draft.runtimePaths} rows={2}
              placeholder="One absolute Python or Node runtime folder per line" aria-describedby="mcp-runtime-help" /></label>
            <p id="mcp-runtime-help" class="mt-1 text-xs text-muted-foreground">Optional, up to eight folders. This server can read their contents. For a Python virtual environment, add the base Python installation shown by its pyvenv.cfg. Grant only the interpreter and library folder you trust; do not grant your home folder.</p>
          </div>
          <div>
            <label class="block">Allowed hosts<Textarea class="mt-1 font-mono" rows={2} bind:value={draft.hosts}
              placeholder="api.example.com, one per line" aria-describedby="mcp-hosts-help" /></label>
            <p id="mcp-hosts-help" class="mt-1 text-xs text-muted-foreground">A sandboxed server has no network except HTTPS to these hosts.</p>
          </div>
          <div>
            <label class="block">Local ports<Input class="mt-1 font-mono" bind:value={draft.ports} placeholder="9876" inputmode="numeric"
              aria-describedby="mcp-ports-help" /></label>
            <p id="mcp-ports-help" class="mt-1 text-xs text-muted-foreground">Ports on {words.machine} a sandboxed server may connect to, for a server that
              drives an app through the app's own socket (Blender's add-on listens on 9876). Without one it reaches no local port at all.</p>
          </div>
          <div>
            <label class="block">Listening ports<Input class="mt-1 font-mono" bind:value={draft.listen} placeholder="13469" inputmode="numeric"
              aria-describedby="mcp-listen-help" /></label>
            <p id="mcp-listen-help" class="mt-1 text-xs text-muted-foreground">Ports the server itself listens on, for an app on {words.machine} to connect to
              (Roblox Studio's plugin dials 13469). Loopback only, so nothing outside {words.machine} can reach it; any app on {words.machine} can.</p>
          </div>
          <fieldset class="space-y-2">
            <legend class="font-medium">Environment values</legend>
            <p class="text-xs text-muted-foreground">Values are kept in {words.secretStore} and are never shown again.</p>
            {#if proposedNames.length}
              <p class="text-xs text-destructive">This server needs: {proposedNames.join(', ')}. Enter each value and press Add value.</p>
            {/if}
            {#each [...draft.kept, ...Object.keys(draft.added)] as name (name)}
              <div class="flex items-center gap-2 text-xs"><span class="font-mono">{name}</span>
                <span class="text-muted-foreground">{name in draft.added ? 'new value' : 'saved'}</span>
                <button type="button" class="icon-button" aria-label={`Remove ${name}`} onclick={() => {
                  draft!.kept = draft!.kept.filter(kept => kept !== name);
                  const { [name]: _removed, ...rest } = draft!.added; draft!.added = rest; }}><X size={14} aria-hidden="true" /></button>
              </div>
            {/each}
            <div class="flex flex-wrap items-end gap-2">
              <label class="flex flex-col gap-1 text-xs">Value name<Input class="font-mono" bind:value={valueName} placeholder="API_KEY"
                oninput={() => { valueError = ''; }} /></label>
              <label class="flex flex-col gap-1 text-xs">Value<Input type="password" autocomplete="off" bind:value={valueText}
                oninput={() => { valueError = ''; }} /></label>
              <Button type="button" variant="secondary" size="sm" disabled={!valueName || !valueText} onclick={addValue}>Add value</Button>
            </div>
            {#if valueError}<p role="alert" class="text-xs text-destructive">{valueError}</p>{/if}
            {#each formGuides as name (name)}<div><SetupGuide topic={guideForValue(name)!} label={`How do I find ${name}?`} /></div>{/each}
          </fieldset>
          <div class="flex items-start justify-between gap-3 rounded-md border border-border p-3">
            <div class="min-w-0">
              <p class="font-medium" id="mcp-unrestricted">Run outside the sandbox</p>
              <p class="mt-0.5 text-xs text-muted-foreground">Off is the default: the server runs confined to its folder and hosts.</p>
              {#if draft.unrestricted}
                <p role="alert" class="mt-1 text-xs text-destructive">An unrestricted server can read and change anything you can,
                  including provider sign-ins. Use it only for a server you trust completely.</p>
              {/if}
            </div>
            <Switch class="mt-0.5 data-[state=checked]:bg-destructive" aria-label="Run outside the sandbox"
              checked={draft.unrestricted} onCheckedChange={on => { draft!.unrestricted = on; }} />
          </div>
        {:else}
          <label class="block">Address<Input class="mt-1 font-mono" bind:value={draft.url} placeholder="https://mcp.example.com/mcp" required /></label>
          <div>
            <label class="block">Token<Input class="mt-1" type="password" autocomplete="off" bind:value={draft.token}
              placeholder={editing?.hasToken ? 'Saved. Type to replace it.' : 'Optional'} aria-describedby="mcp-token-help" /></label>
            <p id="mcp-token-help" class="mt-1 text-xs text-muted-foreground">Sent as a bearer token to this address only. Kept in {words.secretStore} and never shown again.</p>
          </div>
          {#if editing?.hasToken && !draft.token}
            <label class="flex items-center gap-2 text-xs"><input type="checkbox" bind:checked={draft.clearToken} />Remove the saved token</label>
          {/if}
        {/if}
        {#if draft.kind === 'http'}
          <fieldset class="space-y-2">
            <legend class="font-medium">Headers</legend>
            <p class="text-xs text-muted-foreground">Extra HTTP headers the server needs, such as an API key. Values are kept in {words.secretStore} and are never shown again.</p>
            {#each [...draft.keptHeaders, ...Object.keys(draft.addedHeaders)] as name (name)}
              <div class="flex items-center gap-2 text-xs"><span class="font-mono">{name}</span>
                <span class="text-muted-foreground">{name in draft.addedHeaders ? 'new value' : 'saved'}</span>
                <button type="button" class="icon-button" aria-label={`Remove header ${name}`} onclick={() => {
                  draft!.keptHeaders = draft!.keptHeaders.filter(kept => kept !== name);
                  const { [name]: _removed, ...rest } = draft!.addedHeaders; draft!.addedHeaders = rest; }}><X size={14} aria-hidden="true" /></button>
              </div>
            {/each}
            <div class="flex flex-wrap items-end gap-2">
              <label class="flex flex-col gap-1 text-xs">Header name<Input class="font-mono" bind:value={headerName} placeholder="X-Api-Key" /></label>
              <label class="flex flex-col gap-1 text-xs">Header value<Input type="password" autocomplete="off" bind:value={headerValue} /></label>
              <Button type="button" variant="secondary" size="sm" disabled={!headerName || !headerValue} onclick={addHeader}>Add header</Button>
            </div>
          </fieldset>
        {/if}
        <fieldset class="space-y-2">
          <legend class="font-medium">Limits and tools</legend>
          <div class="flex flex-wrap gap-3">
            {#if fields.startup}
              <label class="flex flex-col gap-1 text-xs">Startup timeout<Input class="w-28 font-mono" bind:value={draft.startup} inputmode="numeric" aria-describedby="mcp-limits-help" /></label>
            {/if}
            <label class="flex flex-col gap-1 text-xs">Tool timeout<Input class="w-28 font-mono" bind:value={draft.tool} inputmode="numeric" aria-describedby="mcp-limits-help" /></label>
          </div>
          <p id="mcp-limits-help" class="text-xs text-muted-foreground">Seconds. A start may take up to 120, one tool call up to 3600; a slow render needs more than the usual 300.</p>
          {#if fields.toolLists}
            <div class="grid gap-3 sm:grid-cols-2">
              <label class="block text-xs">Enabled tools<Textarea class="mt-1 font-mono" rows={2} bind:value={draft.enabledTools} placeholder="One per line. Empty offers every tool." /></label>
              <label class="block text-xs">Disabled tools<Textarea class="mt-1 font-mono" rows={2} bind:value={draft.disabledTools} placeholder="One per line. Never offered." /></label>
            </div>
          {/if}
        </fieldset>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" onclick={() => { draft = null; editing = undefined; proposedNames = []; }}>Cancel</Button>
          <Button type="submit" disabled={busy || !draft.name.trim() || (draft.kind === 'stdio' ? !draft.command.trim() : !draft.url.trim())}>Save server</Button>
        </div>
      </form>
    {:else}
      <div class="flex flex-wrap gap-2">
        <Button variant="secondary" size="sm" onclick={() => { draft = blank(); editing = undefined; importing = false; }}>
          <Plus size={14} aria-hidden="true" />New server</Button>
        <Button variant="secondary" size="sm" aria-expanded={importing} onclick={() => { importing = !importing; if (importing) void discover(provider); }}>
          <Download size={14} aria-hidden="true" />Import servers</Button>
      </div>
    {/if}

    {#if importing && !draft}
      <section aria-label="Import servers" class="space-y-3 rounded-lg border border-border p-3">
        <div role="radiogroup" aria-label="Import from" class="flex w-fit gap-1 rounded-lg bg-muted p-1">
          {#each [['codex', 'Codex'], ['claude', 'Claude Code']] as [value, label] (value)}
            <button type="button" role="radio" aria-checked={provider === value} class="rounded-md px-3 py-1"
              class:bg-background={provider === value} class:shadow-sm={provider === value}
              disabled={busy} onclick={() => discover(value as typeof provider)}>{label}</button>
          {/each}
        </div>
        <p class="text-xs text-muted-foreground">Only the definition is copied. Tokens and environment values are not read into
          MiniClaw; you enter them again. An imported server is switched on nowhere.</p>
        {#each sources.filter(source => !items.some(item => item.name === source.name)) as source (source.name)}
          <div role="group" aria-label={source.name} class="flex flex-wrap items-center gap-3 rounded-md border border-border p-2">
            <div class="min-w-0 flex-1">
              <p class="font-medium">{source.name}</p>
              <p class="break-all font-mono text-xs text-muted-foreground">{source.kind === 'http' ? source.url : [source.command, ...source.args].join(' ')}</p>
            </div>
            <Button variant="secondary" size="sm" disabled={busy}
              onclick={() => attempt(() => chat.mcp('import', { provider, name: source.name }))}>Import</Button>
          </div>
        {:else}
          {#if !busy}<p class="text-muted-foreground">Nothing to import from {provider === 'codex' ? 'Codex' : 'Claude Code'}.</p>{/if}
        {/each}
      </section>
    {/if}
    {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  </div>
</section>
