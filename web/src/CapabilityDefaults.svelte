<script lang="ts">
  // Reuses the accessible on and off control.
  import { Switch } from '$lib/components/ui/switch/index.js';
  // Shares API requests with the conversation.
  import type { Chat } from './chat.svelte';

  let { chat, id, active }: { chat: Chat; id: string; active: boolean } = $props();
  type Capability = { key: string; name: string; allowed: boolean; on: boolean };
  let rows = $state<Capability[]>([]);
  let loading = $state(true), saving = $state(false), error = $state(''), saved = $state(false);
  let request = 0;

  // Loads this agent's saved defaults and ignores replies after leaving its page.
  $effect(() => {
    if (!active) return;
    const sequence = ++request;
    loading = true;
    chat.capabilityDefaults<{ capabilities: Capability[] }>(id).then(result => {
      if (sequence === request) { rows = result.capabilities; error = ''; }
    }).catch(cause => {
      if (sequence === request) error = cause instanceof Error ? cause.message : 'Could not load defaults.';
    }).finally(() => { if (sequence === request) loading = false; });
    return () => { request++; };
  });

  // Saves one preference; an error keeps the last acknowledged state and offers another try.
  async function change(row: Capability, on: boolean) {
    if (saving) return;
    const sequence = request, previous = row.on;
    row.on = on;
    saving = true; error = ''; saved = false;
    try {
      await chat.capabilityDefaults(id, { key: row.key, on });
      if (sequence === request) { row.on = on; saved = true; }
    } catch (cause) {
      if (sequence === request) {
        row.on = previous;
        error = cause instanceof Error ? cause.message : 'The default was not saved.';
      }
    } finally { if (sequence === request) saving = false; }
  }
</script>

<section aria-label="Default capabilities" class="space-y-3 text-sm">
  <p class="text-xs text-muted-foreground">Start new conversations with these capabilities on. Existing conversations keep their settings. You can change each conversation from Tools.</p>
  <p class="text-xs text-muted-foreground">Allow a capability in Permissions first. Provider-specific tools are used only with a compatible provider.</p>
  {#if loading}<p role="status" class="text-xs text-muted-foreground">Loading defaults...</p>{/if}
  {#each rows as row (row.key)}
    <div class="flex items-center justify-between gap-4 border-t border-border py-2">
      <div class="min-w-0">
        <p class="break-words">{row.name}</p>
        <p class="text-xs text-muted-foreground">{!row.allowed ? 'Allow in Permissions first' : row.on ? 'On in new conversations' : 'Off in new conversations'}</p>
      </div>
      <Switch aria-label={row.name} checked={row.on} disabled={loading || saving || !row.allowed}
        onCheckedChange={on => change(row, on)} />
    </div>
  {/each}
  {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  <p role="status" class="text-xs text-success">{saved ? 'Saved' : ''}</p>
</section>
