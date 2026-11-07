// Shares binary units with the planner.
import { GiB } from './plan.mjs';

// Tells whether setup can install Claude Code itself.
import { claudeInstallable } from './claude-install.mjs';
const names = { codex: 'Codex (ChatGPT)', claude: 'Claude Code' };
// Names the machine as macOS users knew it; only Windows gets the neutral word.
export function machineName(platform = process.platform) {
  return platform === 'win32' ? 'this computer' : 'this Mac';
}

const stages = {
  download: 'Downloading verified images and packages',
  'vc-runtime': 'Installing the Microsoft Visual C++ runtime (administrator approval required)',
  'windows-git': 'Installing the private Git runtime',
  'windows-network': 'Installing Windows network isolation (administrator approval required)',
  'build-worker': 'Building the VM worker',
  'master-media': 'Preparing the VM setup disk',
  'install-macos': 'Installing macOS in the VM',
  'build-network': 'Building the filtered VM network helper',
};

// Formats binary byte units without presenting disk reservations as downloads.
export function bytes(value) {
  if (value === 0) return '0 B';
  return value >= GiB ? `${(value / GiB).toFixed(2)} GiB` : `${(value / 1024 ** 2).toFixed(2)} MiB`;
}

// Aligns label and value pairs into one readable column.
function table(rows) {
  const width = Math.max(...rows.map(([label]) => label.length));
  return rows.map(([label, value]) => `${label.padEnd(width)}   ${value}`).join('\n');
}

// Lists what this Mac has and what is missing, before any question is asked.
export function checks({ host, providers, tools, detected }) {
  const rows = [
    ['Hardware', `${host.cpuCount} CPUs, ${bytes(host.memoryBytes)} RAM, ${bytes(host.freeBytes)} free`],
    ['Node', tools.node],
    ['Rust (cargo)', tools.prebuilt ? 'not needed, this release is prebuilt' : tools.cargo ? 'found' : 'missing, install Rust and rerun setup'],
    host.platform === 'win32'
      ? ['Windows sandbox', 'AppContainer + WFP service, set up by this installation']
      : ['Seatbelt sandbox', tools.seatbelt ? 'available' : 'missing, agent shell commands stay off'],
  ];
  // Windows binaries need Microsoft's runtime; setup installs it from Microsoft when it is missing or old.
  if (host.platform === 'win32' && tools.vcRuntime) rows.push(['Microsoft Visual C++ runtime', tools.vcRuntime.sufficient
    ? `installed (${tools.vcRuntime.version})` : `${tools.vcRuntime.version ? `${tools.vcRuntime.version} is too old, ` : ''}setup installs it automatically from Microsoft`]);
  const installsClaude = !providers.some(provider => provider.id === 'claude') && claudeInstallable(host.platform, host.arch);
  if (!providers.length) {
    rows.push(['Providers', installsClaude ? 'none found yet, setup installs Claude Code' : 'none found, install Codex or Claude Code']);
  }
  if (installsClaude) rows.push(['Claude Code', 'not found, setup installs it automatically from Anthropic']);
  for (const provider of providers) {
    rows.push([provider.name, provider.signedIn ? 'signed in' : 'found, not signed in']);
  }
  if (detected) rows.push(...foundRows(detected, host.platform));
  return table(rows);
}

// Rows for what setup found by itself; a missing value names the guide that finds it. Paths are shortened to fit.
function foundRows(detected, platform) {
  const rows = [];
  if (platform === 'win32') {
    const version = /OpenAI\.Codex_([0-9.]+)_/.exec(detected.windowsComputerHelper ?? '')?.[1];
    rows.push(['Computer Use helper', detected.windowsComputerHelper ? `found in Codex app ${version ?? ''}`.trim()
      : 'not found; see: miniclaw guide computer-use']);
  } else if (detected.computerApp) rows.push(['Computer Use app', 'found in ~/.codex/computer-use']);
  return rows;
}

