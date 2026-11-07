<script lang="ts">
  // Loads the VM budget when Settings shows this panel.
  import { onMount } from 'svelte';
  // Styled buttons and fields from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  // Shows one VM's state, power buttons and idle time.
  import VmControls from './VmControls.svelte';
  // Confirms the permanent deletion of an orphaned VM.
  import ConfirmDelete from './ConfirmDelete.svelte';
  // Reaches the virtualisation routes through the app's request helper.
  import type { Chat } from './chat.svelte';

  type Guest = { id: string; state: string; cpuCount: number; memoryBytes: number };
  type Amounts = { cpuCount: number; memoryBytes: number; diskBytes: number };
  type ProjectVm = { vmId: string; state: string; projectId: string | null; projectName: string | null };
  type View = { available: boolean; workerCurrent?: boolean | null; projectVms?: ProjectVm[];
    master?: { state: string; ready: boolean; signedIn?: boolean; computerReady?: boolean; reason?: string };
    ledger?: { vms: Guest[]; limits: { cpuPercent: number; memoryPercent: number; diskPercent: number } | null;
      capacity: Amounts | null; used: Amounts | null; exceeded: string[];
      released?: string[]; refused?: { id: string; reason: string }[]; forgotten?: string[] } };

  let { chat }: { chat: Chat } = $props();
  let view = $state<View | null>(null);
  let busy = $state(false), error = $state(''), note = $state('');
  let cpu = $state(50), memory = $state(50), disk = $state(25);

  const gib = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
  const unknown = $derived((view?.ledger?.vms ?? []).filter(vm => vm.state === 'unknown').length);

  // Shows one reply and copies its limits into the form.
  function show(next: View) {
    view = next;
    const limits = next.ledger?.limits;
    if (limits) { cpu = limits.cpuPercent; memory = limits.memoryPercent; disk = limits.diskPercent; }
  }

  // Runs one request against the virtualisation routes and reports a failure in words.
  async function run(action: '' | 'limits' | 'reconcile' | 'forget', body?: unknown,
    done?: (next: View) => string) {
    busy = true; error = ''; note = '';
    try {
      const next = await chat.virtualisation<View>(action, body);
      show(next);
      note = done?.(next) ?? '';
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'The request failed.';
    } finally { busy = false; }
  }

  onMount(() => { void run(''); });
</script>

