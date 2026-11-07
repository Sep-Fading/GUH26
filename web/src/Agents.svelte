<script lang="ts">
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Styled text fields from the shared component library.
  import { Input } from '$lib/components/ui/input/index.js';
  // Loads this panel's data when Settings shows it.
  import { onMount } from 'svelte';
  // Keeps dialog focus and dismissal accessible.
  // Identifies profile management and editing commands.
  import Users from '@lucide/svelte/icons/users';
  import Plus from '@lucide/svelte/icons/plus';
  import Bot from '@lucide/svelte/icons/bot';
  // Ticks skill groups, as project settings do.
  import GroupChoices from './GroupChoices.svelte';
  // Edits delegations per message and the time limit, for the main agent and named agents.
  import DelegationLimits from './DelegationLimits.svelte';
  import Save from '@lucide/svelte/icons/save';
  import X from '@lucide/svelte/icons/x';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Uses the existing model selection control.
  import Choice from './Choice.svelte';
  // Shares Markdown editing with skill definitions.
  import Instructions from './Instructions.svelte';
  // Confirms deletion before changing saved profiles.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Edits lasting rules without enabling scoped tools.
  import AgentCapabilities from './AgentCapabilities.svelte';
  // Shares profile data with the chat.
  import type { Agent, AgentDraft, Chat, McpServer, PermissionTarget } from './chat.svelte';
  // Words a suggested permission the way the settings do.
  import { permissionLabel, type SuggestedPermission } from './message-markdown';

  let { chat }: { chat: Chat } = $props();
  let open = $state(false);
  let id = $state<string>();
  let draft = $state<AgentDraft>(empty());
  let saved = $state(false);
  const supported = $derived(!draft.capabilities.length && !draft.access.applications.length &&
    !draft.access.files.length && !draft.access.network.length);
  // Every other saved agent can be a recipient; an agent never delegates to itself.
  const recipients = $derived(chat.agents.filter(agent => agent.id !== id));
  const model = $derived(chat.models.find(model => model.model === draft.model));

  // Returns a new inactive profile with no capabilities, grants, or delegation targets.
  function empty(): AgentDraft {
    return { name: '', instructions: '', active: false, model: null, effort: null, speed: null,
      capabilities: [], access: { applications: [], files: [], network: [] }, delegatesTo: [],
      delegation: { maxPerTurn: 3, minutes: 10 }, skillIds: [], skillGroupIds: [] };
  }

  // Copies a saved profile into the form or starts an empty definition.
  function edit(agent?: Agent) {
    id = agent?.id;
    if (agent) {
      const { id: _id, ...definition } = agent;
      draft = JSON.parse(JSON.stringify(definition));
    } else {
      draft = empty();
      draft.model = chat.model || null;
      draft.effort = chat.effort || null;
      draft.speed = chat.speed || null;
    }
    chat.agentError = '';
    saved = false; proposed = ''; suggestions = [];
  }

  // Refreshes the registry on opening without navigating away from the conversation.
  function opened(value: boolean) {
    open = value;
    if (value) { edit(); void chat.refreshAgents(); void chat.skills.refresh(); void chat.groups.refresh(); }
  }

  // Selects a model and its advertised default reasoning effort for this profile.
  function chooseModel(value: string) {
    draft.model = value;
    draft.effort = chat.models.find(model => model.model === value)?.defaultReasoningEffort ?? null;
    draft.speed = null;
    saved = false;
  }

  // Saves the current form and updates its revision only after backend acknowledgement.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    const agent = await chat.saveAgent(id, $state.snapshot(draft));
    // What the agent suggested stays after saving: that is when each one can be approved.
    const waiting = suggestions;
    if (agent) { edit(agent); saved = true; suggestions = waiting; }
  }

  // The main agent: the configuration of conversations with no agent. Its capabilities are rules
  // under the agent scope with the nil id; its default model is kept beside them.
  const MAIN = '00000000-0000-0000-0000-000000000000';
  type Defaults = { model: string | null; effort: string | null; speed: string | null; skillIds: string[]; skillGroupIds: string[];
    delegation: AgentDraft['delegation'] };
  let main = $state(false), defaults = $state<Defaults>({ model: null, effort: null, speed: null, skillIds: [], skillGroupIds: [],
    delegation: { maxPerTurn: 3, minutes: 10 } });
  // The speed tiers a chosen model offers; a choice shows Codex's own words for each.
  const tiers = (name: string | null) => (chat.models.find(model => model.model === name)?.serviceTiers ?? [])
    .map(tier => ({ value: tier.id, name: tier.name, label: tier.description ? `${tier.name}: ${tier.description}` : tier.name }));
  let defaultsBusy = $state(false), defaultsError = $state('');

  async function loadMain() {
    void chat.skills.refresh(); void chat.groups.refresh();
    try { defaults = await chat.mainAgent<Defaults>(); defaultsError = ''; }
    catch (cause) { defaultsError = cause instanceof Error ? cause.message : 'Could not load the main agent.'; }
  }

  // Saves the model and effort, and the lists only when they are given; a list left out is kept.
  async function saveMain(next: Partial<Defaults>) {
    defaultsBusy = true; defaultsError = '';
    try { defaults = await chat.mainAgent<Defaults>({ model: defaults.model, effort: defaults.effort, speed: defaults.speed, ...next }); }
    catch (cause) { defaultsError = cause instanceof Error ? cause.message : 'Could not save the main agent.'; }
    finally { defaultsBusy = false; }
  }

  // An agent a model wrote opens as a draft once the lists are loaded. With `replaces` it opens the
  // named agent with the new text; otherwise it is a new, inactive agent. Its suggested permissions
  // are only listed: each becomes a rule when the user approves it, one at a time, after saving.
  type Suggestion = { item: SuggestedPermission; state: 'waiting' | 'allowed' | 'dismissed'; error?: string };
  let mounted = $state(false), proposed = $state(''), suggestions = $state<Suggestion[]>([]);
  let approving = $state(false), rules = $state(0);
  $effect(() => {
    const proposal = chat.agentProposal;
    if (!proposal || !mounted || chat.agentsLoading || chat.skills.loading) return;
    chat.agentProposal = null;
    const old = proposal.replaces ? chat.agents.find(agent => agent.name.toLowerCase() === proposal.replaces!.toLowerCase()) : undefined;
    main = false;
    edit(old);
    const known = chat.models.find(model => model.model === proposal.model);
    const skills = proposal.skills.map(name => chat.skills.items.find(skill => skill.active && skill.name.toLowerCase() === name.toLowerCase()));
    draft.name = old ? old.name : proposal.name;
    draft.instructions = proposal.instructions;
    if (known) {
      draft.model = known.model;
      draft.effort = known.supportedReasoningEfforts.some(effort => effort.reasoningEffort === proposal.effort)
        ? proposal.effort : known.defaultReasoningEffort ?? null;
    }
    draft.skillIds = skills.filter(skill => !!skill).map(skill => skill.id);
    suggestions = proposal.permissions.map(item => ({ item, state: 'waiting' }));
    const missing = proposal.skills.filter((_, index) => !skills[index]);
    proposed = [old ? `This is a new version of ${old.name}, written by the agent. Saving replaces the current text.`
      : proposal.replaces ? `The agent meant to replace "${proposal.replaces}", which does not exist, so this is a new agent.`
      : 'Written by the agent. Review it, then save.',
      proposal.model && !known ? `The model ${proposal.model} is not available, so the model was left as it was.` : '',
      missing.length ? `Not ticked, because no active skill has that name: ${missing.join(', ')}.` : ''].filter(Boolean).join(' ');
  });

  // Turns one approved suggestion into one exact rule of this agent. An MCP server is named by the
  // model and found here by that name.
  async function approve(entry: Suggestion) {
    if (!id || approving) return;
    approving = true; entry.error = '';
    try {
      const item = entry.item;
      let target: PermissionTarget;
      if (item.kind === 'mcp_server') {
        const wanted = item.name;
        const server = (await chat.mcp<{ servers: McpServer[] }>('servers')).servers.find(server => server.name === wanted);
        if (!server) throw new Error(`No MCP server is named ${wanted}.`);
        target = { target_type: 'mcp_server', server: server.id };
      } else if (item.kind === 'feature') target = { target_type: 'feature', name: item.name };
      else if (item.kind === 'host') target = { target_type: 'host', host: item.host };
      else if (item.kind === 'domain') target = { target_type: 'domain', domain: item.domain };
      else if (item.kind === 'application') target = { target_type: 'application', id: item.id, action: item.action };
      else target = { target_type: item.kind, path: item.path, action: item.action };
      await chat.savePermission('agent', id, target, 'allow');
      entry.state = 'allowed';
      rules += 1;
    } catch (cause) {
      entry.error = cause instanceof Error ? cause.message : 'The permission was not saved.';
    } finally { approving = false; }
  }

  onMount(() => { opened(true); mounted = true; });