// Returns the exact plan shown before the user approves any change.
export function review(plan, { destination, cache, cached = new Set(), ledger = null }) {
  const enabled = Object.entries(plan.providers).filter(([, on]) => on).map(([id]) => names[id]);
  const rows = [
    ['Install to', destination],
    ['Interface', `${plan.url} (${machineName(plan.platform)} only)`],
    ['Providers', enabled.join(', ')],
    ['Sandbox', plan.platform === 'win32'
      ? 'AppContainer + WFP service: agent commands see only granted folders; network only through approved hosts'
      : 'Seatbelt: agent shell commands see only granted files, no network'],
    ['VM', plan.master ? `${plan.master.cpuCount} vCPUs, ${plan.master.memoryGiB} GiB RAM, `
      + `${plan.master.diskGiB} GiB disk` : 'none'],
  ];
  // Windows Git always travels inside the release; setup never downloads it.
  if (plan.vcRuntime) rows.push(['Microsoft runtime', `Visual C++ Redistributable (${plan.vcRuntime.arch}) from Microsoft, installed with administrator approval`]);
  if (plan.platform === 'win32') rows.push(['Workspace Git', `Git 2.56.0, bundled with this release${plan.windowsGit
    ? ` (${bytes(plan.windowsGit.bytes)})` : ''}, private read-only runtime`]);
  if (plan.platform === 'win32') rows.push(['Windows service',
    'Administrator approval required for network isolation; normal MiniClaw runs under your account']);
  if (plan.windowsComputerHelper) rows.push(['Windows Computer Use', `${plan.windowsComputerHelper}; foreground control outside the shell sandbox, explicit approval for each action`]);
  if (!plan.master) return table(rows);
  rows.push(
    ['VM budget', `CPU ${plan.limits.cpuPercent}%, RAM ${plan.limits.memoryPercent}%, `
      + `disk ${plan.limits.diskPercent}% of this Mac, shared by all installations`],
    ['Downloads', downloads(plan, cached, cache)],
    ['Peak disk use', bytes(plan.peakBytes)],
  );
  if (ledger && (ledger.used.cpuCount || ledger.used.diskBytes)) rows.push(['Already reserved',
    `${ledger.used.cpuCount} CPU, ${gib(ledger.used.memoryBytes)} RAM, ${gib(ledger.used.diskBytes)} disk by other VMs`]);
  if (plan.computer) rows.push(['Computer Use', `copies ${plan.computer.path} (${bytes(plan.computer.bytes)})`]);
  const sources = [...plan.images, ...plan.packages]
    .map(item => `  ${item.name ?? item.id}: ${bytes(item.bytes)}, ${item.algorithm} verified`
      + (cached.has(item.id) ? ', already downloaded' : `\n    ${item.url}`));
  return [table(rows), '', 'Verified downloads', ...sources, '',
    'The VM boots only when a project asks for it. Chat never waits on it.',
    'Setup starts it once so you can finish macOS setup and sign in to Codex inside it.',
    'No public listeners and no sudo.'].join('\n');
}

// Formats a size in GiB with one decimal, for budgets.
const gib = value => `${(value / GiB).toFixed(1)} GiB`;

// Returns the bytes still to download once cached items are counted.
export function remainingBytes(plan, cached = new Set()) {
  return [...plan.images, ...plan.packages].filter(item => !cached.has(item.id))
    .reduce((sum, item) => sum + item.bytes, 0);
}

// Says how much is left to fetch once cached items are counted.
function downloads(plan, cached, cache) {
  const items = [...plan.images, ...plan.packages];
  const left = remainingBytes(plan, cached);
  if (!cached.size) return `${bytes(left)} into ${cache}`;
  return `${left ? `${bytes(left)} left to fetch` : 'nothing to fetch'}, `
    + `${cached.size} of ${items.length} already in ${cache}`;
}

// Names each global VM budget this plan would exceed, counting what other VMs hold.
// The installer reserves nothing itself, so this is the only warning before a long build.
export function budgetProblems(plan, ledger) {
  if (!plan.master || !ledger) return [];
  const problems = [];
  const memory = plan.master.memoryGiB * GiB;
  if (ledger.used.cpuCount + plan.master.cpuCount > ledger.capacity.cpuCount) {
    problems.push(`CPU: ${ledger.used.cpuCount} reserved + ${plan.master.cpuCount} new is over the ${ledger.capacity.cpuCount} CPU budget`);
  }
  if (ledger.used.memoryBytes + memory > ledger.capacity.memoryBytes) {
    problems.push(`RAM: ${gib(ledger.used.memoryBytes)} reserved + ${gib(memory)} new is over the ${gib(ledger.capacity.memoryBytes)} budget`);
  }
  const disk = plan.vmDiskBytes + plan.retainedImageBytes + plan.retainedPackageBytes;
  if (ledger.used.diskBytes + disk > ledger.capacity.diskBytes) {
    problems.push(`Disk: ${gib(ledger.used.diskBytes)} reserved + ${gib(disk)} new is over the ${gib(ledger.capacity.diskBytes)} budget`);
  }
  return problems;
}

