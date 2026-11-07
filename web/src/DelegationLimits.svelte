<script lang="ts">
  // Words the limit in hours, as the delegation prompt does.
  import { duration } from './delegation-words';

  type Limits = { maxPerTurn: number; minutes: number };
  // The same bounds the backend checks (`agents.rs` `Delegation`).
  const MOST_MINUTES = 1440, MOST_WAIT_MINUTES = 60;
  let { value, disabled = false, onchange }: { value: Limits; disabled?: boolean; onchange: (next: Limits) => void } = $props();
  // What is typed, kept apart from the saved value so a half-typed number is not sent.
  let typed = $state(''), error = $state('');
  $effect(() => { typed = String(value.minutes); error = ''; });
  const shown = $derived(/^\d+$/.test(typed) && Number(typed) >= 1 && Number(typed) <= MOST_MINUTES ? Number(typed) : value.minutes);

  // Sends a whole number of minutes within bounds; anything else stays here with a message.
  function commit() {
    const minutes = Number(typed);
    if (!/^\d+$/.test(typed.trim()) || minutes < 1 || minutes > MOST_MINUTES) {
      error = `Enter a whole number of minutes from 1 to ${MOST_MINUTES}.`;
      return;
    }
    error = '';
    if (minutes !== value.minutes) onchange({ ...value, minutes });
  }
</script>

<div class="space-y-1 pt-1 text-xs">
  <div class="flex flex-wrap items-center gap-4">
    <label class="flex items-center gap-2">Delegations per message
      <select aria-label="Delegations per message" class="rounded-md border border-border bg-background px-2 py-1" {disabled}
        value={String(value.maxPerTurn)} onchange={event => onchange({ ...value, maxPerTurn: Number(event.currentTarget.value) })}>
        {#each Array.from({ length: 10 }, (_, index) => index + 1) as count (count)}<option value={String(count)}>{count}</option>{/each}
      </select></label>
    <label class="flex items-center gap-2">Time limit per run
      <input type="number" aria-label="Time limit per run" class="w-20 rounded-md border border-border bg-background px-2 py-1"
        min="1" max={MOST_MINUTES} step="1" inputmode="numeric" {disabled} aria-invalid={!!error}
        value={typed} oninput={event => { typed = event.currentTarget.value; }} onchange={commit} />
      min</label>
    {#if shown >= 60}<span class="text-muted-foreground">{duration(shown)}</span>{/if}
  </div>
  {#if error}<p role="alert" class="text-destructive">{error}</p>{/if}
  {#if shown > MOST_WAIT_MINUTES}
    <p class="text-muted-foreground">If you choose Wait for it, the conversation waits for at most {duration(MOST_WAIT_MINUTES)}; then the run
      continues in the background, up to the full limit.</p>
  {/if}
</div>
