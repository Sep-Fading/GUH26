<script lang="ts">
  // Styled controls from the shared component library.
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  // Reads and drives the running sign-in.
  import type { Chat } from './chat.svelte';

  let { chat }: { chat: Chat } = $props();
  let code = $state('');
  let sending = $state(false);

  // Hands the pasted code to the waiting sign-in; the box keeps a code MiniClaw refused.
  async function finish(event: SubmitEvent) {
    event.preventDefault();
    if (!code.trim() || sending) return;
    sending = true;
    if (await chat.sendClaudeCode(code)) code = '';
    sending = false;
  }
</script>

{#if chat.login.provider === 'claude' && chat.login.status === 'pending'}
  <div class="space-y-3 text-left text-sm" role="group" aria-label="Claude sign-in">
    {#if chat.login.authUrl}
      <p>Claude Code is opening its sign-in page in your browser. Finish signing in there;
        MiniClaw notices by itself.</p>
      <p>Nothing opened?
        <a class="text-brand underline" href={chat.login.authUrl} target="_blank"
          rel="noopener noreferrer">Open the Claude sign-in page</a></p>
      <form class="space-y-2" onsubmit={finish}>
        <label class="block text-xs text-muted-foreground" for="claude-sign-in-code">
          If Claude shows a code after you sign in, paste it here.</label>
        <div class="flex gap-2">
          <Input id="claude-sign-in-code" aria-label="Code from Claude" autocomplete="off" spellcheck={false}
            bind:value={code} disabled={sending} />
          <Button type="submit" size="sm" disabled={sending || !code.trim()}>Finish sign-in</Button>
        </div>
      </form>
    {:else}
      <p aria-live="polite">Starting Claude Code's sign-in...</p>
    {/if}
    {#if chat.login.notice}<p role="status" class="text-xs text-muted-foreground">{chat.login.notice}</p>{/if}
    <Button variant="outline" size="sm" aria-label="Cancel Claude sign-in" onclick={() => chat.cancelLogin()}>Cancel</Button>
  </div>
{:else if chat.login.provider === 'claude' && chat.login.status === 'failed' && chat.login.error}
  <p role="alert" class="text-sm text-destructive">{chat.login.error}</p>
{/if}
