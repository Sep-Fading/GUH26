// Starts only the selected native VM supervisors.
import { spawn } from 'node:child_process';
// Claims private session ownership and fixed routes.
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
// Creates a fresh control key for this session.
import { randomBytes } from 'node:crypto';
// Resolves supervisor-owned files and socket destinations.
import { dirname, join } from 'node:path';
// Pairs the authenticated controller and worker streams.
import { privateDirectory, startBroker, writeRoutes } from './broker.mjs';
// Authenticates status and approved start calls.
import { startControl } from './control.mjs';
// Restricts guest TLS to checked public destinations.
import { startEgress } from './egress.mjs';
// Owns the optional native guest network helper.
import { startNetwork } from './network-process.mjs';
// Reserves host capacity before starting supervisors.
import { configureResourceLimits, measure, registry as ledgerRegistry, reservePair } from './resource-admission.mjs';
// Reads and repairs the shared VM budget for the settings page.
import { forgetLedger, readLedger, reconcileLedger } from './ledger.mjs';
// Retains dedicated identities across host restarts.
import { openDedicatedRegistry } from './dedicated-registry.mjs';
// Builds a fresh guest from cached vendor media.
import { prepareDedicatedGuest } from './dedicated-setup.mjs';

const nativeStates = new Set(['starting', 'vm_running', 'shutdown_requested',
  'shutdown_waiting', 'shutdown_failed', 'stopped', 'vm_failed', 'start_failed',
  'force_stopping', 'forced_stopped', 'force_stop_failed', 'network_failed']);

// Reports shutdown failures without exposing guest console contents.
export function reportShutdown(role, status) {
  const name = role.startsWith('dedicated:') ? 'Project VM' : 'Master VM';
  if (status === 'shutdown_waiting') {
    console.error(`${name} is still shutting down; its disk remains locked. No force-stop was performed.`);
  } else if (status === 'shutdown_failed') {
    console.error(`${name} shutdown request failed; its disk remains locked.`);
  } else if (status === 'force_stopping') {
    console.error(`${name} has not stopped after 90 seconds; powering off. Unsaved data may be lost.`);
  } else if (status === 'forced_stopped') {
    console.error(`${name} was forced off; unsaved data or unflushed writes may have been lost.`);
  } else if (status === 'force_stop_failed') {
    console.error(`${name} power-off failed; its disk remains locked. Retry shutdown.`);
  } else if (status === 'network_failed') {
    console.error(`${name} network disconnected; shutting down the pair. No fallback network was enabled.`);
  }
}

// Reads bounded native status lines; guest console output uses a separate stream.
function statusReader(report) {
  let line = '', oversized = false;
  return chunk => {
    const parts = chunk.toString().split('\n');
    for (const [index, part] of parts.entries()) {
      if (line.length + part.length > 1024) { line = ''; oversized = true; }
      if (!oversized) line += part;
      if (index === parts.length - 1) continue;
      if (!oversized) {
        let value;
        try { value = JSON.parse(line); }
        catch { /* Non-status output cannot become a lifecycle event. */ }
        if (value && nativeStates.has(value.status)) report(value.status);
      }
      line = '';
      oversized = false;
    }
  };
}

// Summarises the shared ledger for the settings page: guests, budget and what is used.
// File rows stay out of it; only their total matters there.
function ledgerView() {
  const directory = ledgerRegistry();
  const view = readLedger(directory, () => measure(dirname(directory)));
  if (!view) return { vms: [], limits: null, capacity: null, used: null, exceeded: [], freeBytes: null };
  return { vms: view.vms.map(({ id, state, cpuCount, memoryBytes }) => ({ id, state, cpuCount, memoryBytes })),
    limits: view.limits, capacity: view.capacity, used: view.used, exceeded: view.exceeded,
    freeBytes: view.freeBytes };
}

