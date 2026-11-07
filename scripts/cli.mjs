#!/usr/bin/env node
// Reads the selected installation before startup.
import { readFile } from 'node:fs/promises';
// Resolves explicit installation and launcher paths.
import { join, resolve } from 'node:path';
// Resolves bundled commands independently of cwd.
import { fileURLToPath } from 'node:url';
// Runs only the selected trusted command.
import { spawn } from 'node:child_process';
// Remembers where MiniClaw was installed.
import { defaultInstallation } from './setup/default-installation.mjs';

const usage = `miniclaw                      start MiniClaw, or explain how to install it
miniclaw setup [PATH]         install (add --preview to look, --mac-only to skip the VM)
miniclaw start [PATH]         start an installation
miniclaw resume [PATH]        finish an interrupted VM setup
miniclaw repair [PATH]        set up network access for sandboxed tools again (Windows)
miniclaw uninstall [PATH]     remove MiniClaw and its network service from Windows; keeps your data
                              (add --delete-data to delete the data folder too)
miniclaw ledger [--reconcile] show or repair the shared VM budget
                              (--forget drops guests you deleted from disk)
miniclaw package [FOLDER]     build a self-contained release folder and .tar.gz (from a checkout)
miniclaw verify [--chat]      check a running MiniClaw and its VM, feature by feature
miniclaw update [PATH]        rebuild the backend, the page and the VM scripts in place
miniclaw update-vm [PATH]     rebuild the VM's setup disk after a MiniClaw update
                              (add --computer-use to put the Computer Use helper in the VM)
miniclaw where                print the installation these commands use
miniclaw guide [TOPIC]        step-by-step help to find a path or create a key setup asks for
                              (add --mac or --windows for the other computer's steps)
PATH is only needed for an installation other than the remembered one.`;

// Runs a command with inherited terminal IO and forwards graceful shutdown signals.
async function launch(path, args = []) {
  const child = spawn(process.execPath, [path, ...args], { stdio: 'inherit' });
  const stop = () => child.kill('SIGTERM');
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, stop);
  try {
    process.exitCode = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', code => resolve(code ?? 1));
    });
  } finally {
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.off(signal, stop);
  }
}

// Returns the script beside this file.
const script = path => fileURLToPath(new URL(path, import.meta.url));

// Takes an installation path given plainly or with its older flag, else the remembered one.
async function installation(args, flag) {
  if (args[0] === flag && args.length === 2) return resolve(args[1]);
  if (args.length === 1 && !args[0].startsWith('-')) return resolve(args[0]);
  if (args.length) throw new Error(`Use miniclaw --help`);
  return defaultInstallation();
}

// Starts one ready installation; never falls back to a different backend.
async function start(root, explicit) {
  const settings = await readFile(join(root, 'settings.json'), 'utf8').then(JSON.parse, error => {
    if (error.code !== 'ENOENT') throw error;
    throw new Error(explicit ? `No installation at ${root}.`
      : 'Nothing is installed yet. Run: miniclaw setup');
  });
  if (settings.schemaVersion !== 1) throw new Error('Unsupported installation settings');
  if (settings.backend === 'linux') {
    throw new Error('This installation uses the retired Linux backend. Run miniclaw setup to install MiniClaw on this Mac.');
  }
  if (settings.backend !== 'host' || settings.hostReady !== true) {
    throw new Error('This installation is not ready');
  }
  return launch(join(root, 'runtime/start-host.mjs'));
}

// Prints one guide, or the list of guides, for this computer or the one named by --mac/--windows.
// Real paths are filled in where they exist: this computer's processor and home folder.
async function printGuideCommand(args) {
  const { guide, guideList, formatGuide, topics } = await import('./setup/guides.mjs');
  const platform = args.includes('--windows') ? 'win32' : args.includes('--mac') ? 'darwin' : process.platform === 'win32' ? 'win32' : 'darwin';
  const topic = args.find(arg => !arg.startsWith('-'));
  if (!topic) {
    console.log(`Guides (miniclaw guide TOPIC):\n${guideList({ platform })}`);
    return;
  }
  if (!topics.includes(topic)) throw new Error(`No guide named ${topic}. Guides: ${topics.join(', ')}`);
  const here = platform === (process.platform === 'win32' ? 'win32' : 'darwin');
  const ctx = { platform, ...(here ? await localContext() : {}) };
  const item = guide(topic, ctx);
  console.log(`${item.title}\n\n${formatGuide(item, { columns: process.stdout.columns ?? 80 })}`);
}

