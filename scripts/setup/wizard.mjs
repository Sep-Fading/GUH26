// Verifies a user-selected signed native Windows helper.
import { inspectWindowsComputerHelper } from './windows-computer.mjs';
// Renders terminal prompts and measured progress.
// Tells a release from a checkout.
import { prebuilt } from './prebuilt.mjs';
import * as ui from '@clack/prompts';
// Finds default per-user installation and caches.
import { homedir } from 'node:os';
// Checks existing destinations without changing them.
import { access, writeFile, rename } from 'node:fs/promises';
// Resolves explicit user-selected installation directory paths.
import { join, resolve } from 'node:path';
// Finds the release launcher beside these scripts.
import { fileURLToPath } from 'node:url';
// Starts the installed backend during VM setup.
import { spawn } from 'node:child_process';
// Uses pinned image sizes and checksums.
import { images } from './catalog.mjs';
// Computes resource recommendations and validates selections.
import { planSetup, recommend } from './plan.mjs';
// Keeps question routing separate from installation.
import { collectChoices } from './questions.mjs';
// Builds every panel the wizard shows.
import { budgetProblems, checks, failureNotice, installHint, machineName, nextSteps, rememberedNotice, remainingBytes, review, setupHelp, stageList } from './view.mjs';
// Finds downloads that are already cached.
import { cachedItems } from './download.mjs';
// Reads what other VMs already reserve.
import { readLedger } from '../vm/ledger.mjs';
// Shows numbered steps, download bars and build logs.
import { createReporter } from './reporter.mjs';
// Finds Codex and Claude and their sign-in state.
import { detectProviders } from './providers.mjs';
// Installs a missing Claude Code in the background.
import { claudeInstallable, startClaude } from './claude-install.mjs';
// Finds, downloads and installs Microsoft's Visual C++ runtime on Windows.
import { installVcRuntime, startVcRuntime, vcRuntimeArch, vcRuntimeStatus } from './vc-runtime.mjs';
// Detects hardware, dependencies and port conflicts.
import { checkPort, executable, inspectHost } from './system.mjs';
// Refuses existing, relative, shared or device destinations; recognises a finished installation.
import { destinationError, keptData, readInstallation } from './destination.mjs';
// Applies only explicitly approved installation plans.
import { install } from './install.mjs';
// Runs first-time VM setup and guest sign-in.
import { connectSetup } from './connect.mjs';
// Explains what an installation can resume.
import { readReceipt } from './receipt.mjs';
// Lets later commands run without a path.
import { defaultInstallation, rememberInstallation, installationPaths } from './default-installation.mjs';
// Reports unexpected termination after successful guest setup.
import { waitStopped } from '../vm/readiness.mjs';
// Verifies the chosen helper before plan approval.
import { inspectComputerApp } from '../vm/computer-source.mjs';
// Finds the Computer Use helper before asking for it.
import { detectSetupValues } from './detect.mjs';
// Prints the step-by-step guide a failed check names.
import { showGuide } from './questions.mjs';
// Prefills the actual global resource settings.
import { measure, readResourceLimits, registry } from '../vm/resource-admission.mjs';

// Setup run again on an installation: says so and changes nothing. `miniclaw update` updates it.
function existingSetup(ui, { destination }) {
  ui.log.info(`MiniClaw is already installed at ${destination}.\n`
    + 'To update MiniClaw itself, run: miniclaw update. To install a separate copy, run: miniclaw setup "<new folder>"');
  ui.outro?.('Nothing was changed.');
  return 'unchanged';
}

// Throws on cancellation so no prompt symbol can reach the installer.
async function answer(promise) {
  const value = await promise;
  if (ui.isCancel(value)) throw new Error('Setup cancelled');
  return value;
}

