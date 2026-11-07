<script lang="ts">
  // Explains the restart button.
  import { Tooltip } from 'bits-ui';
  // Marks the restart command.
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  // Opens the stream and types into the shell.
  import { openTerminalStream, terminalCall } from './terminal-stream';
  // The conversation the shell belongs to.
  import type { Chat } from './chat.svelte';
  // Cleans up the terminal with the tab.
  import { onMount } from 'svelte';

  let { chat }: { chat: Chat } = $props();
  let host = $state<HTMLDivElement>();
  let status = $state<'opening' | 'live' | 'ended' | 'unavailable'>('opening');
  let why = $state(''), notice = $state('');
  let term: { write: (data: Uint8Array | string) => void; clear: () => void; focus: () => void; dispose: () => void;
    onData: (handle: (data: string) => void) => void; cols: number; rows: number; attachCustomKeyEventHandler: (handle: (event: KeyboardEvent) => boolean) => void } | undefined;
  let fit: { fit: () => void } | undefined;
  let stream: { close: () => void } | undefined;
  let seq = 0;
  const thread = $derived(chat.active?.id ?? '');

  // Sends the size the terminal has after a fit, at most a few times a second.
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  function fitted() {
    if (!term || !fit) return;
    fit.fit();
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { if (thread && status === 'live' && term) void terminalCall(thread, 'resize', { cols: term.cols, rows: term.rows }).catch(() => {}); }, 150);
  }

  // Opens the shell and its stream; a shell that ended shows why and offers a restart.
  async function open() {
    if (!term || !thread) return;
    status = 'opening'; why = ''; notice = '';
    fit?.fit();
    try {
      const opened = await terminalCall<{ seq: number; status: 'live' | 'ended' }>(thread, 'open', { cols: term.cols, rows: term.rows });
      seq = 0;
      term.clear();
      status = opened.status;
      stream?.close();
      stream = openTerminalStream(thread, seq, {
        output: bytes => term?.write(bytes),
        reset: () => term?.clear(),
        exit: code => { status = 'ended'; why = `The shell ended with code ${code}.`; },
        notice: text => { notice = text; },
        error: () => { if (status === 'live') { status = 'unavailable'; why = 'The connection to the shell was lost.'; } },
      });
    } catch (cause) {
      status = 'unavailable';
      why = cause instanceof Error ? cause.message : 'The terminal could not be opened.';
    }
  }

  async function restart() {
    if (!thread) return;
    try { await terminalCall(thread, 'restart', {}); await open(); }
    catch (cause) { status = 'unavailable'; why = cause instanceof Error ? cause.message : 'The shell could not be restarted.'; }
  }

  onMount(() => {
    let observer: ResizeObserver | undefined;
    let disposed = false;
    // xterm and its stylesheet are fetched here, not at module load: jsdom cannot load them, and
    // nothing else in MiniClaw needs them.
    void (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit'), import('@xterm/xterm/css/xterm.css')]);
      if (disposed || !host) return;
      const made = new Terminal({ cursorBlink: true, fontSize: 13, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', scrollback: 5000,
        // Every colour reads at 4.5:1 on the background, the prompt's blue included; xterm's own
        // defaults are too faint for that, and axe checks the terminal like any other text.
        theme: { background: '#0b0b0c', foreground: '#e6e6e6', cursor: '#e6e6e6', selectionBackground: '#3b4252',
          black: '#5c6370', red: '#ff7b72', green: '#7ee787', yellow: '#e3b341', blue: '#79c0ff', magenta: '#d2a8ff', cyan: '#76e3ea', white: '#e6e6e6',
          brightBlack: '#8b949e', brightRed: '#ffa198', brightGreen: '#aff5b4', brightYellow: '#f2cc60', brightBlue: '#a5d6ff', brightMagenta: '#e2c5ff',
          brightCyan: '#b3f0ff', brightWhite: '#ffffff' }, allowProposedApi: false });
      const fitting = new FitAddon();
      made.loadAddon(fitting);
      made.open(host);
      term = made; fit = fitting;
      made.onData(data => { if (thread && status === 'live') void terminalCall(thread, 'input', { text: data }).catch(() => {}); });
      // Cmd shortcuts belong to the page and the browser; Escape then Tab leaves the terminal.
      made.attachCustomKeyEventHandler(event => !(event.metaKey && !event.ctrlKey && !event.altKey));
      observer = new ResizeObserver(() => fitted());
      observer.observe(host);
      fitted();
      await open();
      made.focus();
    })();
    return () => { disposed = true; observer?.disconnect(); stream?.close(); term?.dispose(); clearTimeout(resizeTimer); };
  });
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <div class="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3 text-xs text-muted-foreground">
    <span role="status" class="min-w-0 flex-1 truncate">
      {#if status === 'opening'}Starting the shell...
      {:else if status === 'live'}{notice || 'Shared with the agent: what it runs after your approval runs here too.'}
      {:else}{why}{/if}
    </span>
    <Tooltip.Root>
      <Tooltip.Trigger class="icon-button" aria-label="Restart shell" disabled={status === 'opening' || !thread} onclick={restart}>
        <RotateCcw size={14} aria-hidden="true" />
      </Tooltip.Trigger>
      <Tooltip.Portal><Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>Restart the shell with an empty screen</Tooltip.Content></Tooltip.Portal>
    </Tooltip.Root>
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div bind:this={host} role="region" aria-label="Terminal" tabindex="0" class="terminal-host min-h-0 flex-1 bg-[#0b0b0c] p-1"></div>
</div>

<style>
  .terminal-host :global(.xterm) { height: 100%; }
  .terminal-host :global(.xterm-viewport) { overflow-y: auto; }
</style>