<section aria-label="Virtualisation" class="flex min-h-0 flex-1 flex-col overflow-y-auto">
  <h2 class="sticky top-0 z-[5] flex h-12 shrink-0 items-center border-b border-border bg-background px-5 text-base font-semibold">Virtualisation</h2>
  <div class="space-y-6 p-5 text-sm">
    {#if view && !view.available}
      <p class="text-muted-foreground">This installation has no VM. Run <code class="rounded bg-muted px-1">miniclaw setup</code>
        and choose the VM to add one.</p>
    {:else if view?.ledger}
      <div>
        <VmControls {chat} vmId="master" name="Master VM" state={view.master?.state}
          note={view.master?.state === 'running' && !view.master.ready ? 'waiting for login' : view.master?.reason ?? ''} />
        <ul class="mt-2 space-y-1 text-xs text-muted-foreground">
          <li>Codex sign-in in the VM: {view.master?.signedIn ? 'signed in' : 'not confirmed while the VM is off'}</li>
          <li>Computer Use helper: {view.master?.computerReady ? 'installed' : 'not reported'}</li>
        </ul>
        {#if view.workerCurrent === false}
          <div role="status" class="mt-3 rounded-md border border-border bg-muted p-3 text-xs">
            <p class="font-medium text-foreground">This VM's worker is older than this MiniClaw.</p>
            <p class="mt-1 text-muted-foreground">Shut the VM down, run <code class="rounded bg-background px-1">miniclaw update-vm</code>
              in a terminal, start the VM, then run "Install Codex.command" from the MiniClawSetup disk inside it.</p>
          </div>
        {:else if view.workerCurrent}
          <p class="mt-2 text-xs text-muted-foreground">The VM's worker is up to date.</p>
        {/if}
      </div>

      {#if view.projectVms?.length}
        <div>
          <h3 class="font-medium">Project VMs</h3>
          <div class="mt-2 space-y-2">
            {#each view.projectVms as vm (vm.vmId)}
              <VmControls {chat} vmId={vm.vmId} state={vm.state}
                name={vm.projectName ?? `Orphaned VM ${vm.vmId.slice(0, 8)}`}
                note={vm.projectId ? '' : 'Its project was deleted.'}>
                {#snippet children(current)}
                  {#if !vm.projectId}
                    <ConfirmDelete label="Delete VM" actionLabel="Delete VM" triggerText="Delete VM"
                      disabled={!['off', 'failed'].includes(current)}
                      description="This deletes the VM's disk with every file, app and sign-in inside it. It cannot be undone and nothing goes to the Trash. Conversations that ran in this VM stay listed but can no longer continue."
                      action={async () => { await chat.deleteVm(vm.vmId); await run(''); }} />
                  {/if}
                {/snippet}
              </VmControls>
            {/each}
          </div>
          <p class="mt-2 text-xs text-muted-foreground">A VM that belongs to a project is deleted with its project.</p>
        </div>
      {/if}

      <div>
        <h3 class="font-medium">Share of this Mac for all VMs</h3>
        <p class="mt-0.5 text-xs text-muted-foreground">One budget is shared by every MiniClaw VM and installation on this Mac.
          Lowering it never stops a VM or deletes a file; it only refuses later starts.</p>
        <form class="mt-3 grid grid-cols-3 gap-3" onsubmit={event => { event.preventDefault();
          void run('limits', { cpuPercent: cpu, memoryPercent: memory, diskPercent: disk },
            () => 'Limits saved.'); }}>
          <label class="block text-xs">CPU %
            <Input class="mt-1" type="number" min="1" max="100" aria-label="CPU percent" bind:value={cpu} /></label>
          <label class="block text-xs">Memory %
            <Input class="mt-1" type="number" min="1" max="100" aria-label="Memory percent" bind:value={memory} /></label>
          <label class="block text-xs">Disk %
            <Input class="mt-1" type="number" min="1" max="100" aria-label="Disk percent" bind:value={disk} /></label>
          <div class="col-span-3"><Button type="submit" size="sm" disabled={busy}>Save limits</Button></div>
        </form>
        {#if view.ledger.capacity && view.ledger.used}
          <dl class="mt-3 grid grid-cols-3 gap-3 text-xs">
            <div><dt class="text-muted-foreground">CPU in use</dt>
              <dd>{view.ledger.used.cpuCount} of {view.ledger.capacity.cpuCount}</dd></div>
            <div><dt class="text-muted-foreground">Memory in use</dt>
              <dd>{gib(view.ledger.used.memoryBytes)} of {gib(view.ledger.capacity.memoryBytes)}</dd></div>
            <div><dt class="text-muted-foreground">Disk reserved</dt>
              <dd>{gib(view.ledger.used.diskBytes)} of {gib(view.ledger.capacity.diskBytes)}</dd></div>
          </dl>
        {/if}
        {#if view.ledger.exceeded.length}
          <p role="status" class="mt-2 text-xs text-destructive">Over budget: {view.ledger.exceeded.join(', ')}. New VM starts are refused until this is resolved.</p>
        {/if}
      </div>

      <div>
        <h3 class="font-medium">Recorded VMs</h3>
        {#if view.ledger.vms.length}
          <div class="mt-2 overflow-x-auto">
            <table class="w-full text-left text-xs">
              <thead class="text-muted-foreground"><tr><th class="py-1 pr-3 font-normal">State</th>
                <th class="py-1 pr-3 font-normal">CPU</th><th class="py-1 pr-3 font-normal">Memory</th>
                <th class="py-1 font-normal">Folder</th></tr></thead>
              <tbody>
                {#each view.ledger.vms as vm (vm.id)}
                  <tr class="border-t border-border"><td class="py-1.5 pr-3">{vm.state}</td>
                    <td class="py-1.5 pr-3">{vm.cpuCount}</td><td class="py-1.5 pr-3">{gib(vm.memoryBytes)}</td>
                    <td class="break-all py-1.5">{vm.id}</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}
          <p class="mt-1 text-xs text-muted-foreground">No VM has been reserved on this Mac yet.</p>
        {/if}
        <div class="mt-3 flex flex-wrap gap-2">
          <Button variant="secondary" size="sm" disabled={busy || !unknown}
            title="Releases CPU and memory of a VM whose supervisor died, after checking that nothing uses it"
            onclick={() => run('reconcile', {}, next =>
              `Released ${next.ledger?.released?.length ?? 0}. ${(next.ledger?.refused ?? []).map(item => `${item.id}: ${item.reason}`).join(' ')}`)}>
            Release idle VMs{unknown ? ` (${unknown})` : ''}</Button>
          <Button variant="secondary" size="sm" disabled={busy}
            title="Drops a stopped VM whose folder or disk you deleted, so its disk budget is free again"
            onclick={() => run('forget', {}, next =>
              next.ledger?.forgotten?.length ? `Forgot ${next.ledger.forgotten.length} deleted VM(s).`
                : 'No stopped VM has been deleted from disk.')}>
            Forget deleted VMs</Button>
        </div>
        <p class="mt-2 text-xs text-muted-foreground">Both keep a backup of the ledger first and touch nothing on disk.</p>
      </div>
    {:else if !error}
      <p class="text-muted-foreground">Loading...</p>
    {/if}
    {#if note}<p role="status" class="text-xs text-success">{note}</p>{/if}
    {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  </div>
</section>