// Reads what this Mac offers: hardware, build tools, the sandbox and both providers.
async function inspect(destination) {
  const [host, providers, cargo, detected] = await Promise.all([
    inspectHost(destination), detectProviders(), executable('cargo'), detectSetupValues()]);
  const seatbelt = await access('/usr/bin/sandbox-exec').then(() => true, () => false);
  // Read-only registry check; nothing is downloaded here, so previews stay offline.
  const vcArch = vcRuntimeArch(host.platform, host.arch);
  const vcRuntime = vcArch ? await vcRuntimeStatus({ arch: vcArch }) : null;
  return { host, providers, detected, tools: { node: process.version, cargo, seatbelt, vcRuntime, prebuilt: await prebuilt.available() } };
}

// Starts the installed backend with the session's private links and the wizard's providers.
function launchBackend(destination, settings) {
  return async links => {
    const data = join(destination, 'data');
    const child = spawn(join(destination, `runtime/bin/miniclaw-server${process.platform === 'win32' ? '.exe' : ''}`), [], {
      cwd: data, stdio: 'ignore', env: { ...process.env,
        MINICLAW_PORT: String(settings.port), MINICLAW_DATA_DIR: data,
        MINICLAW_CHAT_DIR: join(data, 'chat-workspace'), MINICLAW_VM_LINKS: links,
        ...(process.platform === 'win32' ? { MINICLAW_NETWORK_CONFIG: join(data, 'windows-network.json') } : {}),
        CODEX_BIN: settings.codexBin ?? '', CLAUDE_BIN: settings.claudeBin ?? '',
        ...(settings.windowsComputerHelper ? { MINICLAW_HELPER_BIN: settings.windowsComputerHelper } : {}),
        MINICLAW_WEB_DIR: join(destination, 'runtime/web') } });
    await new Promise((done, fail) => { child.once('spawn', done); child.once('error', fail); });
    return { close: () => new Promise(done => {
      if (child.exitCode !== null || child.signalCode !== null) return done();
      child.once('close', done);
      child.kill('SIGTERM');
    }) };
  };
}

