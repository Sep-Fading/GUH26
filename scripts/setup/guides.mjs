// Step-by-step guides for every value MiniClaw setup asks the user to find or create.
// Plain data and string helpers only (no Node imports): setup, `miniclaw guide` and the web page share this text.

// The order `miniclaw guide` lists them in.
export const topics = ['computer-use', 'provider-sign-in'];

const helperFolder = 'app\\resources\\cua_node\\bin\\node_modules\\@oai\\cua\\bin\\windows';

// The helper file name the Codex app ships for this PC's processor.
export function windowsHelperName(arch) {
  return arch === 'arm64' ? 'codex-computer-use-arm64.exe' : 'codex-computer-use.exe';
}

// Removes surrounding spaces and the quotes Explorer's "Copy as path" and Finder add.
export function unquote(value) {
  const text = String(value ?? '').trim();
  return /^(["']).*\1$/.test(text) ? text.slice(1, -1).trim() : text;
}

// True for a Windows path that starts at a drive or a share.
const windowsAbsolute = path => /^[A-Za-z]:[\\/]/.test(path) || /^\\\\[^\\]+\\/.test(path);
const baseName = path => path.split(/[\\/]/).pop();

// Says why a typed Windows helper path cannot be right, and what a right one looks like; undefined when it may be.
// `?` asks for the guide, so it is never an error.
export function windowsHelperPathProblem(input, { arch } = {}) {
  const path = unquote(input);
  if (path === '?') return undefined;
  const name = arch ? windowsHelperName(arch) : 'codex-computer-use-arm64.exe or codex-computer-use.exe';
  const example = `Example: C:\\Program Files\\WindowsApps\\OpenAI.Codex_<version>_${arch === 'x64' ? 'x64' : 'arm64'}__...\\${arch ? name : windowsHelperName('arm64')}`;
  if (!windowsAbsolute(path)) return `Enter the full path, starting with a drive such as C:\\, to ${name}.\n${example}\nType ? for a step-by-step guide.`;
  if (!['codex-computer-use.exe', 'codex-computer-use-arm64.exe'].includes(baseName(path).toLowerCase())) {
    return `That is not the helper. The file is named ${name}, inside the Codex app's package folder.\n${example}\nType ? for a step-by-step guide.`;
  }
  return undefined;
}

// Says why a typed macOS Computer Use app path cannot be right; undefined when it may be.
export function macComputerAppProblem(input) {
  const path = unquote(input);
  if (path === '?') return undefined;
  const example = 'Example: /Users/<you>/.codex/computer-use/Codex Computer Use.app';
  if (!path.startsWith('/')) return `Enter the full path, starting with /, to Codex Computer Use.app.\n${example}\nType ? for a step-by-step guide.`;
  if (!/\.app\/?$/.test(path)) return `Choose the application itself, a folder ending in .app, not a file inside it.\n${example}\nType ? for a step-by-step guide.`;
  return undefined;
}

// Platform words every guide uses.
function words(ctx) {
  const windows = ctx.platform === 'win32';
  return { windows, machine: windows ? 'this PC' : 'this Mac' };
}

// Each guide: when setup asks, why it is needed, a short hint, numbered steps and official sources.
const builders = {
  'computer-use': (ctx, w) => w.windows ? {
    title: 'Find the Codex Computer Use helper (Windows)',
    asked: 'Windows setup: "Connect the installed ChatGPT Windows Computer Use helper?"',
    why: 'MiniClaw starts this signed OpenAI program to see and control apps on this PC. Setup saves its path; it never copies it.',
    hint: [`The file ${ctx.arch ? windowsHelperName(ctx.arch) : 'codex-computer-use[-arm64].exe'} inside the Codex app's package folder,`,
      'under C:\\Program Files\\WindowsApps. Setup looks there first.'],
    steps: [
      'Install the Codex (ChatGPT) desktop app from OpenAI and sign in. In the app open Plugins > Computer Use, choose Install plugin if asked, and turn on the Computer Use server and skill.',
      `The helper is ${ctx.arch ? windowsHelperName(ctx.arch) : 'codex-computer-use-arm64.exe on ARM64 PCs or codex-computer-use.exe on x64 PCs (Settings > System > About > System type says which)'}, typically in C:\\Program Files\\WindowsApps\\OpenAI.Codex_<version>_${ctx.arch === 'x64' ? 'x64' : 'arm64'}__2p2nqsd0c76g0\\${helperFolder}. <version> changes with every app update.`,
      'Windows hides the WindowsApps folder, so ask Windows instead. In PowerShell (as yourself, not as administrator) run this; it prints the full path:'
        + (ctx.arch ? `\nJoin-Path (Get-AppxPackage -Name OpenAI.Codex).InstallLocation '${helperFolder}\\${windowsHelperName(ctx.arch)}'`
          : `\n(Get-ChildItem (Join-Path (Get-AppxPackage -Name OpenAI.Codex).InstallLocation '${helperFolder}') -Filter 'codex-computer-use*.exe').FullName`),
      'An error or no output means the Codex app is not installed for your account. Test-Path followed by the path in quotes prints True when the file is there.',
      'Paste the path at the setup prompt; quotes from "Copy as path" are fine. Setup checks its OpenAI signature and saves your choice. After a Codex app update MiniClaw finds the helper in the new version when it starts and before each use, and checks the signature again; if it cannot, the Tools menu says why.',
    ],
    sources: ['https://learn.chatgpt.com/docs/computer-use', 'https://learn.microsoft.com/en-us/powershell/module/appx/get-appxpackage'],
  } : {
    title: 'Find Codex Computer Use.app (macOS)',
    asked: 'macOS setup, when the VM is added: "Include Computer Use in the VM?"',
    why: 'Setup copies this OpenAI-signed app into the VM so agents there can use Computer Use. Your sign-in is not copied.',
    hint: ['The Codex app installs it at ~/.codex/computer-use/Codex Computer Use.app.', 'Setup looks there first.'],
    steps: [
      'Install the Codex (ChatGPT) desktop app on this Mac and sign in. In the app open Plugins > Computer Use, choose Install plugin if asked, turn on the Computer Use server and skill, then choose Try now.',
      'When macOS asks, allow Screen Recording and Accessibility for Codex Computer Use in System Settings > Privacy & Security.',
      `The app is then at ~/.codex/computer-use/Codex Computer Use.app, which is ${ctx.home ?? '/Users/<you>'}/.codex/computer-use/Codex Computer Use.app.`,
      'Finder hides folders whose names start with a dot. To look: in Finder choose Go > Go to Folder, type ~/.codex/computer-use and press Return.',
      'Or in Terminal run this; it prints the app if it is there:\nls -d ~/.codex/computer-use/*.app',
      'Paste the full path at the setup prompt, starting with /. Setup checks OpenAI\'s code signature before it copies the app into the VM.',
    ],
    sources: ['https://learn.chatgpt.com/docs/computer-use', 'https://support.apple.com/guide/mac-help/mchlp1236/mac'],
  },

  'provider-sign-in': (ctx, w) => ({
    title: 'Sign in to Codex or Claude',
    asked: 'Setup lists providers that are "found, not signed in"; sign-in happens in MiniClaw afterwards.',
    why: 'MiniClaw uses your own Codex or Claude Code sign-in. There is no separate MiniClaw account.',
    hint: ['Start MiniClaw, open Settings > Providers and press Sign in.'],
    steps: [
      `Start MiniClaw (${w.windows ? '.\\miniclaw.cmd' : 'miniclaw'}) and open the address setup printed, http://127.0.0.1:<port>.`,
      'Open Settings > Providers and press Sign in next to Codex (ChatGPT) or Claude Code.',
      'Codex (ChatGPT): finish the sign-in in the browser page that opens.',
      'Claude Code: Claude Code opens its sign-in page in your browser and MiniClaw notices the sign-in by itself. If nothing opened (the page may also be behind other windows), use "Open the Claude sign-in page" under Claude Code; that page ends with a code to paste into "Code from Claude", then press Finish sign-in.',
      'Cancel under Claude Code stops a Claude sign-in; if Claude Code stops with an error, MiniClaw shows it there. MiniClaw reuses the credentials Codex or Claude Code store themselves.',
      `Already signed in with the codex or claude command on ${w.machine}? Nothing more is needed.`,
    ],
    sources: ['https://developers.openai.com/codex/auth', 'https://docs.claude.com/en/docs/claude-code/setup'],
  }),
};

// Returns one guide for a platform. `ctx` may carry arch and home to fill in real paths.
export function guide(id, ctx = {}) {
  const build = builders[id];
  if (!build) throw new Error(`No guide named ${id}. Guides: ${topics.join(', ')}`);
  const context = { platform: 'darwin', ...ctx };
  return { id, ...build(context, words(context)) };
}

// Which guide explains an MCP server's environment value, by its name. No value has one; the MCP server page still
// asks, so the lookup stays.
export function guideForValue() { return null; }

// Which guide sets up a known server, by its saved name. No server has one.
export function guideForServer() { return null; }

// Says what a correct value looks like when one plainly is not. No value has a known shape, so always null.
export function valueProblem() { return null; }

// Splits a word longer than `room` after a path separator, so a long path breaks where a reader expects it.
function pieces(word, room) {
  if (word.length <= room || /^https:\/\//.test(word)) return [word];
  const parts = [];
  let rest = word;
  while (rest.length > room) {
    const cut = Math.max(rest.lastIndexOf('\\', room - 1), rest.lastIndexOf('/', room - 1));
    const at = cut > room / 3 ? cut + 1 : room;
    parts.push(rest.slice(0, at));
    rest = rest.slice(at);
  }
  return [...parts, rest];
}

// Breaks text into lines of at most `width`, indenting continuation lines. A URL longer than the width keeps its own line.
function wrap(text, width, indent) {
  const lines = [];
  let line = '';
  // A quoted command argument is one word, so a copied command never breaks inside its quotes when it fits.
  for (const word of text.match(/(?<=^|\s)'(?:[^']|'')*'[;,.)]*(?=\s|$)|(?<=^|\s)"[^"]*"[;,.)]*(?=\s|$)|\S+/g) ?? []) {
    for (const [index, part] of pieces(word, width - indent.length).entries()) {
      // A broken path continues on the next line without a space, so its pieces read as one path.
      if (index > 0) { lines.push(line); line = indent + part; continue; }
      if (line.trim() && line.length + 1 + part.length > width) { lines.push(line); line = indent + part; }
      else line = line ? `${line} ${part}` : part;
    }
  }
  if (line) lines.push(line);
  return lines;
}

// Formats a guide as numbered plain lines that fit a console of `columns` (no pager, no box). Prose is wrapped;
// command lines (the lines after the first in a step) are indented and left whole.
export function formatGuide(item, { columns = 80 } = {}) {
  const width = Math.max(40, columns - 8);
  const lines = [];
  item.steps.forEach((step, index) => {
    const number = `${index + 1}. `;
    const [prose, ...commands] = step.split('\n');
    lines.push(...wrap(number + prose, width, ' '.repeat(number.length)));
    // Commands stay whole on their own lines so they can be copied; a terminal soft-wraps a long one without breaking it.
    for (const command of commands) lines.push(`${' '.repeat(number.length + 2)}${command}`);
  });
  lines.push('', 'Source:');
  for (const source of item.sources) lines.push(`  ${source}`);
  return lines.join('\n');
}

// Prints one guide through the prompt library's plain log: no box and no hard wrapping, so commands copy whole.
// Static output, so a long guide simply scrolls; only the question asked after it is redrawn.
export function printGuide(ui, item, { columns = 80 } = {}) {
  ui.log.message(`${item.title}\n\n${formatGuide(item, { columns })}`);
}

// Lists topics with their first hint line, for `miniclaw guide` without a topic.
export function guideList(ctx = {}) {
  const items = topics.map(id => guide(id, ctx));
  const width = Math.max(...topics.map(id => id.length));
  return items.map(item => `${item.id.padEnd(width)}  ${item.title}`).join('\n');
}

// Builds docs/setup-guides.md: the inventory and every guide for both platforms.
export function guidesMarkdown() {
  const lines = ['# Setup guides', '',
    'Every value MiniClaw setup asks people to find or create, with the step-by-step guide setup shows for it.',
    'Generated from `scripts/setup/guides.mjs`; regenerate with `node scripts/setup/guides.mjs --markdown > docs/setup-guides.md`.',
    'The same text is shown by setup (choose "Show me how to find this" or type `?`), by `miniclaw guide <topic>`,',
    'and in Settings > MCP servers ("How do I find …?").', '',
    '## Inventory', '', '| Topic | When it is asked | Why it is needed |', '| --- | --- | --- |'];
  for (const id of topics) {
    const item = guide(id, { platform: 'darwin' });
    lines.push(`| \`${id}\` | ${item.asked.replaceAll('|', '\\|')} | ${item.why.replaceAll('|', '\\|')} |`);
  }
  for (const id of topics) for (const [platform, name] of [['darwin', 'macOS'], ['win32', 'Windows']]) {
    const item = guide(id, { platform });
    lines.push('', `## ${item.title.replace(/ \((Windows|macOS)\)$/, '')} (${name})`, '', `Topic \`${id}\`. ${item.hint.join(' ')}`, '');
    item.steps.forEach((step, index) => {
      const [prose, ...commands] = step.split('\n');
      lines.push(`${index + 1}. ${prose.replaceAll('<', '&lt;')}`);
      if (commands.length) lines.push('', '   ```', ...commands.map(command => `   ${command}`), '   ```', '');
    });
    lines.push('', `Sources: ${item.sources.map(source => `<${source}>`).join(', ')}`);
  }
  return `${lines.join('\n')}\n`;
}

// `node scripts/setup/guides.mjs --markdown` prints docs/setup-guides.md. The browser has no process, so this is inert there.
if (globalThis.process?.argv?.[1]?.replaceAll('\\', '/').endsWith('scripts/setup/guides.mjs') && globalThis.process.argv[2] === '--markdown') {
  globalThis.process.stdout.write(guidesMarkdown());
}
