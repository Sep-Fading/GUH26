// Names stages and formats byte counts.
import { bytes, stageLabel } from './view.mjs';

// Turns installer callbacks into numbered steps: a progress bar per download and a
// build log that clears on success and stays on failure. `ui` is the prompt library.
export function createReporter(ui, stages, { clock = Date.now, columns = process.stdout.columns ?? 80 } = {}) {
  // A wrapped line would break the log's redraw, so lines fit the terminal.
  const width = Math.max(40, Math.min(120, columns - 8));
  let open = null;
  let buffer = '';
  let transfer = null;
  // Closes whatever indicator is open with its success text.
  const settle = () => {
    if (open?.kind === 'bar' || open?.kind === 'percent') open.handle.stop(open.done ?? 'Completed');
    if (open?.kind === 'log') open.handle.success(open.title);
    open = null;
  };
  const step = stage => `Step ${Math.max(1, stages.indexOf(stage) + 1)} of ${stages.length}`;
  return {
    // Starts one stage; every download is a bar under the same step number.
    onStage(stage, image) {
      settle();
      buffer = '';
      if (stage === 'download') {
        const title = `${step(stage)}  ${image.name ?? image.id}`;
        const handle = ui.progress({ style: 'heavy', max: 1000, size: 30 });
        handle.start(title);
        open = { kind: 'bar', handle, title };
        transfer = { permille: 0, start: clock(), first: null };
        return;
      }
      const title = `${step(stage)}  ${stageLabel(stage)}`;
      if (stage === 'install-macos') {
        // The installer reports a percentage, so one bar updates in place.
        const handle = ui.progress({ style: 'heavy', max: 100, size: 30 });
        handle.start(title);
        open = { kind: 'percent', handle, title, percent: 0, last: '', done: `${title}: macOS installed` };
        return;
      }
      open = { kind: 'log', handle: ui.taskLog({ title, limit: 6 }), title };
    },
    // Moves the bar by what arrived since the last report and estimates the time left.
    onProgress(image, value) {
      if (open?.kind !== 'bar') return;
      const name = image.name ?? image.id;
      if (value.verified) { open.done = `${name}: checksum verified`; return; }
      if (value.phase === 'verify') { open.handle.message(`${name}: verifying checksum`); return; }
      transfer.first ??= { received: value.received, at: clock() };
      const permille = Math.floor(value.received / value.total * 1000);
      const seconds = Math.max(0.001, (clock() - transfer.first.at) / 1000);
      const rate = (value.received - transfer.first.received) / seconds;
      const left = rate > 0 ? ` | ~${Math.ceil((value.total - value.received) / rate)}s left` : '';
      open.handle.advance(permille - transfer.permille,
        `${bytes(value.received)} of ${bytes(value.total)} | ${bytes(Math.max(0, rate))}/s${left}`);
      transfer.permille = permille;
    },
    // Streams build output into the log as short plain lines.
    onOutput(chunk) {
      if (open?.kind !== 'log' && open?.kind !== 'percent') return;
      buffer = (buffer + chunk).slice(-16384);
      const lines = buffer.split('\n');
      buffer = lines.pop();
      for (const raw of lines) {
        // eslint-disable-next-line no-control-regex
        const line = raw.replace(/\u001b\[[0-9;]*[A-Za-z]/g, '').trim();
        if (!line) continue;
        let percent;
        try { const value = JSON.parse(line); if (value.status === 'installing') percent = value.percent; }
        catch { /* Ordinary build output. */ }
        if (open.kind === 'percent') {
          // Other output is kept only to explain a failure.
          if (!Number.isInteger(percent)) { open.last = line.slice(0, width); continue; }
          const next = Math.min(100, Math.max(open.percent, percent));
          if (next > open.percent) open.handle.advance(next - open.percent, `macOS installation ${next}%`);
          open.percent = next;
          continue;
        }
        open.handle.message(line.slice(0, width));
      }
    },
    // Closes the last stage after a successful installation.
    finish() { settle(); },
    // Leaves the failing stage's log on screen; a cancellation has no log worth reading.
    fail(message, { showLog = true } = {}) {
      if (open?.kind === 'log') open.handle.error(`${open.title}: ${message}`, { showLog });
      if (open?.kind === 'bar') open.handle.error(`${open.title}: ${message}`);
      if (open?.kind === 'percent') {
        open.handle.error(`${open.title}: ${message}${open.last ? `\n${open.last}` : ''}`);
      }
      open = null;
    },
  };
}
