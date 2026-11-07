<script lang="ts">
  // Words the page for the machine MiniClaw runs on.
  import { words } from './platform.svelte';
  // Styled text fields from the shared component library.
  // Edits the project's standing instructions as live Markdown.
  import Instructions from './Instructions.svelte';
  // Lists and edits the notes the project's conversations share.
  import ProjectMemory from './ProjectMemory.svelte';
  // Chooses which issues agents may close alone.
  import IssueSettings from './IssueSettings.svelte';
  import { Input } from '$lib/components/ui/input/index.js';
  // Styled button from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  // Shows the project VM's state, power buttons and idle time.
  import VmControls from './VmControls.svelte';
  // Confirms project deletion, with or without its VM.
  import ProjectDelete from './ProjectDelete.svelte';
  // Keeps project settings focus and tooltips accessible.
  import { AlertDialog, Dialog, Tooltip } from 'bits-ui';
  // Identifies project creation, settings, and saving.
  import Plus from '@lucide/svelte/icons/plus';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import X from '@lucide/svelte/icons/x';
  import Save from '@lucide/svelte/icons/save';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Shares project drafts and conversation readiness.
  import type { Project, ProjectDraft, VmSetup } from './project-state.svelte';
  import type { Chat } from './chat.svelte';
  // Carries observed skill revisions into confirmation.
  import type { Skill } from './skill-state.svelte';
  // Selects groups independently from individual skills.
  import GroupChoices from './GroupChoices.svelte';
  // Edits project-wide defaults beside other settings.
  import PermissionSettings from './PermissionSettings.svelte';

  let { chat, project }: { chat: Chat; project?: Project } = $props();
  let open = $state(false), saved = $state(false);
  let id = $state<string>();
  let machineLocked = $state(false);
  let setupVmId = $state('');
  let setupRequestId = '';
  let setupRunning = $state(false);
  let setupStage = $state('');
  let setupPercent = $state(0);
  let setupWorkerConnected = $state(false);
  let setupError = $state('');
  let setupAuthError = $state('');
  let setupAuthCode = $state('');
  let setupAuthUrl = $state('');
  let setupAuthPending = $state(false);
  let setupEpoch = 0;
  let activation = $state<Skill>();
  let activationError = $state('');
  let draft = $state<ProjectDraft>({ name: '', agentIds: [], skillIds: [],
    environment: { kind: 'host' } });
  const label = $derived(project ? `Settings for ${project.name}` : 'New project');
  // The editor's text, and the instruction files found in the project's folder.
  let instructions = $state('');
  let rules = $state<{ name: string; bytes: number }[]>([]), rulesCut = $state(false);

  // Opens an observed revision and refreshes available agents and owned skills.
  function opened(value: boolean) {
    if (chat.skills.saving || (!value && setupRunning)) return;
    open = value;
    activation = undefined;
    if (!value) return;
    id = project?.id;
    machineLocked = !!project?.environment || !!setupVmId;
    const unfinished = !project && setupVmId ? $state.snapshot(draft) : null;
    draft = project ? { name: project.name, agentIds: [...project.agentIds],
      skillIds: [...project.skillIds], skillGroupIds: [...(project.skillGroupIds ?? [])],
      environment: project.environment, workspace: project.workspace, instructions: project.instructions ?? '', revision: project.revision }
      : unfinished ?? { name: '', agentIds: [], skillIds: [], skillGroupIds: [],
        environment: { kind: 'host' } };
    instructions = draft.instructions ?? '';
    rules = []; rulesCut = false;
    if (id) void chat.projects.rules(id).then(found => { rules = found.files; rulesCut = found.cut; }, () => {});
    saved = false; chat.projects.error = ''; setupError = '';
    setupAuthError = ''; setupAuthCode = ''; setupAuthUrl = '';
    void chat.refreshAgents(); void chat.skills.refresh(); void chat.groups.refresh();
  }

  // Commits project settings only after the selected VM is ready.
  async function commit() {
    const result = await chat.projects.save(id, $state.snapshot(draft));
    if (result) { id = result.id; draft.revision = result.revision;
      machineLocked = true; saved = true; setupVmId = ''; setupRequestId = '';
      setupStage = ''; setupPercent = 0; }
  }

  // Polls native installation and worker sign-in without starting another machine.
  async function waitForVm(epoch: number) {
    for (let attempt = 0; attempt < 1800; attempt++) {
      const status: VmSetup = await chat.projects.vmStatus(setupVmId);
      if (epoch !== setupEpoch) return false;
      setupStage = status.stage ?? '';
      setupWorkerConnected = status.workerConnected === true;
      setupPercent = Number.isSafeInteger(status.percent) && status.percent! >= 0
        && status.percent! <= 100 ? status.percent! : 0;
      if (status.state === 'failed' || status.state === 'missing'
        || status.state === 'unknown' || status.state === 'off') {
        throw new Error(status.reason ?? 'Project VM stopped before setup completed.');
      }
      if (status.state === 'running' && status.ready === true) return true;
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    throw new Error('Project VM did not become ready. Its files were kept for retry.');
  }

  // Starts a fresh VM once, waits for readiness, and only then saves the project.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (setupRunning) return;
    if (id || draft.environment?.kind !== 'vm' || !draft.environment.dedicated) {
      await commit();
      return;
    }
    const epoch = ++setupEpoch;
    setupRunning = true; setupError = ''; saved = false;
    try {
      if (!setupVmId) {
        setupRequestId ||= crypto.randomUUID();
        const started = await chat.projects.provisionVm(setupRequestId);
        if (epoch !== setupEpoch) return;
        setupVmId = started.vmId;
        draft.environment = { kind: 'vm', dedicated: true, vmId: setupVmId };
        machineLocked = true;
      }
      if (await waitForVm(epoch)) await commit();
    } catch (error) {
      if (epoch === setupEpoch) setupError = error instanceof Error
        ? error.message : 'Project VM setup failed.';
    } finally { if (epoch === setupEpoch) setupRunning = false; }
  }

  // Reuses the retained VM ID while creating a new installation attempt.
  async function retrySetup() {
    if (!setupVmId || setupRunning) return;
    const epoch = ++setupEpoch;
    setupRunning = true; setupError = '';
    try {
      await chat.projects.retryVm(setupVmId);
      if (await waitForVm(epoch)) await commit();
    } catch (error) {
      if (epoch === setupEpoch) setupError = error instanceof Error
        ? error.message : 'Project VM setup failed.';
    } finally { if (epoch === setupEpoch) setupRunning = false; }
  }

  // Cancels this setup attempt but keeps the guest for retry.
  async function cancelSetup() {
    if (!setupVmId || !setupRunning) return;
    ++setupEpoch;
    try {
      const status = await chat.projects.cancelVm(setupVmId);
      setupStage = status.stage ?? 'failed';
      setupError = status.reason ?? 'VM setup was cancelled. Its files were kept for retry.';
    } catch (error) {
      setupError = error instanceof Error ? error.message : 'Could not cancel VM setup.';
    } finally { setupRunning = false; }
  }

  // Shows this guest's Codex device code without handling credentials in MiniClaw.
  async function signInVm() {
    if (!setupVmId || !setupWorkerConnected || setupAuthPending) return;
    setupAuthPending = true; setupAuthError = '';
    try {
      const login = await chat.projects.startVmAuth(setupVmId);
      if (login.status !== 'pending'
        || login.verificationUrl !== 'https://auth.openai.com/codex/device'
        || !/^[A-Z0-9-]{4,32}$/.test(login.userCode)) {
        throw new Error('Project VM returned an invalid Codex sign-in code.');
      }
      setupAuthCode = login.userCode;
      setupAuthUrl = login.verificationUrl;
    } catch (error) {
      setupAuthError = error instanceof Error ? error.message : 'Could not start Codex sign-in.';
    } finally { setupAuthPending = false; }
  }

  // Selects active skills immediately; inactive additions require explicit activation consent.
  function chooseSkill(skill: Skill, input: HTMLInputElement) {
    const selected = draft.skillIds.includes(skill.id);
    if (!selected && !skill.active) {
      input.checked = false;
      activation = { ...skill };
      activationError = '';
      return;
    }
    draft.skillIds = selected ? draft.skillIds.filter(id => id !== skill.id)
      : [...draft.skillIds, skill.id];
    saved = false;
  }

  // Activates the observed revision and adds it only after backend acknowledgement.
  async function activateSkill() {
    if (!activation || !chat.ready || chat.skills.saving || draft.skillIds.length >= 8) return;
    const { id, ...definition } = activation;
    activationError = '';
    const skill = await chat.skills.save(id, { ...definition, active: true });
    if (!skill) {
      activationError = chat.skills.error || 'Could not activate skill. Reopen settings and try again.';
      return;
    }
    draft.skillIds = [...draft.skillIds.filter(value => value !== id), id];
    saved = false;
    activation = undefined;
  }

  // Reloads current settings after a conflict without silently replacing unsaved edits.
  async function reload() {
    await chat.projects.refresh();
    const current = chat.projects.items.find(p => p.id === id);
    if (current) {
      draft = { name: current.name, agentIds: [...current.agentIds],
        skillIds: [...current.skillIds], skillGroupIds: [...(current.skillGroupIds ?? [])],
        environment: current.environment, workspace: current.workspace, instructions: current.instructions ?? '', revision: current.revision };
      machineLocked = !!current.environment;
      saved = false;
    }
  }