// Runs the VM session for a backend on the Mac, with private channels; shutdown keeps
// every disk. No VM boots until a task asks, and a private link file tells the backend
// where workers attach.
export async function startPair(pair, {
  signal, onOutput = () => {}, onStatus = reportShutdown, admit = reservePair, limits,
  prepareDedicated = prepareDedicatedGuest, setup = false,
} = {}) {
  signal?.throwIfAborted();
  const egressMode = pair.egress ?? 'public-web';
  if (!pair.mac?.worker) throw new Error('A VM session needs a Mac-backend pair');
  if (!['openai', 'public-web'].includes(egressMode)) throw new Error('Unknown egress mode');
  await privateDirectory(pair.root);
  const lock = join(pair.root, 'ActiveSession');
  await mkdir(lock, { mode: 0o700 });
  let dir, broker, hostDir, control, controlKeyPath;
  let egress, network, closing;
  let registry;
  const dedicatedBrokers = [];
  const dedicatedStarts = new Map();
  const dedicatedLeases = new Map();
  let masterReservation, masterSettled = false, masterStart, masterArgs;
  let masterFailure;
  const children = [];
  // Waits for native shutdown confirmation before releasing session ownership.
  function close() {
    closing ??= (async () => {
      await registry?.close();
      for (const { child } of children) if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      const codes = await Promise.all(children.map(child => child.exited));
      await Promise.all([...dedicatedLeases.values()].map(async entry => {
        await entry.network?.close();
        if (!entry.settled) {
          entry.lease?.settle(['stopped', 'forced_stopped'].includes(entry.owner.status)
            ? ['master'] : []);
          entry.settled = true;
        }
      }));
      await network?.close();
      await egress?.close();
      await broker?.close();
      await Promise.all(dedicatedBrokers.map(entry => entry.broker.close()));
      await control?.close();
      const master = children.findLast(entry => entry.role === 'master');
      const stopped = !master?.child.pid || ['stopped', 'forced_stopped'].includes(master.status)
        ? ['master'] : [];
      if (masterReservation && !masterSettled) {
        masterReservation.settle(stopped);
        masterSettled = true;
      }
      if (dir) await rm(dir, { recursive: true, force: true });
      if (hostDir) await rm(hostDir, { recursive: true, force: true });
      await rm(lock, { recursive: true, force: true });
      return codes.find(code => code !== 0) ?? 0;
    })();
    return closing;
  }
  // Keeps only bounded native status output; guest RPC never passes through logging.
  async function start(role, binary, args) {
    const child = spawn(binary, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    const exited = new Promise(resolve => {
      child.once('error', () => resolve(1));
      child.once('close', code => resolve(code ?? 1));
    });
    const owner = { role, child, exited, status: 'starting' };
    children.push(owner);
    child.stdout.on('data', statusReader(status => {
      owner.status = status;
      onStatus(role, status);
    }));
    for (const stream of [child.stdout, child.stderr]) stream.on('data', value => onOutput(value.toString()));
    await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
    return owner;
  }
  // Settles only when setup's VM exits by itself; a normal session never settles it.
  let reportStopped;
  const done = new Promise(resolve => { reportStopped = resolve; });
  // Reserves Master compute only after an action explicitly requests this VM.
  async function startMaster() {
    if (closing) throw new Error('VM session is shutting down');
    const previous = children.findLast(entry => entry.role === 'master');
    if (previous && previous.child.exitCode === null && previous.child.signalCode === null) return;
    if (previous && !['stopped', 'forced_stopped'].includes(previous.status)) {
      throw new Error('Master VM did not stop cleanly; inspect its retained disk before retrying');
    }
    if (masterStart) return masterStart;
    masterStart = (async () => {
      signal?.throwIfAborted();
      const lease = await admit(pair, { initialLimits: limits, roles: ['master'] });
      masterReservation = lease;
      masterSettled = false;
      let owner;
      try {
        if (closing) throw new Error('VM session is shutting down');
        if (pair.network) network = await startNetwork(pair.network, dir, { signal, onOutput });
        if (closing) throw new Error('VM session is shutting down');
        owner = await start('master', pair.master.binary, masterArgs);
        void owner.exited.then(async code => {
          // Setup waits on this VM, so its exit is reported; in normal use it comes and goes.
          if (setup && !closing) reportStopped(code);
          await network?.close();
          network = null;
          if (!masterSettled) {
            lease.settle(['stopped', 'forced_stopped'].includes(owner.status) ? ['master'] : []);
            masterSettled = true;
          }
        });
      } catch (error) {
        if (!owner && !masterSettled) {
          lease.settle(['master']);
          masterSettled = true;
        }
        await network?.close();
        network = null;
        throw error;
      }
    })().finally(() => { masterStart = null; });
    return masterStart;
  }
  // Reports native state without treating a missing Codex connection as VM-off proof.
  function masterStatus() {
    const owner = children.findLast(entry => entry.role === 'master');
    if (!owner && masterFailure) return { state: 'failed', reason: masterFailure };
    if (!owner) return { state: 'off' };
    if (owner.child.exitCode !== null || owner.child.signalCode !== null) {
      if (['stopped', 'forced_stopped'].includes(owner.status)) return { state: 'off' };
      return { state: 'failed', reason: owner.status === 'network_failed'
        ? 'Master VM network disconnected.' : 'Master VM stopped before it was ready.' };
    }
    if (owner.status === 'vm_running') return { state: 'running' };
    if (['shutdown_requested', 'shutdown_failed', 'force_stopping',
      'force_stop_failed'].includes(owner.status)) return { state: 'stopping' };
    return { state: 'starting' };
  }
  // Reports a retained project guest without borrowing Master's state.
  function dedicatedStatus(vmId) {
    const base = registry?.status(vmId)
      ?? { state: 'missing', reason: 'Assigned VM does not exist.' };
    if (base.state === 'missing' || base.state === 'failed') return base;
    const owner = children.findLast(entry => entry.role === `dedicated:${vmId}`);
    if (!owner) return { ...base, state: base.stage === 'awaiting_setup' ? 'off' : 'starting' };
    if (owner.child.exitCode !== null || owner.child.signalCode !== null) {
      return ['stopped', 'forced_stopped'].includes(owner.status)
        ? { ...base, state: 'off' }
        : { ...base, state: 'failed', reason: owner.status === 'network_failed'
          ? 'Project VM network disconnected.' : 'Project VM stopped before it was ready.' };
    }
    return { ...base, state: owner.status === 'vm_running' ? 'running'
      : ['shutdown_requested', 'shutdown_failed', 'force_stopping',
        'force_stop_failed'].includes(owner.status) ? 'stopping' : 'starting' };
  }
  // Reserves and starts exactly the named project's guest and fixed worker route.
  async function startDedicated(vmId, prepared) {
    if (closing) throw new Error('VM session is shutting down');
    const machine = registry?.details(vmId);
    if (!machine || (machine.stage !== 'awaiting_setup' && !prepared)) {
      throw new Error('Assigned VM does not exist.');
    }
    const old = children.findLast(entry => entry.role === `dedicated:${vmId}`);
    if (old && old.child.exitCode === null && old.child.signalCode === null) return dedicatedStatus(vmId);
    if (old && !['stopped', 'forced_stopped'].includes(old.status)) {
      throw new Error('Project VM resource ownership is unresolved');
    }
    if (dedicatedStarts.has(vmId)) return dedicatedStarts.get(vmId);
    const request = (async () => {
      const broker = dedicatedBrokers[machine.slot];
      if (!broker) throw new Error('Project VM worker route is unavailable');
      const attemptRoot = join(pair.root, 'dedicated-vms', vmId, `Attempt-${machine.attempt}`);
      const guest = prepared?.guest ?? join(attemptRoot, 'Guest');
      const media = prepared?.media ?? join(attemptRoot, 'setup', 'MasterSetup.dmg');
      const lease = prepared?.lease ?? await admit({ ...pair,
        master: { ...pair.master, guest, media } },
      { initialLimits: limits, roles: ['master'] });
      let attachedNetwork, owner;
      try {
        const routes = join(lock, `DedicatedRoutes-${machine.slot}.json`);
        await writeRoutes(routes, [
          { port: 10242, path: join(broker.dir, 'worker.sock') },
          { port: 10243, path: join(dir, 'egress.sock') },
        ]);
        if (pair.network) attachedNetwork = await startNetwork(pair.network,
          broker.dir, { signal, onOutput });
        const args = ['run', guest, '--media', media, '--routes', routes];
        if (attachedNetwork) args.push('--network', broker.dir);
        owner = await start(`dedicated:${vmId}`, pair.master.binary, args);
        const entry = { owner, lease, network: attachedNetwork, settled: false };
        dedicatedLeases.set(vmId, entry);
        void owner.exited.then(async () => {
          await attachedNetwork?.close();
          if (!entry.settled) {
            lease?.settle(['stopped', 'forced_stopped'].includes(owner.status)
              ? ['master'] : []);
            entry.settled = true;
          }
        });
        return dedicatedStatus(vmId);
      } catch (error) {
        await attachedNetwork?.close();
        if (!owner) lease?.settle(['master']);
        throw error;
      }
    })().finally(() => dedicatedStarts.delete(vmId));
    dedicatedStarts.set(vmId, request);
    return request;
  }
  // Stops one unfinished project setup without affecting Master or Linux.
  async function stopDedicated(vmId) {
    const owner = children.findLast(entry => entry.role === `dedicated:${vmId}`);
    if (owner && owner.child.exitCode === null && owner.child.signalCode === null) {
      owner.child.kill('SIGTERM');
      await owner.exited;
    }
  }
  try {
    // Nothing is reserved at start; each VM reserves its own share when it boots.
    signal?.throwIfAborted();
    dir = await mkdtemp('/private/tmp/mc-link-');
    broker = await startBroker(dir);
    {
      hostDir = await mkdtemp('/private/tmp/mc-host-link-');
      const key = randomBytes(32);
      controlKeyPath = join(hostDir, 'control.key');
      await writeFile(controlKeyPath, key.toString('hex'), { flag: 'wx', mode: 0o600 });
      registry = await openDedicatedRegistry(pair.root, {
        setup: async (machine, report, setupSignal) => {
          const prepared = await prepareDedicated(pair, machine, {
            signal: setupSignal, report,
            reserve: async ({ guest, media }) => admit({ ...pair,
              master: { ...pair.master, guest, media } },
            { initialLimits: limits, roles: ['master'] }),
          });
          await startDedicated(machine.vmId, prepared);
          await report('awaiting_setup', 100);
        },
        stop: ({ vmId }) => stopDedicated(vmId),
      });
      control = await startControl(hostDir, key, {
        status: vmId => vmId === 'master' ? masterStatus()
          : dedicatedStatus(vmId),
        list: () => registry.list(),
        provision: requestId => registry.provision(requestId),
        retry: vmId => registry.details(vmId)?.stage === 'awaiting_setup'
          ? startDedicated(vmId) : registry.retry(vmId),
        cancel: vmId => registry.cancel(vmId),
        // Deletes a stopped project VM's disk for good and frees its share of the budget. The
        // control service has already refused a running, starting or stopping VM.
        delete: async vmId => {
          await registry.remove(vmId);
          dedicatedLeases.delete(vmId);
          await forgetLedger(ledgerRegistry(), { confirm: async () => true }).catch(() => {});
          return dedicatedStatus(vmId);
        },
        start: async vmId => {
          if (vmId !== 'master') return startDedicated(vmId);
          masterFailure = null;
          try { await startMaster(); return masterStatus(); }
          catch (error) {
            masterFailure = error?.code === 'resource_limit' ? 'VM resource limit exceeded.'
              : error?.message === 'VM resource ownership is unresolved'
                ? 'Master VM resource ownership is unresolved.'
                : error?.code === 'ENOENT' ? 'Master VM setup is incomplete.'
                  : 'Master VM could not start. Check VM diagnostics.';
            throw error;
          }
        },
        stop: async vmId => {
          const role = vmId === 'master' ? 'master' : `dedicated:${vmId}`;
          const owner = children.findLast(entry => entry.role === role);
          if (!owner?.child.kill('SIGTERM')) throw new Error('VM shutdown request failed');
          return { state: 'stopping' };
        },
        // The settings page reads and repairs the shared budget through these. The page asks
        // the user first, so every idle or deleted guest it finds is accepted here.
        ledger: () => ledgerView(),
        limits: values => { configureResourceLimits(values); return ledgerView(); },
        reconcile: async () => {
          const { released, refused } = await reconcileLedger(ledgerRegistry(), { confirm: async () => true });
          return { ...ledgerView(), released, refused };
        },
        forget: async () => {
          const { forgotten } = await forgetLedger(ledgerRegistry(), { confirm: async () => true });
          return { ...ledgerView(), forgotten };
        },
        show: async vmId => {
          const role = vmId === 'master' ? 'master' : `dedicated:${vmId}`;
          const owner = children.findLast(entry => entry.role === role);
          if (!owner?.child.kill('SIGUSR1')) throw new Error('VM window request failed');
          return { state: 'running' };
        } });
      for (let slot = 0; slot < 64; slot++) {
        const routeDir = join(dir, `d${String(slot).padStart(2, '0')}`);
        await mkdir(routeDir, { mode: 0o700 });
        dedicatedBrokers.push({ slot, dir: routeDir,
          broker: await startBroker(routeDir) });
      }
    }
    egress = await startEgress(dir, { mode: egressMode });
    const masterRoutes = join(lock, 'MasterRoutes.json');
    await writeRoutes(masterRoutes, [
      { port: 10242, path: join(dir, 'worker.sock') },
      { port: 10243, path: join(dir, 'egress.sock') },
    ]);
    // The backend on the Mac reads this to find the control socket and worker routes.
    await writeFile(join(hostDir, 'links.json'), JSON.stringify({
      controlSocket: join(hostDir, 'control.sock'), controlKey: controlKeyPath,
      worker: pair.mac.worker, master: join(dir, 'controller.sock'), dedicated: dir,
    }), { flag: 'wx', mode: 0o600 });
    signal?.throwIfAborted();
    masterArgs = ['run', pair.master.guest, '--media', pair.master.media,
      '--routes', masterRoutes];
    if (pair.network) masterArgs.push('--network', dir);
    // A working VM stays hidden behind an input shield. First-time setup needs a window
    // the user can click in, for macOS setup and the guest installer.
    if (!setup) masterArgs.push('--background');
    signal?.throwIfAborted();
    // VMs come and go while the session lives, so it ends only when it is closed.
    return { close, done, egressMode, startMaster,
      linksPath: join(hostDir, 'links.json'),
      controlPath: join(hostDir, 'control.sock'), controlKeyPath,
      // Fails setup if a project VM died; Master failures show in its status instead.
      check() {
        const stopped = children.find(({ child, role, status }) => role !== 'master'
          && (child.exitCode !== null || child.signalCode !== null)
          && !['stopped', 'forced_stopped'].includes(status));
        if (stopped) throw new Error('A VM stopped before setup completed');
      },
    };
  } catch (error) {
    await close();
    throw error;
  }
}
