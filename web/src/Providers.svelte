<script lang="ts">
  // Refreshes provider state when the panel is shown.
  import { onMount } from 'svelte';
  // Keeps dialog focus and dismissal accessible.
  // Styled switch and button from the shared component library.
  import { Switch } from '$lib/components/ui/switch/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  // Identifies provider settings and its commands.
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  // Shows a running Claude sign-in under its provider.
  import ClaudeSignIn from './ClaudeSignIn.svelte';
  // Reads provider state from the chat.
  import type { Chat, Provider } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
  const names = { codex: 'Codex (ChatGPT)', claude: 'Claude Code' };

  // Describes one provider's state in a short phrase.
  function describe(provider: Provider) {
    if (!provider.installed) return 'Not installed';
    if (!provider.enabled) return 'Disabled';
    if (provider.status === 'signed_in') {
      return [provider.email, provider.planType].filter(Boolean).join(' · ') || 'Signed in';
    }
    if (provider.status === 'signed_out') return 'Signed out';
    if (provider.status === 'unsupported_auth') return 'Subscription sign-in required';
    return 'Unavailable';
  }

  onMount(() => { void chat.refresh(); });
</script>

<section aria-label="Providers" class="flex min-h-0 flex-1 flex-col overflow-y-auto">
  <!-- The right padding keeps the button clear of the sheet's own close button. -->
  <div class="sticky top-0 z-[5] flex h-12 shrink-0 items-center justify-between border-b border-border bg-background pl-5 pr-14">
    <h2 class="text-base font-semibold">Providers</h2>
    <button class="icon-button" aria-label="Refresh providers" title="Refresh providers and models"
      disabled={chat.refreshing || chat.modelsLoading} onclick={() => chat.refresh(true)}>
      <RefreshCw size={16} class={chat.refreshing ? 'animate-spin motion-reduce:animate-none' : ''} aria-hidden="true" />
    </button>
  </div>
  <ul class="divide-y divide-line">
    {#each chat.providers as provider (provider.id)}
      <li class="flex items-center gap-3 px-5 py-4">
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium">{names[provider.id]}</p>
          <p class="truncate text-xs text-muted-foreground">{describe(provider)}</p>
          {#if provider.enabled && provider.status === 'signed_in'}
            {@const count = chat.models.filter(model => (model.provider ?? 'codex') === provider.id).length}
            <p class="mt-1 text-xs text-muted-foreground">
              {count} {count === 1 ? 'model' : 'models'} available
            </p>
          {/if}
        </div>
        {#if provider.enabled && provider.status === 'signed_out'}
          <Button variant="outline" size="sm"
            aria-label={`Sign in to ${names[provider.id]}`}
            disabled={chat.login.status === 'pending'}
            onclick={() => provider.id === 'claude' ? chat.signInClaude() : chat.signIn()}>
            Sign in
          </Button>
        {/if}
        <Switch class="data-[state=checked]:bg-success" aria-label={`Enable ${names[provider.id]}`}
          checked={provider.enabled} disabled={!provider.installed}
          onCheckedChange={enabled => chat.setProvider(provider.id, enabled)} />
      </li>
      {#if provider.id === 'claude' && chat.login.provider === 'claude'
        && (chat.login.status === 'pending' || chat.login.status === 'failed')}
        <li class="px-5 pb-4"><ClaudeSignIn {chat} /></li>
      {/if}
    {:else}
      <li class="px-5 py-4 text-sm text-muted-foreground">No provider is running.</li>
    {/each}
  </ul>
  {#if chat.modelError}
    <p role="alert" class="px-5 py-3 text-sm text-destructive">
      {chat.modelError}. Refresh to try again.
    </p>
  {/if}
  <p class="border-t border-border px-5 py-3 text-xs text-muted-foreground">
    Each provider uses its own sign-in. A chat stays with the provider of the model
    it was started with. Refresh here to check for new models.
  </p>
</section>
