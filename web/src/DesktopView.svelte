<script lang="ts">
  // Moves files between this Mac and the VM's offered folders.
  import Transfers from './Transfers.svelte';
  // Polls VM status while mounted.
  import { onMount } from 'svelte';
  // Labels window and shutdown buttons.
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Power from '@lucide/svelte/icons/power';
  // Marks a screen without a picture.
  import MonitorOff from '@lucide/svelte/icons/monitor-off';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  // Folds the screen away on a narrow window.
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  // Uses the selected VM identity.
  import type { Chat } from './chat.svelte';
  // Reads the desktop stream and reports when it ends.
  import { readFrames } from './desktop-stream';

  // `embedded` is the panel's VM tab: the panel supplies the frame, width and border.
  let { chat, embedded = false }: { chat: Chat; embedded?: boolean } = $props();
  let error = $state('');
  let noPicture = $state(false);
  // Until the first frame arrives the screen keeps a standard shape, so nothing jumps.
  let sized = $state(false);
  let waiting = $state(false);
  // A narrow window stacks the screen above the chat, where the user may fold it away.
  let folded = $state(false);
  // Frames are painted here. A canvas needs no image URL, so the page's
  // Content-Security-Policy can keep refusing blob: and other foreign image sources.
  let screen = $state<HTMLCanvasElement>();
  let painting = false;

  // Names the VM state in words a person would use.
  const label = $derived(chat.vmStatus === 'running' ? (chat.vmReady ? 'Running' : 'Waiting for login')
    : chat.vmStatus === 'starting' ? 'Starting' : chat.vmStatus === 'stopping' ? 'Shutting down'
    : chat.vmStatus === 'off' ? 'Off' : chat.vmStatus === 'failed' ? 'Failed'
    : chat.vmStatus === 'missing' ? 'Not set up' : 'Unknown');
  // Colors the state dot: working, in transition, failed or idle.
  const dot = $derived(chat.vmStatus === 'running' && chat.vmReady ? 'bg-success'
    : ['starting', 'stopping', 'running'].includes(chat.vmStatus) ? 'bg-amber-500'
    : chat.vmStatus === 'failed' ? 'bg-destructive' : 'bg-muted-foreground/40');

  // Refreshes native state without starting the selected VM.
  onMount(() => {
    const timer = setInterval(() => void chat.refreshVmStatus().catch(() => {}), 3000);
    return () => clearInterval(timer);
  });

  // Paints one frame at once. A frame that arrives while another is decoding is dropped;
  // the next one is newer anyway.
  async function show(frame: Uint8Array) {
    if (painting || !screen) return;
    painting = true;
    try {
      const picture = await createImageBitmap(new Blob([frame as BlobPart], { type: 'image/jpeg' }));
      if (screen.width !== picture.width) screen.width = picture.width;
      if (screen.height !== picture.height) screen.height = picture.height;
      screen.getContext('2d')?.drawImage(picture, 0, 0);
      sized = picture.width > 1;
      picture.close();
      noPicture = false;
    } catch {
      noPicture = true;
    } finally { painting = false; }
  }

  // Keeps a stream open while the VM is ready. A stream that ends or fails is reopened, so
  // the picture cannot freeze on its last frame; the endpoint never boots a stopped VM.
  $effect(() => {
    if (!chat.vmReady) return;
    const target = chat.vmTarget;
    const stop = new AbortController();
    void (async () => {
      for (let attempt = 0; !stop.signal.aborted; attempt++) {
        try {
          await readFrames(`/api/vms/${target}/desktop?stream=${attempt}`, stop.signal, show);
        } catch {
          if (!stop.signal.aborted) noPicture = true;
        }
        await new Promise(resolve => setTimeout(resolve, 2000));
      }
    })();
    return () => {
      stop.abort();
      sized = false;
    };
  });

  // Sends one VM-specific host action through MiniClaw's mutation guard.
  async function action(kind: 'show' | 'shutdown') {
    if (waiting) return;
    waiting = true;
    error = '';
    try {
      const response = await fetch(`/api/vms/${chat.vmTarget}/desktop/${kind}`, {
        method: 'POST', body: '{}',
        headers: { 'X-MiniClaw-Request': '1', 'Content-Type': 'application/json' },
      });
      if (!response.ok) {
        const result = await response.json().catch(() => null);
        throw new Error(result?.error?.message ?? 'Desktop action failed.');
      }
      if (kind === 'shutdown') await chat.refreshVmStatus();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Desktop action failed.';
    } finally {
      waiting = false;
    }
  }
</script>

