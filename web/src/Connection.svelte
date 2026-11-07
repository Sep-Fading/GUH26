<script lang="ts">
  // Starts health checks and registers cleanup.
  import { onMount } from 'svelte';
  // Records which operating system the backend runs on.
  import { setHost } from './platform.svelte';
  // Provides an accessible tooltip for retry.
  import { Tooltip } from 'bits-ui';
  // Shows refresh, success, and failure states.
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';

  type Status = 'checking' | 'connected' | 'offline' | 'waiting';
  let status = $state<Status>('checking');
  let request: AbortController | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;

  // Checks the Rust health endpoint and updates the badge, replacing any earlier check.
  // Reports backend reachability, not Codex sign-in or model availability.
  async function check() {
    request?.abort();
    clearTimeout(retry);
    const controller = new AbortController();
    request = controller;
    status = 'checking';
    const timer = setTimeout(() => controller.abort(), 8000);

    try {
      const response = await fetch('/api/health', {
        cache: 'no-store',
        signal: controller.signal,
      });
      if (!response.ok) throw new Error('Health request failed');

      const body: unknown = await response.json();
      // This badge checks the Rust endpoint, not Codex authorization.
      if (
        typeof body !== 'object' || body === null ||
        !('backend' in body) || body.backend !== 'ok'
      ) {
        throw new Error('Invalid health response');
      }

      // An older health check must not overwrite the latest result.
      if (request === controller) {
        if ('platform' in body) setHost(body.platform);
        status = 'startup' in body && body.startup === 'waiting_for_worker'
          ? 'waiting' : 'connected';
      }
    } catch {
      if (request === controller) status = 'offline';
    } finally {
      clearTimeout(timer);
      if (request === controller && (status === 'waiting' || status === 'offline')) {
        retry = setTimeout(() => { void check(); }, 2000);
      }
    }
  }

  // Starts health checks and cancels requests and retries when removed.
  onMount(() => {
    void check();
    return () => {
      request?.abort();
      request = undefined;
      clearTimeout(retry);
    };
  });
</script>

<div class="flex flex-col items-end gap-1">
  <div class="flex min-h-9 items-center gap-3">
    <span role="status" class="flex items-center gap-2 text-xs">
      {#if status === 'connected'}
        <CircleCheck size={15} class="text-success" aria-hidden="true" />
        <span class="max-md:sr-only">Backend connected</span>
      {:else if status === 'waiting'}
        <RefreshCw size={14} aria-hidden="true" />
        <span>Waiting for Master VM</span>
      {:else if status === 'offline'}
        <CircleAlert size={15} class="text-destructive" aria-hidden="true" />
        <span>Disconnected</span>
      {:else}
        <RefreshCw size={14} aria-hidden="true" />
        <span>Checking</span>
      {/if}
    </span>

    <Tooltip.Root>
      <Tooltip.Trigger
        class="icon-button max-sm:hidden"
        aria-label="Check connection"
        disabled={status === 'checking'}
        onclick={check}
      >
        <RefreshCw size={16} aria-hidden="true" />
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content role="tooltip" class="tooltip" sideOffset={8}>
          Check connection
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  </div>
  {#if status === 'offline'}
    <p role="alert" class="text-xs text-destructive">Backend unavailable</p>
  {/if}
</div>
