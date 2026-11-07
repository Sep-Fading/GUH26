// Computes defaults from actual host capacity.
import { recommend } from './plan.mjs';
// Names the real home folder in the macOS guide.
import { homedir } from 'node:os';
// Prints numbered guides and checks typed helper paths.
import { guide, printGuide, macComputerAppProblem, unquote, windowsHelperName, windowsHelperPathProblem } from './guides.mjs';

// Converts prompt cancellation into a stopped workflow rather than a saved value.
async function answer(ui, promise) {
  const value = await promise;
  if (ui.isCancel(value)) throw new Error('Setup cancelled');
  return value;
}

// Prints one numbered guide as static output; only the question after it is redrawn, so its length does not matter.
export function showGuide(ui, id, ctx, columns = process.stdout.columns ?? 80) {
  printGuide(ui, guide(id, ctx), { columns });
}

// Asks for a path until it passes `problem`; `?` prints the guide and asks again. Pasted quotes are removed.
async function askPath(ui, { message, initialValue, problem, show }) {
  while (true) {
    const path = unquote(await answer(ui, ui.text({ message, initialValue, validate: problem })));
    if (path !== '?') return path;
    show();
  }
}

// Offers the detected Windows Computer Use helper, a typed path, the guide, or no helper. Connecting is never the default.
async function chooseWindowsHelper(ui, host, previous, detected, columns) {
  const ctx = { platform: 'win32', arch: host.arch };
  const show = () => showGuide(ui, 'computer-use', ctx, columns);
  const name = windowsHelperName(host.arch);
  const version = /OpenAI\.Codex_([0-9.]+)_/.exec(detected ?? '')?.[1];
  while (true) {
    const choice = await answer(ui, ui.select({ message: 'Connect the installed ChatGPT Windows Computer Use helper?',
      initialValue: !previous ? 'skip' : previous === detected ? 'detected' : 'path', options: [
        ...(detected ? [{ value: 'detected', label: 'Yes, use the helper found on this PC',
          hint: `${name}${version ? ` from Codex app ${version}` : ''}` }] : []),
        { value: 'path', label: detected ? 'Yes, but one somewhere else' : 'Yes, I will enter its path', hint: 'foreground control, approved per action' },
        { value: 'guide', label: 'Show me how to find this' },
        { value: 'skip', label: 'No (can configure later)' }] }));
    if (choice === 'skip') return null;
    if (choice === 'detected') return detected;
    if (choice === 'guide') { show(); continue; }
    if (!detected) show();
    return askPath(ui, { message: `Full path to ${name} (type ? for a step-by-step guide)`, initialValue: previous ?? '',
      problem: input => windowsHelperPathProblem(input, { arch: host.arch }), show });
  }
}

// Offers the detected Codex Computer Use.app, else shows where it usually is and asks for its path.
async function chooseMacComputerApp(ui, previous, detected, columns) {
  const show = () => showGuide(ui, 'computer-use', { platform: 'darwin', home: homedir() }, columns);
  const ask = () => askPath(ui, { message: 'Full path to Codex Computer Use.app (type ? for a step-by-step guide)',
    initialValue: previous ?? detected ?? '', problem: macComputerAppProblem, show });
  if (!detected) { show(); return ask(); }
  while (true) {
    const choice = await answer(ui, ui.select({ message: 'Computer Use helper for the VM',
      initialValue: previous && previous !== detected ? 'path' : 'detected', options: [
        { value: 'detected', label: 'Use the app found on this Mac', hint: '~/.codex/computer-use/Codex Computer Use.app' },
        { value: 'path', label: 'It is somewhere else' },
        { value: 'guide', label: 'Show me how to find this' }] }));
    if (choice === 'detected') return detected;
    if (choice === 'guide') { show(); continue; }
    show();
    return ask();
  }
}

// Reads a bounded whole number with its recommended value visible.
async function number(ui, message, value, minimum, maximum, initial = value) {
  return Number(await answer(ui, ui.text({ message, initialValue: String(initial),
    placeholder: `${value} (recommended)`, validate: input => {
      if (!/^\d+$/.test(input ?? '') || Number(input) < minimum || Number(input) > maximum) {
        return `Enter a whole number from ${minimum} to ${maximum}`;
      }
    } })));
}