// Completes VM onboarding and guest sign-in, retaining the installation on interruption.
async function finishVm(result, destination, signal) {
  const running = await connectSetup(result.pair, { signal,
    launch: launchBackend(destination, result),
    computerUse: result.masterSetup?.computerUse === true,
    onOutput: process.env.MINICLAW_VM_DIAGNOSTICS === '1'
      ? chunk => process.stderr.write(chunk) : undefined,
    onStatus(state) {
      ui.log.step(state === 'ready' ? 'The VM worker is connected.'
        : state === 'worker' ? 'Waiting for Codex inside the VM. Unlock the VM if it shows a login screen.'
        : 'Starting MiniClaw on this Mac.');
    },
    async macSignIn(url) {
      ui.note(`Open ${url}\nSign in to Codex or Claude there. Setup continues by itself.`,
        'Sign in on this Mac first');
    },
    async guestSetup(pending) {
      ui.note('In the VM window, finish macOS setup if it asks.\n'
        + 'Open MiniClawSetup in Finder and run Install Codex.command.\n'
        + (result.masterSetup?.computerUse
          ? 'Computer Use is included. Approve Accessibility and screen recording inside the VM.\n' : '')
        + 'Confirm below when that is done.', 'Finish setup inside the VM');
      return answer(ui.confirm({ message: 'VM setup complete?', signal: pending,
        initialValue: false, active: 'Yes, check the VM', inactive: 'Not yet, keep the files' }));
    },
    showCode({ url, code }) {
      ui.note(`${url}\nCode  ${code}\n\nThis signs in Codex inside the VM, separately from this Mac.`,
        'Sign in to Codex in the VM');
    },
  });
  try {
    signal.throwIfAborted();
    const saved = { ...result, status: 'installed', vmReady: true,
      masterSetup: { ...result.masterSetup, status: 'installed', oauthReady: true } };
    const staging = join(destination, 'settings.ready.json');
    await writeFile(staging, `${JSON.stringify(saved, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
    await rename(staging, join(destination, 'settings.json'));
    ui.outro(`MiniClaw is running at http://127.0.0.1:${result.port}. Press Ctrl-C to stop it and the VM.`);
    await waitStopped(running, signal);
  } finally {
    ui.log.info('Stopping. The VM shuts down by itself; one that has not after 90 seconds is powered off.');
    process.exitCode = await running.close() ?? process.exitCode;
  }
}

// Runs a body with Ctrl-C and termination wired to one abort signal.
async function interruptible(body) {
  const controller = new AbortController();
  const abort = () => controller.abort();
  process.on('SIGINT', abort); process.on('SIGTERM', abort);
  try { return await body(controller.signal); }
  finally { process.off('SIGINT', abort); process.off('SIGTERM', abort); }
}

// Drives the reviewed terminal workflow; previews perform no writes or downloads.
async function main() {
  const args = process.argv.slice(2);
  let preview = false, macOnly = false, resuming = false, explicitDestination = false, keep = null;
  let destination = installationPaths().installation;
  for (let index = 0; index < args.length; index++) {
    if (args[index] === '--preview') preview = true;
    else if (args[index] === '--resume') resuming = true;
    else if (args[index] === '--mac-only' || args[index] === '--host-only') macOnly = true;
    else if (args[index] === '--destination' && args[index + 1]) {
      destination = resolve(args[++index]); explicitDestination = true;
    } else if (args[index] === '--help') {
      for (const line of setupHelp({ release: await prebuilt.available() })) console.log(line);
      return;
    } else throw new Error('Unknown setup option; use --help');
  }
  const cache = installationPaths().cache;
  const interactive = process.stdin.isTTY && process.stdout.isTTY;
  if (resuming) {
    if (preview || macOnly) throw new Error('Resume cannot be combined with preview or Mac-only');
    if (process.platform === 'win32') {
      const { pendingWindowsSetup, resumeWindows } = await import('./resume-windows.mjs');
      // Explain what can be resumed before asking for a terminal, as on the Mac. A finished installation is a no-op.
      try { await pendingWindowsSetup(destination); }
      catch (error) { if (!error.nothingToResume) throw error; console.log(error.message); return; }
      if (!interactive) throw new Error('Interactive terminal required');
      ui.intro('MiniClaw setup: recover Windows installation');
      await resumeWindows(destination, { onOutput: text => ui.log.info(text.trim()) });
      await rememberInstallation(destination);
      return ui.outro(`Installation recovered. Start with: miniclaw start "${destination}"`);
    }
    // Explain what can be resumed before asking for a terminal.
    const saved = await readReceipt(destination);
    if (!interactive) throw new Error('Interactive terminal required');
    ui.intro('MiniClaw setup: finish the VM');
    if (!await answer(ui.confirm({ message: `Start MiniClaw and its VM on port ${saved.port}?`,
      initialValue: false }))) return ui.cancel('Nothing was started');
    await checkPort(saved.port);
    return interruptible(signal => finishVm(saved, destination, signal));
  }
  if (preview) {
    const facts = await inspect(destination);
    const defaults = recommend(facts.host);
    const enabled = { codex: facts.providers.some(provider => provider.id === 'codex'),
      claude: facts.providers.some(provider => provider.id === 'claude')
        || claudeInstallable(facts.host.platform, facts.host.arch) };
    console.log(checks(facts));
    if (!enabled.codex && !enabled.claude) {
      console.log('\nInstall Codex or Claude Code first. MiniClaw needs at least one provider.');
      return;
    }
    const plan = planSetup({ port: defaults.port, providers: enabled, vmEnabled: false,
      master: null, limits: defaults.limits }, facts.host, images);
    console.log(`\n${review(plan, { destination, cache })}`);
    console.log('\nPreview only. Nothing was changed. The optional VM is chosen during interactive setup.');
    return;
  }
  if (!interactive) throw new Error('Interactive terminal required; use --preview to inspect defaults');
  ui.intro('MiniClaw setup');
  const checking = ui.spinner();
  // macOS keeps its original wording; Windows uses the neutral name.
  const machine = machineName();
  checking.start(`Checking ${machine}`);
  let facts = await inspect(destination);
  checking.stop(`Checked ${machine}`);
  ui.note(checks(facts), machine.replace('this', 'This'));
  // Run again on an installation (the remembered one, or the folder named): say so and stop.
  // Nothing of MiniClaw itself is built or installed again; `miniclaw update` does that.
  if (!explicitDestination) {
    const remembered = await defaultInstallation();
    if (await readInstallation(remembered)) destination = remembered;
  }
  const installed = await readInstallation(destination);
  if (installed) return existingSetup(ui, { destination });
  if (!facts.tools.cargo && !await prebuilt.available()) throw new Error('Rust is required to build MiniClaw. Install it from rustup.rs, then rerun setup. Nothing was changed.');
  // A missing Claude Code installs in the background while the next questions are asked.
  const claude = startClaude(ui, facts.providers, { platform: facts.host.platform, arch: facts.host.arch, detect: detectProviders });
  // A missing Microsoft runtime downloads meanwhile too; it is installed with the approved plan.
  const vcRuntime = startVcRuntime(ui, facts.tools.vcRuntime, { platform: facts.host.platform });
  try {
  if (!claude && !facts.providers.length) throw new Error('Install Codex or Claude Code first, then rerun setup. Nothing was changed.');
  if (facts.host.platform === 'darwin' && !facts.tools.seatbelt) ui.log.warn('Seatbelt is missing, so agents will not get shell commands.');
  if (!explicitDestination) destination = await answer(ui.text({
    message: 'Where should MiniClaw be installed?',
    initialValue: destination, validate: destinationError,
  }));
  const invalid = await destinationError(destination);
  if (invalid) throw new Error(invalid);
  // The folder typed holds an installation: say so, without installing anything.
  const typed = await readInstallation(destination);
  if (typed) {
    await claude?.cancel(); await vcRuntime?.cancel();
    return existingSetup(ui, { destination });
  }
  // A folder `miniclaw uninstall` left with the user's data is installed into again only when they agree.
  keep = await keptData(destination);
  if (keep && !await answer(ui.confirm({ message: `${destination} holds the data MiniClaw kept when it was uninstalled `
    + '(conversations, projects, settings). Install MiniClaw there again and keep that data?', initialValue: true }))) {
    throw new Error('Setup cancelled. Nothing was changed. Run setup again and choose a new folder.');
  }
  } catch (error) { await claude?.cancel(); await vcRuntime?.cancel(); throw error; }
  // The provider questions come next, so wait for the background installation here.
  if (claude) facts = { ...facts, providers: await claude.finish().catch(async error => { await vcRuntime?.cancel(); throw error; }) };
  // MiniClaw's Windows binaries cannot run without the runtime, so a failed download stops here.
  let vcDownload = null;
  if (vcRuntime) {
    const waiting = ui.spinner();
    waiting.start('Downloading the Microsoft Visual C++ runtime installer from Microsoft');
    try { vcDownload = await vcRuntime.finish(); } catch (error) { waiting.stop('The Microsoft Visual C++ runtime installer is not available'); throw error; }
    waiting.stop('Microsoft Visual C++ runtime installer downloaded; Microsoft\'s signature checked');
  }
  if (!facts.providers.length) throw new Error('Install Codex or Claude Code first, then rerun setup. Nothing was changed.');
  facts = { ...facts, host: await inspectHost(destination) };
  let previous = macOnly || facts.host.platform === 'win32' ? {} : { limits: readResourceLimits() };
  const imageCached = facts.host.platform !== 'win32' && (await cachedItems(images, cache)).has('macos');
  let plan, cached, ledger;
  while (true) {
    let choice = await collectChoices(ui, facts.host, previous,
      { hostOnly: macOnly, providers: facts.providers, imageCached, detected: facts.detected });
    previous = choice;
    try {
      const computer = choice.computerApp ? await inspectComputerApp(choice.computerApp) : null;
      const windowsComputer = choice.windowsComputerHelper ? await inspectWindowsComputerHelper(choice.windowsComputerHelper) : null;
      if (windowsComputer) choice = { ...choice, windowsComputerHelper: windowsComputer.path, windowsComputer };
      // A release needs no Go toolchain download: its network helper is already built.
      plan = planSetup({ ...choice, computer, prebuilt: await prebuilt.available() }, facts.host, images);
      if (vcDownload) plan = { ...plan, vcRuntime: vcDownload };
      await checkPort(plan.port);
      if (plan.master) for (const name of ['xcrun', 'python3']) {
        if (!await executable(name)) throw new Error(`${name} is required to build the VM. Install the Xcode command line tools.`);
      }
      cached = await cachedItems([...plan.images, ...plan.packages], cache);
      // Other VMs on this Mac share one budget; find out now, not after a long build.
      const view = plan.master ? readLedger(registry(), () => measure(join(registry(), '..'))) : null;
      ledger = view && { used: view.used, capacity: view.capacity };
      const problems = budgetProblems(plan, ledger);
      if (problems.length) throw new Error(`The VM would not be allowed to start.\n${problems.join('\n')}\n`
        + 'Run `miniclaw ledger` to see what holds the budget, or choose a smaller VM.');
    } catch (error) {
      ui.log.warn(error.message);
      // A check that knows what the user must find or install prints its guide before offering to change the choices.
      if (error.guide) showGuide(ui, error.guide, { platform: facts.host.platform, arch: facts.host.arch, home: homedir() });
      if (!await answer(ui.confirm({ message: 'Change these choices?', initialValue: true }))) {
        return ui.cancel('Setup cancelled. Nothing was changed.');
      }
      continue;
    }
    ui.note(review(plan, { destination, cache, cached, ledger }), 'Review');
    const next = await answer(ui.select({ message: 'Ready to install?', initialValue: 'install', options: [
      { value: 'install', label: plan.master ? 'Install MiniClaw and prepare the VM' : 'Install MiniClaw',
        hint: !plan.master ? installHint({ release: await prebuilt.available() })
          : remainingBytes(plan, cached) > 1024 ** 3 ? 'large download; can be resumed'
            : 'macOS installs inside the VM; this is the long part' },
      { value: 'edit', label: 'Change choices' },
      { value: 'cancel', label: 'Cancel', hint: 'nothing is changed' },
    ] }));
    if (next === 'cancel') return ui.cancel('Nothing was changed.');
    if (next === 'install') break;
  }
  const binary = id => facts.providers.find(provider => provider.id === id)?.path ?? null;
  let restartRecommended = false;
  await interruptible(async signal => {
    const report = createReporter(ui, stageList(plan));
    let result;
    try {
      result = await install({ plan, destination, cache, approved: true, signal, keep,
        codexBin: binary('codex'), claudeBin: binary('claude'),
        installRuntime: async options => { const done = await installVcRuntime(options); restartRecommended ||= done.restart; return done; },
        onStage: report.onStage, onProgress: report.onProgress, onOutput: report.onOutput });
      report.finish();
    } catch (error) {
      report.fail(signal.aborted ? 'cancelled' : 'stopped', { showLog: !signal.aborted });
      ui.log.info(failureNotice(error));
      throw signal.aborted ? new Error('Setup cancelled') : error;
    }
    ui.log.success('MiniClaw is installed.');
    if (restartRecommended) ui.log.warn('Restart Windows when convenient: Microsoft\'s Visual C++ runtime installer asked for a restart to finish.');
    // Another installation that commands without a path used until now is named, never replaced silently.
    const previous = await defaultInstallation();
    const earlier = await access(join(previous, 'settings.json')).then(() => previous, () => null);
    await rememberInstallation(destination);
    const notice = rememberedNotice(earlier, destination);
    if (notice && process.platform === 'win32') ui.log.warn(notice);
    if (result.pair) {
      ui.log.info('If setup is interrupted, continue with:  miniclaw resume');
      return finishVm(result, destination, signal);
    }
    // A Windows release starts from its own launcher, beside these scripts.
    const launcher = fileURLToPath(new URL('../../miniclaw.cmd', import.meta.url));
    ui.note(nextSteps({ destination, port: result.port, providers: facts.providers,
      enabled: plan.providers, vm: false,
      launcher: process.platform === 'win32' && await access(launcher).then(() => true, () => false) ? launcher : undefined }), 'Next steps');
    ui.outro('Setup complete.');
  });
}

main().catch(error => {
  ui.cancel(error.message);
  process.exitCode = /cancel|abort/i.test(error.message) ? 130 : 1;
});