// Fills guides with this computer's real paths: processor and home folder. Never reads secrets.
async function localContext() {
  const { homedir } = await import('node:os');
  return { arch: process.arch, home: homedir() };
}

// Dispatches each short command to the script that does the work.
async function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === '--help' || command === 'help') return console.log(usage);
  if (!command) {
    return start(await defaultInstallation(), false).catch(error => {
      if (!/Nothing is installed yet/.test(error.message)) throw error;
      console.log(`MiniClaw is not installed yet. Run: miniclaw setup\n\n${usage}`);
    });
  }
  if (command === 'setup') {
    // A plain path is the destination; every other option passes through.
    // Only something that looks like a path counts, so a stray word is still an error.
    const plain = args.findIndex((arg, index) => !arg.startsWith('-') && /[/\\]/.test(arg)
      && args[index - 1] !== '--destination');
    const forwarded = plain < 0 ? args
      : [...args.slice(0, plain), '--destination', args[plain], ...args.slice(plain + 1)];
    return launch(script('./setup/wizard.mjs'), forwarded);
  }
  if (command === 'resume') {
    return launch(script('./setup/wizard.mjs'),
      ['--resume', '--destination', await installation(args, '--destination')]);
  }
  if (command === 'repair') {
    const { repairWindows } = await import('./setup/repair-windows.mjs');
    if (process.platform !== 'win32') throw new Error('miniclaw repair sets up the Windows network service again; this computer does not use it.');
    await repairWindows(await installation(args, '--destination'));
    return;
  }
  if (command === 'uninstall') {
    const deleteData = args.includes('--delete-data');
    if (process.platform !== 'win32') throw new Error('miniclaw uninstall removes MiniClaw from Windows. On a Mac, delete the installation folder.');
    const destination = await installation(args.filter(arg => arg !== '--delete-data'), '--destination');
    if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error('Interactive terminal required: miniclaw uninstall asks before it removes anything.');
    const ui = await import('@clack/prompts');
    const { uninstallWindows } = await import('./setup/uninstall-windows.mjs');
    ui.intro('MiniClaw uninstall');
    try {
      await uninstallWindows(destination, { deleteData, onOutput: text => ui.log.info(text.trim()),
        confirm: async message => (await ui.confirm({ message, initialValue: false })) === true });
    } catch (error) { ui.cancel(error.message); process.exitCode = 1; return; }
    return ui.outro('Done.');
  }
  if (process.platform === 'win32' && ['ledger', 'vm', 'update-vm'].includes(command)) {
    throw new Error(`miniclaw ${command} manages the macOS VM. MiniClaw on Windows has no VM.`);
  }
  if (command === 'ledger' || (command === 'vm' && args[0] === 'ledger')) {
    return launch(script('./vm/ledger-cli.mjs'), command === 'vm' ? args.slice(1) : args);
  }
  if (command === 'package') return launch(script('./package-release.mjs'), args);
  if (command === 'verify') return launch(script(process.platform === 'win32' ? './setup/verify-host.mjs' : './vm/verify-live.mjs'), args);
  if (command === 'update') return launch(script('./setup/update-runtime.mjs'), args);
  if (command === 'update-vm') return launch(script('./vm/update-media.mjs'), args);
  if (command === 'where') return console.log(await defaultInstallation());
  if (command === 'guide') return printGuideCommand(args);
  if (command !== 'start') throw new Error('Unknown command; use miniclaw --help');
  return start(await installation(args, '--installation'), args.length > 0);
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