// Says how a multi-choice question is answered. In a plain Windows console the prompt library draws ASCII marks,
// where "[+]" is a chosen row and "[•]" only the highlighted one, so the legend names them; elsewhere the squares
// are self-explanatory and only the keys are named.
export function selectionLegend(env = process.env, platform = process.platform) {
  const unicode = platform !== 'win32' || Boolean(env.WT_SESSION) || env.TERM_PROGRAM === 'vscode'
    || env.TERM_PROGRAM === 'Terminus-Sublime' || env.TERM === 'xterm-256color' || env.TERMINAL_EMULATOR === 'JetBrains-JediTerm';
  const keys = 'Press Space to choose or clear the highlighted row, then Enter to continue.';
  return unicode ? keys : `${keys} [+] means chosen, [ ] not chosen; [•] only marks the highlighted row.`;
}

// Prints the legend once before a multi-choice question, as an info line rather than a guide, when the terminal
// can show messages.
function explainSelection(ui) {
  ui.log?.info?.(selectionLegend());
}

// Asks which installed providers to enable; a single provider needs no question.
async function chooseProviders(ui, found, previous) {
  if (!found.length) {
    throw new Error('Install Codex or Claude Code first. MiniClaw needs at least one provider.');
  }
  let enabled = found.map(provider => provider.id);
  if (found.length > 1) {
    explainSelection(ui);
    enabled = await answer(ui, ui.multiselect({ message: 'Providers to enable', required: true,
      initialValues: previous ? found.filter(provider => previous[provider.id]).map(provider => provider.id)
        : enabled,
      options: found.map(provider => ({ value: provider.id, label: provider.name,
        hint: provider.signedIn ? 'signed in' : 'found; sign in from MiniClaw later' })) }));
  }
  return { codex: enabled.includes('codex'), claude: enabled.includes('claude') };
}

// Asks only relevant installation questions, keeping every choice separate from effects.
// The short path is providers, port and an optional VM.
// `detected` holds what setup found by itself (see detect.mjs); a found value is offered before any path is typed.
export async function collectChoices(ui, host, previous = {},
  { hostOnly = false, providers = [], imageCached = false, detected = {}, columns = process.stdout.columns ?? 80 } = {}) {
  const defaults = recommend(host);
  const enabled = await chooseProviders(ui, providers, previous.providers);
  const port = await number(ui, 'UI/API port', defaults.port, 1024, 65535, previous.port ?? defaults.port);
  const base = { port, backend: 'host', providers: enabled, linux: null };
  if (host.platform === 'win32') base.windowsComputerHelper = await chooseWindowsHelper(ui, host, previous.windowsComputerHelper,
    detected.windowsComputerHelper ?? null, columns);
  const vmEnabled = hostOnly || host.platform === 'win32' ? false : await answer(ui, ui.confirm({
    message: 'Add an isolated macOS VM for untrusted projects?',
    initialValue: previous.vmEnabled ?? false,
    active: `Yes (${imageCached ? 'macOS image already downloaded' : '20 GiB download'}, `
      + `${defaults.master.diskGiB} GiB disk${defaults.masterFits ? '' : ', tight on this Mac'})`,
    inactive: 'No (recommended)' }));
  if (!vmEnabled) return { ...base, vmEnabled, master: null, limits: defaults.limits, computerApp: null };
  let limits = { ...defaults.limits, ...previous.limits };
  if (await answer(ui, ui.confirm({ message: 'Change global VM resource limits?', initialValue: false,
    active: 'Customize', inactive: 'Keep current limits (recommended)' }))) {
    limits = {
      cpuPercent: await number(ui, 'VM CPU budget (%)', limits.cpuPercent, 1, 100),
      memoryPercent: await number(ui, 'VM RAM budget (%)', limits.memoryPercent, 1, 100),
      diskPercent: await number(ui, 'VM disk budget (%)', limits.diskPercent, 1, 100),
    };
  }
  const size = previous.master ?? {};
  const master = {
    cpuCount: await number(ui, `VM vCPUs (recommended: ${defaults.master.cpuCount})`,
      defaults.master.cpuCount, 2, host.cpuCount, size.cpuCount),
    memoryGiB: await number(ui, `VM RAM in GiB (recommended: ${defaults.master.memoryGiB})`,
      defaults.master.memoryGiB, 4, 65536, size.memoryGiB),
    diskGiB: await number(ui, `VM disk in GiB (recommended: ${defaults.master.diskGiB})`,
      defaults.master.diskGiB, 64, 65536, size.diskGiB),
  };
  let computerApp = null;
  if (await answer(ui, ui.confirm({ message: 'Include Computer Use in the VM?',
    initialValue: Boolean(previous.computerApp),
    active: 'Yes, copy the signed helper', inactive: 'No (recommended until needed)' }))) {
    computerApp = await chooseMacComputerApp(ui, previous.computerApp, detected.computerApp ?? null, columns);
  }
  return { ...base, vmEnabled, master, limits, computerApp };
}
