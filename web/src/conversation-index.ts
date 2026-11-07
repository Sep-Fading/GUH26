// Parses bounded previews without enabling raw HTML.
import MarkdownIt from 'markdown-it';
// Reuses the conversation's existing message shape.
import type { Chat } from './chat.svelte';

export type Exchange = { id: string; prompt: string; response: string };
type Message = Pick<Chat['messages'][number], 'id' | 'role' | 'text'>;
const markdown = new MarkdownIt({ html: false, linkify: false, typographer: false });

// Keeps image descriptions as escaped text without loading their sources.
markdown.renderer.rules.image = (tokens, index) => markdown.utils.escapeHtml(tokens[index].content);
// Keeps link labels readable without adding navigation inside a tooltip.
markdown.renderer.rules.link_open = markdown.renderer.rules.link_close = () => '';

// Returns one bounded prompt/response excerpt per loaded user message.
export function exchanges(messages: readonly Message[]): Exchange[] {
  const result: Exchange[] = [];
  for (const message of messages) {
    if (message.role === 'user') {
      const words = message.text.trim().split(/\s+/, 8).join(' ');
      const prompt = words.slice(0, 80);
      result.push({ id: message.id, prompt: prompt || 'Untitled prompt', response: '' });
    } else {
      const current = result.at(-1);
      if (current && current.response.length < 1200) {
        current.response = (current.response + (current.response ? '\n\n' : '')
          + message.text.slice(0, 1200)).slice(0, 1200);
      }
    }
  }
  return result;
}

// Returns inert Markdown HTML for the visible response preview only.
export function previewMarkdown(source: string): string {
  return markdown.render(source.slice(0, 1200));
}
