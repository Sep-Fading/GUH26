// Renders prompts and tables in the terminal.
import * as ui from '@clack/prompts';
// Reads and repairs the shared reservation ledger.
import { forgetLedger, readLedger, reconcileLedger } from './ledger.mjs';
// Locates the real registry and measures this Mac.
import { measure, registry } from './resource-admission.mjs';
// Measures the volume that holds the registry.
import { dirname } from 'node:path';

const GiB = 1024 ** 3;
const gib = bytes => `${(bytes / GiB).toFixed(1)} GiB`;

// Prints every reserved guest and the budget it counts against.
function show(view) {
  const rows = view.vms.map(vm => `${vm.state.padEnd(9)} ${String(vm.cpuCount).padStart(2)} CPU  `
    + `${gib(vm.memoryBytes).padStart(9)}  ${vm.id}`);
  console.log(rows.length ? rows.join('\n') : 'No guests are recorded.');
  console.log(`\nIn use   ${view.used.cpuCount} CPU, ${gib(view.used.memoryBytes)} RAM, ${gib(view.used.diskBytes)} disk`);
  console.log(`Budget   ${view.capacity.cpuCount} CPU, ${gib(view.capacity.memoryBytes)} RAM, ${gib(view.capacity.diskBytes)} disk`
    + ` (${view.limits.cpuPercent}% / ${view.limits.memoryPercent}% / ${view.limits.diskPercent}% of this Mac)`);
  if (view.exceeded.length) console.log(`Over budget: ${view.exceeded.join(', ')}`);
  if (view.vms.some(vm => vm.state === 'unknown')) {
    console.log('\nA guest marked unknown lost its supervisor and still holds its CPU and RAM.');
    console.log('Run `miniclaw ledger --reconcile` to release it after checking it is idle.');
  }
}

// Lists the ledger, or releases idle guests whose supervisor died.
async function main() {
  const args = process.argv.slice(2);
  if (args.some(arg => !['--reconcile', '--forget', '--yes'].includes(arg))
    || (args.includes('--reconcile') && args.includes('--forget'))) {
    throw new Error('Use miniclaw ledger [--reconcile | --forget] [--yes]');
  }
  const directory = registry();
  const host = () => measure(dirname(directory));
  const view = readLedger(directory, host);
  if (!view) return console.log('No VM has been reserved on this Mac yet.');
  if (!args.includes('--reconcile') && !args.includes('--forget')) return show(view);
  const assumeYes = args.includes('--yes');
  if (args.includes('--forget')) {
    if (!assumeYes && !(process.stdin.isTTY && process.stdout.isTTY)) {
      throw new Error('Forget needs a terminal, or --yes to accept every deleted guest');
    }
    const result = await forgetLedger(directory, {
      async confirm(guest) {
        if (assumeYes) { console.log(`Forgetting ${guest.id}`); return true; }
        ui.note(`${guest.id}\nIts folder or disk no longer exists. ${gib(guest.bytes)} of disk is still reserved for it.`,
          'Deleted guest');
        return await ui.confirm({ message: 'Forget this guest and free its disk budget?',
          initialValue: false }) === true;
      },
    });
    console.log(result.forgotten.length
      ? `Forgot ${result.forgotten.length} guest(s). Backup: ${result.backup}`
      : 'No stopped guest has been deleted from disk. Nothing was changed.');
    return show(readLedger(directory, host));
  }
  if (!assumeYes && !(process.stdin.isTTY && process.stdout.isTTY)) {
    throw new Error('Reconcile needs a terminal, or --yes to accept every idle guest');
  }
  const result = await reconcileLedger(directory, {
    async confirm(guest) {
      const text = `${guest.id}\n${guest.cpuCount} CPU, ${gib(guest.memoryBytes)} RAM\n`
        + 'No process names this guest and none of its files are open.';
      if (assumeYes) { console.log(`Releasing ${guest.id}`); return true; }
      ui.note(text, 'Idle guest with no supervisor');
      const answer = await ui.confirm({ message: 'Release its CPU and RAM? Its disk stays reserved.',
        initialValue: false });
      return answer === true;
    },
  });
  for (const guest of result.refused) console.log(`Left alone: ${guest.id} (${guest.reason})`);
  console.log(result.released.length
    ? `Released ${result.released.length} guest(s). Backup: ${result.backup}`
    : 'Nothing was changed.');
  show(readLedger(directory, host));
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