// Returns the plain name of one installation stage.
export function stageLabel(stage, platform = process.platform) {
  if (stage === 'build') return `Building MiniClaw for ${machineName(platform)}`;
  return stages[stage] ?? stage;
}

// Lists the stages this plan will run, in order.
export function stageList(plan) {
  return plan.master ? ['download', 'build', 'build-worker', 'master-media', 'build-network', 'install-macos']
    : [...(plan.packages?.length ? ['download'] : []), ...(plan.vcRuntime ? ['vc-runtime'] : []), 'build',
      ...(plan.platform === 'win32' && (plan.windowsGit || plan.packages?.some(item => item.id === 'windows-git')) ? ['windows-git'] : []),
      ...(plan.platform === 'win32' ? ['windows-network'] : [])];
}

// The lines of `miniclaw setup --help`. A Windows release installs itself, not a source checkout;
// the Mac and a checkout keep their words.
export function setupHelp({ platform = process.platform, release = false } = {}) {
  return ['miniclaw setup [--preview] [--mac-only] [--resume] [--destination PATH]',
    platform === 'win32' ? `Installs MiniClaw on this Windows computer from ${release ? 'this release' : 'the source checkout'}.`
      : 'Installs MiniClaw on this Apple silicon Mac from the source checkout.',
    '--preview shows the default plan and changes nothing.',
    '--mac-only skips the optional VM. --resume finishes an interrupted VM setup.',
    'Not sure where to find a path or how to create a key setup asks for? Run: miniclaw guide'];
}

// The hint beside "Install MiniClaw" when no VM is chosen.
export function installHint({ platform = process.platform, release = false } = {}) {
  return platform === 'win32' && release ? 'installs this release' : 'builds from this checkout';
}

// Says that commands without a path now use the new installation, when another one was remembered before;
// the earlier one stays installed and is named, so it can still be updated or removed. Null when nothing changed.
export function rememberedNotice(previous, destination, platform = process.platform) {
  if (!previous) return null;
  const same = platform === 'win32' ? previous.toLowerCase() === destination.toLowerCase() : previous === destination;
  if (same) return null;
  return `miniclaw commands without a path (start, update, repair, uninstall) now use ${destination}.\n`
    + `The installation at ${previous} is still there; name it to use it, e.g. miniclaw uninstall "${previous}".`;
}

// Says truthfully what a failed installation left behind: Windows keeps the folder when its
// network service may already exist, so `miniclaw resume` can finish it.
export function failureNotice(error) {
  const cache = 'Verified downloads stay in the cache, so running setup again fetches nothing twice.';
  if (error?.retained) {
    return `The unfinished installation was kept at ${error.retained}.\n`
      + `Finish it with:  miniclaw resume "${error.retained}"\n${cache}`;
  }
  if (error?.keptData) return `Nothing was installed; your data in ${error.keptData} was kept as it was.\n${cache}`;
  return `Nothing was installed; the unfinished folder was removed.\n${cache}`;
}

// Tells the user how to start MiniClaw and what is left to do.
// A Windows release names its own launcher (`launcher`), which stays in the folder setup ran from.
export function nextSteps({ destination, port, providers, enabled, vm, launcher }) {
  const folder = launcher?.replace(/[\\/][^\\/]+$/, '');
  const lines = [launcher ? `Start      .\\miniclaw.cmd  (in ${folder})` : 'Start      miniclaw',
    `Open       http://127.0.0.1:${port}`];
  for (const provider of providers) {
    if (enabled[provider.id] && !provider.signedIn) {
      lines.push(`Sign in    ${provider.name}: use the sign in button in MiniClaw`);
    }
  }
  lines.push('Providers  turn each on or off from the plug icon, no restart needed');
  if (vm) lines.push('VM         mark a project "run in VM"; it boots on first use');
  return lines.join('\n');
}
