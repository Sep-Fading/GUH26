<script lang="ts">
  // Marks the user-triggered quota refresh control.
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Shares provider readings with the composer.
  import type { Chat } from './chat.svelte';
  // Names windows and formats their reset times.
  import { usageRows, resetTime } from './usage';

  let { chat }: { chat: Chat } = $props();
  const providers = $derived((chat.account.providers ?? []).filter(provider => provider.installed));
</script>

<section aria-label="Provider usage limits" class="border-b border-border p-5">
  <div class="flex items-center justify-between gap-3">
    <h3 class="text-sm font-semibold">Usage limits</h3>
    <button class="icon-button" aria-label="Refresh usage limits" disabled={chat.usageLoading}
      onclick={() => chat.refreshUsage(true)}>
      <RefreshCw size={15} aria-hidden="true" />
    </button>
  </div>
  <p class="mt-0.5 text-xs text-muted-foreground">Current account limits, shared with usage outside MiniClaw.</p>
  <div class="mt-4 grid min-w-0 grid-cols-1 gap-6 lg:grid-cols-2">
    {#each providers as provider (provider.id)}
      {@const reading = chat.usageFor(provider.id)}
      <section aria-label={`${provider.id === 'claude' ? 'Claude' : 'Codex'} account limits`} class="min-w-0">
        <div class="mb-3 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
          <h4 class="text-sm font-medium">{provider.id === 'claude' ? 'Claude Code' : 'Codex'}</h4>
          {#if reading.status === 'stale'}<span class="text-xs text-muted-foreground">Out of date</span>
          {:else if reading.fetchedAt}<span class="text-xs text-muted-foreground" title={new Date(reading.fetchedAt).toLocaleString()}>
            Updated {new Date(reading.fetchedAt).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}
          </span>{/if}
        </div>
        {#if !provider.enabled}
          <p class="text-sm text-muted-foreground">Provider disabled</p>
        {:else if provider.status !== 'signed_in'}
          <p class="text-sm text-muted-foreground">{provider.status === 'signed_out' ? 'Sign in from Providers to view limits.' : 'Provider unavailable'}</p>
        {:else if !usageRows(reading).length}
          <p class="text-sm text-muted-foreground">{chat.usageLoading ? 'Reading usage limits...' : 'Usage unavailable'}</p>
        {:else}
          <ul class="space-y-4">
            {#each usageRows(reading) as window (window.id)}
              <li class="min-w-0">
                <div class="flex items-baseline justify-between gap-3 text-sm">
                  <span class="min-w-0 break-words">{window.label}</span>
                  <span class="shrink-0 tabular-nums" class:text-destructive={(window.usedPercent ?? 0) >= 90}>
                    {window.usedPercent === null ? 'Unknown' : `${window.usedPercent}% used`}
                  </span>
                </div>
                <div class="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
                  {#if window.usedPercent !== null}<div class="h-full rounded-full"
                    class:bg-destructive={window.usedPercent >= 90} class:bg-chart-1={window.usedPercent < 90}
                    style={`width: ${Math.min(100, Math.max(0, window.usedPercent))}%`}></div>{/if}
                </div>
                <p class="mt-1 text-xs text-muted-foreground">{resetTime(window.resetsAt)}</p>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}
  </div>
</section>
