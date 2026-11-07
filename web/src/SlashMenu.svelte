<script lang="ts">
  // Lists the commands when the composer first needs them.
  import type { Chat } from './chat.svelte';

  type Command = { name: string; description: string; builtin: boolean };

  // `text` is the composer's draft; choosing a command replaces it with `/name `.
  let { chat, text = $bindable('') }: { chat: Chat; text?: string } = $props();
  let commands = $state<Command[]>([]);
  let loaded = false, dismissed = $state(''), index = $state(0);

  // The menu is for a command being typed: a slash, then a name with nothing after it yet.
  const typed = $derived(/^\/[a-z0-9_-]*$/.test(text) ? text.slice(1) : null);
  const matches = $derived(typed === null || dismissed === text ? []
    : commands.filter(command => command.name.startsWith(typed)));
  // What the message box tells a screen reader while the menu is open: the list and its active option.
  export const controls = () => matches.length ? 'slash-commands' : undefined;
  export const activeOption = () => matches.length ? `slash-${matches[index]?.name}` : undefined;

  $effect(() => {
    if (typed === null || loaded) return;
    loaded = true;
    chat.slashCommands<{ commands: Command[] }>().then(result => { commands = result.commands; },
      () => { loaded = false; });
  });
  $effect(() => { if (index >= matches.length) index = 0; });

  function choose(command: Command) { text = `/${command.name} `; }

  // Takes the keys that belong to an open menu; answers whether it used the key.
  export function key(event: KeyboardEvent) {
    if (!matches.length) return false;
    if (event.key === 'ArrowDown') index = (index + 1) % matches.length;
    else if (event.key === 'ArrowUp') index = (index - 1 + matches.length) % matches.length;
    else if ((event.key === 'Enter' && !event.shiftKey && !event.isComposing) || event.key === 'Tab') choose(matches[index]);
    else if (event.key === 'Escape') dismissed = text;
    else return false;
    event.preventDefault();
    return true;
  }
</script>

{#if matches.length}
  <ul role="listbox" id="slash-commands" aria-label="Slash commands"
    class="absolute bottom-full left-0 z-20 mb-2 max-h-60 w-[min(26rem,100%)] overflow-y-auto rounded-xl border border-border
      bg-background p-1 shadow-lg">
    {#each matches as command, position (command.name)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li role="option" id={`slash-${command.name}`} aria-selected={position === index}
        class={`flex cursor-pointer flex-col rounded-lg px-3 py-2 text-sm ${position === index ? 'bg-muted' : ''}`}
        onmouseenter={() => { index = position; }} onmousedown={event => { event.preventDefault(); choose(command); }}>
        <span class="font-mono font-medium">/{command.name}</span>
        {#if command.description}<span class="text-xs text-muted-foreground">{command.description}</span>{/if}
      </li>
    {/each}
  </ul>
{/if}