<aside class={embedded ? 'flex min-h-0 w-full flex-1 flex-col gap-2 overflow-y-auto bg-muted p-3'
  : 'flex w-full shrink-0 flex-col gap-2 border-b border-border bg-muted p-3'}
  aria-label="Guest desktop">
  <div class="flex items-center justify-between gap-2">
    <div class="flex min-w-0 items-center gap-2">
      <span class={`size-2 shrink-0 rounded-full ${dot}`} aria-hidden="true"></span>
      <p class="truncate text-sm font-medium">
        {chat.vmName}{chat.vmTarget === 'master' ? '' : ` · ${chat.vmTarget.slice(0, 8)}`}
      </p>
      <p class="shrink-0 text-xs text-muted-foreground">{label}</p>
    </div>
    <div class="flex items-center">
      <button class="icon-button hover:bg-border/60 lg:hidden" aria-expanded={!folded}
        title={folded ? 'Show screen' : 'Hide screen'} aria-label={folded ? 'Show screen' : 'Hide screen'}
        disabled={!chat.vmReady} onclick={() => { folded = !folded; }}>
        <ChevronDown size={16} class={folded ? '' : 'rotate-180'} aria-hidden="true" />
      </button>
      <Transfers {chat} vmId={chat.vmTarget} disabled={!chat.vmReady} />
      <button class="icon-button hover:bg-border/60" title="Open desktop window" aria-label="Open desktop window"
        disabled={!chat.vmReady || waiting} onclick={() => void action('show')}>
        <ExternalLink size={15} aria-hidden="true" />
      </button>
      <button class="icon-button hover:bg-border/60" title={`Shut down ${chat.vmName}`}
        aria-label={`Shut down ${chat.vmName}`}
        disabled={chat.vmStatus !== 'running' || waiting}
        onclick={() => void action('shutdown')}>
        <Power size={15} aria-hidden="true" />
      </button>
    </div>
  </div>
  <!-- A small screen keeps its space for the chat while the VM has nothing to show. -->
  <!-- The frame is dark only when it stands for a live screen. -->
  <!-- Stacked above the chat, the screen keeps to a third of the window's height. -->
  <!-- With a picture the frame takes the picture's own shape, so no bars appear beside it.
       Beside the chat it fills the column's width; stacked above it, a share of the height. -->
  <div class={`relative overflow-hidden rounded-lg border ${chat.vmReady
    ? `border-black/10 bg-[#16181a] shadow-sm ${folded ? 'max-lg:hidden' : ''} ${noPicture || !sized
      ? 'aspect-[16/10] w-full max-lg:aspect-auto max-lg:h-[22dvh]' : 'mx-auto w-fit max-w-full'}`
    : 'aspect-[16/10] w-full border-dashed border-border bg-background max-lg:hidden'}`}>
    {#if chat.vmReady}
      <div role="img" aria-label={`Live ${chat.vmName} desktop`} class:invisible={noPicture}
        class={noPicture || !sized ? 'absolute inset-0' : 'block max-w-full'}>
        <canvas bind:this={screen} aria-hidden="true" width="1" height="1"
          class={`pointer-events-none block ${noPicture || !sized ? 'h-full w-full'
            : 'h-auto max-h-[28dvh] w-auto max-w-full max-sm:max-h-[22dvh] lg:max-h-[62dvh] lg:w-full'}`}></canvas>
      </div>
      {#if noPicture}
        <div role="status" class="absolute inset-0 flex flex-col items-center justify-center gap-2 px-5 text-center">
          <MonitorOff size={20} class="text-[#fff]/50" aria-hidden="true" />
          <p class="text-xs font-medium text-[#fff]/90">No picture from this VM yet</p>
          <p class="text-[11px] leading-4 text-[#fff]/60 max-lg:hidden">The VM works without it. To see its screen, open
            its window, turn on Control this VM, and allow Screen Recording for MiniClaw in the
            VM's System Settings.</p>
          <button class="mt-1 rounded-md bg-[#fff]/10 px-2.5 py-1 text-[11px] font-medium text-[#fff] hover:bg-[#fff]/20"
            disabled={waiting} onclick={() => void action('show')}>Open VM window</button>
        </div>
      {/if}
    {:else}
      <div class="absolute inset-0 flex flex-col items-center justify-center gap-2 px-4 text-center text-xs text-muted-foreground">
        {#if chat.vmStatus === 'starting' || chat.vmStatus === 'running'}
          <LoaderCircle size={18} class="animate-spin" aria-hidden="true" />
        {:else}
          <MonitorOff size={20} class="opacity-60" aria-hidden="true" />
        {/if}
        <p>
          {chat.vmStatus === 'off' ? 'No desktop signal while this VM is off' :
            chat.vmStatus === 'starting' ? 'Waiting for the guest desktop…' :
            chat.vmStatus === 'running' ? 'Log in to the VM in its window to continue' :
            'Desktop unavailable'}
        </p>
      </div>
    {/if}
  </div>
  {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
</aside>