</script>

<Dialog.Root {open} onOpenChange={opened}>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Dialog.Trigger {...props} class="icon-button shrink-0" aria-label={label}
          disabled={!chat.ready || chat.projects.saving}>
          {#if project}<Settings2 size={15} aria-hidden="true" />{:else}<Plus size={16} aria-hidden="true" />{/if}
        </Dialog.Trigger>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content class="tooltip" sideOffset={8}>{label}</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-1/2 z-50 flex max-h-[90dvh] w-[min(480px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-background shadow-lg">
      <div class="flex items-center justify-between border-b border-border px-5 py-3">
        <Dialog.Title class="text-base font-semibold">Project settings</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close project settings"
          disabled={chat.projects.saving || setupRunning}>
          <X size={18} aria-hidden="true" />
        </Dialog.Close>
      </div>
      <form class="min-h-0 space-y-5 overflow-y-auto p-5" onsubmit={save}>
        <label class="block text-sm">Name
          <Input class="mt-1" bind:value={draft.name} maxlength={80} required
            disabled={chat.projects.saving || setupRunning} oninput={() => saved = false} />
        </label>
        <label class="block text-sm">Workspace folder
          <Input class="mt-1" aria-label="Workspace folder" maxlength={4096}
            placeholder={words.workspacePlaceholder}
            value={draft.workspace ?? ''} disabled={chat.projects.saving || setupRunning}
            oninput={event => { draft.workspace = event.currentTarget.value.trim() || undefined;
              saved = false; }} />
          <span class="mt-1 block text-xs text-muted-foreground">Optional. Chats in this project can read and
            write here without asking. Use the full path of an existing folder.</span>
        </label>
        <div>
          <Instructions label="Project instructions" bind:value={instructions} disabled={chat.projects.saving || setupRunning}
            onchange={() => { draft.instructions = instructions; saved = false; }} />
          <p class="mt-1 text-xs text-muted-foreground">Told to every conversation in this project, new and reopened, with Codex and with
            Claude. {#if rules.length}Also read from the folder: {rules.map(file => `${file.name} (${file.bytes < 1024 ? `${file.bytes} B` : `${Math.round(file.bytes / 1024)} KB`})`).join(', ')}{rulesCut ? ', cut at 32 KiB' : ''}.
            {:else if draft.workspace && id}No AGENTS.md or CLAUDE.md was found in the folder.{:else}An AGENTS.md or CLAUDE.md in the folder is read too.{/if}</p>
        </div>
        <fieldset class="space-y-2 border-y border-border py-3 text-sm"
          disabled={chat.projects.saving || setupRunning || machineLocked}>
          <legend class="font-medium">Runs on</legend>
          <label class="flex items-center gap-2">
            <input type="radio" name="project-machine" checked={draft.environment?.kind === 'host'}
              onchange={() => { draft.environment = { kind: 'host' }; saved = false; }} />Mac
          </label>
          <label class="flex items-center gap-2">
            <input type="radio" name="project-machine" checked={draft.environment?.kind === 'vm'}
              onchange={() => { draft.environment = { kind: 'vm', dedicated: false }; saved = false; }} />Virtual machine
          </label>
          {#if draft.environment?.kind === 'vm'}
            <label class="ml-6 flex items-center gap-2 text-muted-foreground">
              <input type="checkbox" checked={draft.environment.dedicated}
                onchange={event => { draft.environment = { kind: 'vm',
                  dedicated: event.currentTarget.checked }; saved = false; }} />Use a separate VM
            </label>
            {#if !draft.environment.dedicated}
              <p class="ml-6 text-xs text-muted-foreground">Uses Master</p>
            {/if}
          {/if}
        </fieldset>
        {#if setupVmId}
          <div class="rounded-md border border-border bg-muted px-3 py-3 text-sm">
            <p role="status" aria-label="Project VM setup status" class="font-medium">
              {setupStage === 'installing' ? 'Installing macOS'
                : setupStage === 'awaiting_setup' ? 'Waiting for guest setup and Codex sign-in'
                : setupStage === 'failed' ? 'Project VM setup stopped'
                : 'Preparing project VM'}
            </p>
            <progress aria-label="Project VM setup progress" max="100" value={setupPercent}
              class="mt-2 h-2 w-full accent-ink"></progress>
            {#if setupStage === 'awaiting_setup'}
              <p class="mt-2 text-xs text-muted-foreground">Finish macOS setup in the VM window. Open MiniClawSetup,
                and run Install Codex.command.</p>
              {#if setupWorkerConnected && !setupAuthCode}
                <Button variant="secondary" class="mt-3" type="button" disabled={setupAuthPending}
                  onclick={signInVm}>Sign in to Codex for this VM</Button>
              {/if}
              {#if setupAuthCode}
                <p class="mt-3 text-xs text-muted-foreground">Enter this code at Codex sign-in:</p>
                <p class="mt-1 font-mono text-base">{setupAuthCode}</p>
                <a href={setupAuthUrl} target="_blank" rel="noopener noreferrer"
                  class="mt-2 inline-block underline" aria-label="Open Codex sign-in">Open Codex sign-in</a>
              {/if}
              {#if setupAuthError}<p role="alert" class="mt-2 text-xs text-destructive">{setupAuthError}</p>{/if}
            {/if}
            {#if setupError}<p role="alert" class="mt-2 text-xs text-destructive">{setupError}</p>{/if}
            {#if setupRunning}
              <Button variant="secondary" class="mt-3" type="button" onclick={cancelSetup}>Cancel VM setup</Button>
            {:else if setupError && !id}
              <Button variant="secondary" class="mt-3" type="button" onclick={retrySetup}>Retry VM setup</Button>
            {/if}
          </div>
        {/if}
        {#if setupError && !setupVmId}<p role="alert" class="text-xs text-destructive">{setupError}</p>{/if}
        <fieldset class="space-y-2 text-sm" disabled={chat.projects.saving || setupRunning || chat.agentsLoading}>
          <legend class="mb-2 font-medium">Agents</legend>
          <label class="flex items-center gap-2 text-muted-foreground"><input type="checkbox" checked disabled />MiniClaw</label>
          {#each chat.agents as agent (agent.id)}
            <label class="flex items-center gap-2">
              <input type="checkbox" value={agent.id} bind:group={draft.agentIds}
                disabled={!agent.active && !draft.agentIds.includes(agent.id)} onchange={() => saved = false} />
              <span class="min-w-0 break-words">{agent.name}</span>
              {#if !agent.active}<span class="text-xs text-muted-foreground">Inactive</span>{/if}
            </label>
          {/each}
          {#each draft.agentIds.filter(id => !chat.agents.some(a => a.id === id)) as missing}
            <label class="flex items-center gap-2"><input type="checkbox" value={missing}
              bind:group={draft.agentIds} />Unavailable agent</label>
          {/each}
          {#if chat.agentError}<p role="alert" class="text-destructive">{chat.agentError}</p>{/if}
        </fieldset>
        <fieldset class="space-y-2 text-sm"
          disabled={chat.projects.saving || setupRunning || chat.skills.loading || chat.skills.saving}>
          <legend class="mb-2 font-medium">MiniClaw skills</legend>
          {#each chat.skills.items as skill (skill.id)}
            <label class="flex items-center gap-2">
              <input type="checkbox" value={skill.id} checked={draft.skillIds.includes(skill.id)}
                disabled={!draft.skillIds.includes(skill.id) && draft.skillIds.length >= 8}
                onchange={event => chooseSkill(skill, event.currentTarget)} />
              <span class="min-w-0 break-words">{skill.name}</span>
              {#if !skill.active}<span class="text-xs text-muted-foreground">Inactive</span>{/if}
            </label>
          {/each}
          {#each draft.skillIds.filter(id => !chat.skills.items.some(s => s.id === id)) as missing}
            <label class="flex items-center gap-2"><input type="checkbox" value={missing}
              bind:group={draft.skillIds} />Unavailable skill</label>
          {/each}
          {#if !chat.skills.items.length && !draft.skillIds.length}<p class="text-muted-foreground">No skills</p>{/if}
          {#if chat.skills.error}<p role="alert" class="text-destructive">{chat.skills.error}</p>{/if}
        </fieldset>
        <GroupChoices registry={chat.groups} bind:selected={draft.skillGroupIds}
          label="MiniClaw skill groups" disabled={chat.projects.saving || setupRunning} onchange={() => saved = false} />
        <!-- Notes belong to a saved project; they are changed at once, not with Save. -->
        {#if id}<ProjectMemory projects={chat.projects} {id} />{/if}
        <!-- The review level is also saved at once. -->
        {#if id}<IssueSettings projects={chat.projects} {id} />{/if}
        {#if project && machineLocked && project.environment?.kind === 'vm'
          && project.environment.dedicated && project.environment.vmId}
          <VmControls {chat} vmId={project.environment.vmId} name="Project VM" />
        {/if}
        {#if id}<PermissionSettings {chat} kind="project" {id} />{/if}
        {#if chat.projects.error}
          <div class="flex items-center gap-2 text-sm text-destructive">
            <p role="alert" class="min-w-0 break-words">{chat.projects.error}</p>
            <button type="button" class="icon-button" aria-label="Reload project settings"
              disabled={chat.projects.loading || chat.projects.saving} onclick={reload}>
              <RefreshCw size={16} aria-hidden="true" />
            </button>
          </div>
        {/if}
        <div class="flex items-center justify-between gap-3">
          {#if project}<ProjectDelete {chat} {project} ondeleted={() => { open = false; }} />{/if}
          <span role="status" aria-label="Project save status" class="min-w-0 flex-1 text-xs text-success">{saved ? 'Saved' : ''}</span>
          <Button type="submit" 
      disabled={!chat.ready || chat.projects.saving || setupRunning || !draft.name.trim() || !draft.environment}>
            <Save size={16} aria-hidden="true" />Save project
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<AlertDialog.Root open={!!activation}
  onOpenChange={value => { if (!value && !chat.skills.saving) activation = undefined; }}>
  <AlertDialog.Portal>
    <AlertDialog.Overlay class="fixed inset-0 z-[60] bg-black/30" />
    <AlertDialog.Content escapeKeydownBehavior={chat.skills.saving ? 'ignore' : 'close'}
      class="fixed left-1/2 top-1/2 z-[70] w-[min(400px,calc(100%-24px))]
      -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-background p-5 shadow-lg">
      <AlertDialog.Title class="text-base font-semibold">Activate skill?</AlertDialog.Title>
      <AlertDialog.Description class="mt-2 break-words text-sm text-muted-foreground">
        Activate {activation?.name} across MiniClaw and add it to this project's selection?
        Other chats already using this skill may continue. Activation stays in effect
        even if you don't save this project.
      </AlertDialog.Description>
      {#if activationError}<p role="alert" class="mt-3 text-sm text-destructive">{activationError}</p>{/if}
      <div class="mt-5 flex flex-wrap justify-end gap-3">
        <AlertDialog.Cancel class="rounded-md border border-border px-3 py-2 text-sm"
          disabled={chat.skills.saving}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action class="rounded-md bg-primary px-3 py-2 text-sm text-primary-foreground disabled:opacity-50"
          disabled={!chat.ready || chat.skills.saving} onclick={activateSkill}>Activate and add</AlertDialog.Action>
      </div>
    </AlertDialog.Content>
  </AlertDialog.Portal>
</AlertDialog.Root>
