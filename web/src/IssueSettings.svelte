<script lang="ts">
  // Reads and saves the project's review level through the backend.
  import type { Projects, ReviewLevel } from './project-state.svelte';

  let { projects, id }: { projects: Projects; id: string } = $props();
  let level = $state<ReviewLevel>('low'), loaded = $state(false), busy = $state(false), error = $state('');
  const choices: { value: ReviewLevel; label: string; description: string }[] = [
    { value: 'none', label: 'Nothing', description: 'Every issue waits for you.' },
    { value: 'low', label: 'Low (default)', description: 'Wording, cosmetics, small fixes. Medium and High wait for you.' },
    { value: 'medium', label: 'Low and Medium', description: 'Only High waits for you.' },
    { value: 'high', label: 'Everything', description: 'Agents close every issue by themselves.' },
  ];

  // Saves a new level at once; a failure restores the previous one and says why.
  // A choice made while a save runs is put back, so the radios never need disabling.
  async function choose(next: ReviewLevel, input: HTMLInputElement) {
    if (busy) { input.checked = false; return; }
    const previous = level;
    level = next; busy = true; error = '';
    try { await projects.saveReviewLevel(id, next); }
    catch (cause) { level = previous; error = cause instanceof Error ? cause.message : 'The review level could not be saved.'; }
    finally { busy = false; }
  }

  // Loads the saved level whenever the section shows another project.
  $effect(() => {
    const project = id;
    loaded = false; error = '';
    projects.reviewLevel(project).then(
      found => { if (project === id) { level = found; loaded = true; } },
      cause => { if (project === id) error = cause instanceof Error ? cause.message : 'The review level could not be read.'; });
  });
</script>

<section aria-label="Issues" class="space-y-2 border-t border-border pt-4 text-sm">
  <h3 class="font-medium">Issues</h3>
  <p class="text-xs text-muted-foreground">The bugs and features agents track in this project. The agent that owns an issue decides when it is
    done. Up to the level below, that closes it. Above it, the issue waits in the Issues tab until you close it.</p>
  <fieldset class="group space-y-1.5" disabled={!loaded} aria-busy={busy}>
    <legend class="mb-1.5 text-xs font-medium">Agents may close without review</legend>
    {#each choices as choice (choice.value)}
      <label class={['flex cursor-pointer items-start gap-2.5 rounded-md border px-3 py-2 transition-colors',
        'group-disabled:cursor-default group-disabled:opacity-60 group-disabled:hover:bg-transparent',
        level === choice.value ? 'border-ring bg-muted' : 'border-border hover:bg-muted']}>
        <input type="radio" name={`review-level-${id}`} value={choice.value} checked={level === choice.value}
          class="mt-0.5 accent-brand" onchange={event => choose(choice.value, event.currentTarget)} />
        <span class="flex flex-col gap-0.5">
          <span class="font-medium">{choice.label}</span>
          <span class="text-xs text-muted-foreground">{choice.description}</span>
        </span>
      </label>
    {/each}
  </fieldset>
  {#if error}<p role="alert" class="text-xs text-destructive">{error}</p>{/if}
  <p class="text-xs text-muted-foreground">This only decides who closes an issue. File changes and commands still ask for approval as usual
    unless the conversation has Full access.</p>
</section>