</script>

<section aria-label="Agents" class="flex min-h-0 flex-1 flex-col">
  <h2 class="flex h-12 shrink-0 items-center border-b border-border px-5 text-base font-semibold">Agents</h2>
  <div class="grid min-h-0 flex-1 grid-cols-1 overflow-y-auto sm:grid-cols-[180px_minmax(0,1fr)]">
    <nav class="border-b border-border p-3 sm:border-b-0 sm:border-r" aria-label="Agent profiles">
      <button class="flex min-h-9 w-full items-center gap-2 rounded px-2 py-2 text-left text-sm hover:bg-muted" class:bg-muted={main}
        disabled={chat.agentSaving} onclick={() => { main = true; void loadMain(); }}>
        <Bot size={15} class="shrink-0 text-muted-foreground" aria-hidden="true" /><span class="min-w-0 flex-1 truncate">Main agent</span>
      </button>
      <button class="flex min-h-9 items-center gap-2 px-2 text-sm text-brand"
        disabled={chat.agentSaving} onclick={() => { main = false; edit(); }}>
        <Plus size={16} aria-hidden="true" />New agent
      </button>
      <div class="max-h-28 overflow-y-auto sm:max-h-none">
        {#each chat.agents as agent (agent.id)}
          <button class="flex min-h-9 w-full items-center gap-2 rounded px-2 py-2 text-left text-sm
            hover:bg-muted" class:bg-muted={!main && id === agent.id}
            disabled={chat.agentSaving} onclick={() => { main = false; edit(agent); }}>
            <span class="min-w-0 flex-1 truncate">{agent.name}</span>
            <span class="size-1.5 shrink-0 rounded-full bg-border" class:bg-success={agent.active}
              aria-hidden="true"></span>
          </button>
        {/each}
      </div>
    </nav>
    {#if main}
    <div class="min-w-0 space-y-4 p-5 text-sm">
      <div>
        <h3 class="font-semibold">Main agent</h3>
        <p class="mt-1 text-xs text-muted-foreground">The agent of every conversation that has no other agent. A named agent has its own
          configuration and takes nothing from here.</p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Choice label="Default model" value={defaults.model ?? ''} placeholder="Provider's default"
          options={chat.models.map(model => ({ value: model.model, label: model.displayName }))}
          disabled={defaultsBusy || !!chat.modelError} onchange={value => saveMain({ model: value,
            effort: chat.models.find(model => model.model === value)?.defaultReasoningEffort ?? null, speed: null })} />
        <Choice label="Default reasoning effort" value={defaults.effort ?? ''} placeholder="Default effort"
          options={(chat.models.find(model => model.model === defaults.model)?.supportedReasoningEfforts ?? []).map(e =>
            ({ value: e.reasoningEffort, label: e.reasoningEffort }))}
          disabled={defaultsBusy || !defaults.model} onchange={value => saveMain({ model: defaults.model, effort: value })} />
        <Choice label="Default speed" value={defaults.speed ?? 'standard'} placeholder="Standard"
          options={[{ value: 'standard', label: 'Standard' }, ...tiers(defaults.model)]}
          disabled={defaultsBusy || !tiers(defaults.model).length}
          onchange={value => saveMain({ model: defaults.model, effort: defaults.effort, speed: value === 'standard' ? null : value })} />
        {#if defaults.model}
          <button type="button" class="text-xs text-brand underline-offset-2 hover:underline" disabled={defaultsBusy}
            onclick={() => saveMain({ model: null, effort: null, speed: null })}>Use the provider's default</button>
        {/if}
      </div>
      <p class="text-xs text-muted-foreground">A new conversation starts with this model unless you pick another in the composer.</p>
      {#if defaultsError}<p role="alert" class="text-xs text-destructive">{defaultsError}</p>{/if}
      <div class="space-y-3 border-t border-border pt-4">
        <div>
          <h3 class="font-semibold">Always-on skills</h3>
          <p class="mt-1 text-xs text-muted-foreground">Used in every conversation that has no other agent, old ones included. They are read
            when a message is sent, so a change here, or to a group's skills, applies from the next message. An inactive skill or
            group is left out and never blocks a message.</p>
        </div>
        <GroupChoices registry={chat.groups} bind:selected={defaults.skillGroupIds} disabled={defaultsBusy} label="Always-on skill groups"
          onchange={() => saveMain({ skillGroupIds: $state.snapshot(defaults.skillGroupIds) })} />
        <fieldset class="space-y-2 text-sm" disabled={defaultsBusy || chat.skills.loading}>
          <legend class="mb-2 font-medium">Always-on skills</legend>
          {#each chat.skills.items as skill (skill.id)}
            <label class="flex items-center gap-2">
              <input type="checkbox" value={skill.id} bind:group={defaults.skillIds} aria-label={skill.name}
                disabled={!defaults.skillIds.includes(skill.id) && defaults.skillIds.length >= 8}
                onchange={() => saveMain({ skillIds: $state.snapshot(defaults.skillIds) })} />
              <span class="min-w-0 break-words">{skill.name}</span>
              {#if !skill.active}<span class="text-xs text-muted-foreground">Inactive, left out</span>{/if}
            </label>
          {:else}<p class="text-muted-foreground">No skills</p>{/each}
        </fieldset>
      </div>
      <div class="space-y-2 border-t border-border pt-4">
        <div>
          <h3 class="font-semibold">Delegation</h3>
          <p class="mt-1 text-xs text-muted-foreground">A conversation with no other agent may hand a task to any active agent. You are
            asked each time. These limits apply from the next request, in old conversations too.</p>
        </div>
        <DelegationLimits value={defaults.delegation} disabled={defaultsBusy} onchange={next => saveMain({ delegation: next })} />
      </div>
      <AgentCapabilities {chat} id={MAIN} capabilitiesOnly />
    </div>
    {:else}
    <form class="min-w-0 space-y-4 p-5" onsubmit={save}>
      {#if proposed}<p role="note" class="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs">{proposed}</p>{/if}
      <label class="block text-sm">Name
        <Input class="mt-1" bind:value={draft.name} maxlength={80}
          required disabled={chat.agentSaving} oninput={() => saved = false} />
      </label>
      <Instructions bind:value={draft.instructions} disabled={chat.agentSaving}
        onchange={() => saved = false} />
      <fieldset class="space-y-2 text-sm" disabled={chat.agentSaving || chat.skills.loading}>
        <legend class="mb-1">Skills</legend>
        {#each chat.skills.items as skill (skill.id)}
          <label class="flex items-center gap-2">
            <input type="checkbox" value={skill.id} bind:group={draft.skillIds}
              disabled={!draft.skillIds.includes(skill.id) && (!skill.active || draft.skillIds.length >= 8)}
              onchange={() => saved = false} />
            <span class="min-w-0 break-words">{skill.name}</span>
            {#if !skill.active}<span class="text-xs text-muted-foreground">Inactive</span>{/if}
          </label>
        {/each}
        {#each draft.skillIds.filter(id => !chat.skills.items.some(s => s.id === id)) as missing}
          <label class="flex items-center gap-2">
            <input type="checkbox" value={missing} bind:group={draft.skillIds}
              onchange={() => saved = false} />Unavailable skill
          </label>
        {/each}
        {#if !chat.skills.items.length && !draft.skillIds.length}
          <p class="text-xs text-muted-foreground">{chat.skills.loading ? 'Loading skills...' : 'No skills'}</p>
        {/if}
        {#if chat.skills.error}<p role="alert" class="text-xs text-destructive">{chat.skills.error}</p>{/if}
      </fieldset>
      <GroupChoices registry={chat.groups} bind:selected={draft.skillGroupIds}
        disabled={chat.agentSaving} onchange={() => saved = false} />
      <div class="flex flex-wrap items-center gap-2">
        <Choice label="Agent model" value={draft.model ?? ''} placeholder="Default model"
          options={chat.models.map(model => ({ value: model.model, label: model.displayName }))}
          disabled={chat.agentSaving || !!chat.modelError} onchange={chooseModel} />
        <Choice label="Agent reasoning effort" value={draft.effort ?? ''} placeholder="Default effort"
          options={(model?.supportedReasoningEfforts ?? []).map(e =>
            ({ value: e.reasoningEffort, label: e.reasoningEffort }))}
          disabled={chat.agentSaving || !model?.supportedReasoningEfforts.length}
          onchange={value => { draft.effort = value; saved = false; }} />
        <Choice label="Agent speed" value={draft.speed ?? 'standard'} placeholder="Standard"
          options={[{ value: 'standard', label: 'Standard' }, ...tiers(draft.model)]}
          disabled={chat.agentSaving || !tiers(draft.model).length}
          onchange={value => { draft.speed = value === 'standard' ? null : value; saved = false; }} />
      </div>
      <!-- Settings from an older design that nothing can enforce. They are shown only when an agent still carries them. -->
      {#if !supported}
        <dl class="space-y-2 border-y border-border py-3 text-xs">
          <div><dt class="text-muted-foreground">Settings from an older design</dt>
            <dd class="break-words [overflow-wrap:anywhere]">{[
              ...draft.capabilities.map(c => c === 'computer_use' ? 'Computer Use' : 'Deep Research'),
              ...draft.access.applications.map(a => `${a.id}: ${a.actions.join(', ')}`),
              ...draft.access.files.map(f => `${f.path}: ${[f.read ? 'read' : '', f.write ? 'write' : ''].filter(Boolean).join(', ')}`),
              ...draft.access.network].join('; ')}</dd></div>
        </dl>
      {/if}
      <fieldset class="space-y-2 text-sm">
        <legend class="font-medium">May delegate to</legend>
        <p class="text-xs text-muted-foreground">This agent may hand a task to the agents ticked here. You are asked each time
          whether to wait for the result. A delegated agent uses its own permissions and cannot delegate further.</p>
        {#each recipients as agent (agent.id)}
          <label class="flex items-center gap-2"><input type="checkbox" value={agent.id} bind:group={draft.delegatesTo}
            disabled={chat.agentSaving} onchange={() => saved = false} />{agent.name}{agent.active ? '' : ' (inactive)'}</label>
        {:else}<p class="text-muted-foreground">No other agents</p>{/each}
        {#if draft.delegatesTo.length}
          <DelegationLimits value={draft.delegation} disabled={chat.agentSaving}
            onchange={next => { draft.delegation = next; saved = false; }} />
        {/if}
      </fieldset>
      {#if suggestions.length}
        <section aria-label="Suggested permissions" class="space-y-2 rounded-md border border-border p-3 text-sm">
          <h3 class="font-medium">Suggested by the agent that wrote this</h3>
          <p class="text-xs text-muted-foreground">{id ? 'None of these is granted. Approve the ones you agree with, one at a time; each becomes one rule below, which you can remove again.'
            : 'None of these is granted. Save the agent first, then approve each one you agree with.'}</p>
          <ul class="space-y-2">
            {#each suggestions as entry, index (index)}
              <li class="flex flex-wrap items-start gap-x-3 gap-y-1 border-t border-border pt-2 first:border-t-0 first:pt-0">
                <div class="min-w-0 flex-1">
                  <p class="break-words font-medium [overflow-wrap:anywhere]">{permissionLabel(entry.item)}</p>
                  <p class="break-words text-xs text-muted-foreground">{entry.item.reason}</p>
                  {#if entry.error}<p role="alert" class="text-xs text-destructive">{entry.error}</p>{/if}
                </div>
                {#if entry.state === 'allowed'}<span class="text-xs text-success">Allowed</span>
                {:else if entry.state === 'dismissed'}<span class="text-xs text-muted-foreground">Not allowed</span>
                {:else}
                  <Button type="button" size="sm" variant="outline" disabled={!id || approving || chat.agentSaving}
                    aria-label={`Allow: ${permissionLabel(entry.item)}`} onclick={() => approve(entry)}>Allow</Button>
                  <Button type="button" size="sm" variant="ghost" disabled={approving}
                    aria-label={`Do not allow: ${permissionLabel(entry.item)}`} onclick={() => { entry.state = 'dismissed'; }}>No</Button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/if}
      {#if id}{#key rules}<AgentCapabilities {chat} {id} />{/key}
      {:else}<p class="text-xs text-muted-foreground">Save the agent to set what it may use.</p>{/if}
      {#if !supported}
        <p class="text-xs text-destructive">This access policy cannot yet be enforced. Activation is unavailable.</p>
      {/if}
      <label class="flex items-center gap-2 text-sm">
        <input type="checkbox" bind:checked={draft.active}
          disabled={chat.agentSaving || !supported} onchange={() => saved = false} />Active
      </label>
      {#if chat.agentError}
        <div class="flex items-center gap-2 text-sm text-destructive">
          <p role="alert" class="min-w-0 break-words">{chat.agentError}</p>
          <button type="button" class="icon-button" aria-label="Reload agents"
            disabled={chat.agentSaving || chat.agentsLoading} onclick={() => chat.refreshAgents()}>
            <RefreshCw size={16} aria-hidden="true" />
          </button>
        </div>
      {/if}
      <div class="flex flex-wrap items-center justify-between gap-3">
        {#if id && draft.revision}
          <ConfirmDelete label="Delete agent" actionLabel="Delete agent" triggerText="Delete"
            disabled={chat.agentSaving}
            description={`Delete ${draft.name}? Its chats stay readable, but cannot continue. Pinned instructions remain with those chats.`}
            action={async () => { await chat.deleteAgent(id!, draft.revision!); edit(); }} />
        {/if}
        <span role="status" class="text-xs text-success">{saved ? 'Saved' : ''}</span>
        <Button type="submit" disabled={chat.agentSaving || !chat.ready ||
      !draft.name.trim() || !draft.instructions.trim()}>
          <Save size={16} aria-hidden="true" />Save agent
        </Button>
      </div>
    </form>
    {/if}
  </div>
</section>
