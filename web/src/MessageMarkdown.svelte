<script lang="ts">
  // Turns the reply into inert HTML; raw tags are escaped there.
  import { agentProposal, renderMessage, serverProposal, skillProposal } from './message-markdown';

  let { text }: { text: string } = $props();
  const html = $derived(renderMessage(text));

  // Handles the buttons a rendered reply can contain: Copy on code, loading one image, and adding a proposed MCP server.
  function act(event: MouseEvent) {
    const button = (event.target as HTMLElement).closest('button');
    if (!button) return;
    if (button.hasAttribute('data-copy')) {
      const code = button.closest('.md-code')?.querySelector('code')?.textContent ?? '';
      void navigator.clipboard.writeText(code).then(() => {
        button.textContent = 'Copied';
        setTimeout(() => { button.textContent = 'Copy'; }, 1500);
      }, () => { button.textContent = 'Copy failed'; });
    }
    // A proposed MCP server: checked again, then handed to the app, which opens the server form.
    const proposal = button.hasAttribute('data-mcp-add') ? serverProposal(button.getAttribute('data-mcp-add') ?? '') : null;
    if (proposal) window.dispatchEvent(new CustomEvent('miniclaw:add-mcp-server', { detail: proposal }));
    // A proposed skill or agent travels the same way: checked again, then the form opens filled in.
    const skill = button.hasAttribute('data-skill-add') ? skillProposal(button.getAttribute('data-skill-add') ?? '') : null;
    if (skill) window.dispatchEvent(new CustomEvent('miniclaw:add-skill', { detail: skill }));
    const agent = button.hasAttribute('data-agent-add') ? agentProposal(button.getAttribute('data-agent-add') ?? '') : null;
    if (agent) window.dispatchEvent(new CustomEvent('miniclaw:add-agent', { detail: agent }));
    const address = button.getAttribute('data-image');
    if (address?.startsWith('https://')) {
      const image = document.createElement('img');
      image.src = address;
      image.alt = button.getAttribute('data-alt') ?? '';
      image.referrerPolicy = 'no-referrer';
      image.loading = 'lazy';
      image.className = 'md-loaded-image';
      button.replaceWith(image);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="markdown" onclick={act}>{@html html}</div>
