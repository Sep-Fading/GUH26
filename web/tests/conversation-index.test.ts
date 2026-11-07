// Checks exchange grouping and safe preview rendering.
import { expect, test } from 'vitest';
// Exercises previews independently of scroll and layout.
import { exchanges, previewMarkdown } from '../src/conversation-index';

test('each prompt owns only its following responses', () => {
  expect(exchanges([
    { id: 'old', role: 'assistant', text: 'Unloaded prompt' },
    { id: 'one', role: 'user', text: 'First question' },
    { id: 'a', role: 'assistant', text: 'First answer' },
    { id: 'b', role: 'assistant', text: 'More detail' },
    { id: 'two', role: 'user', text: 'Second question' },
    { id: 'three', role: 'user', text: 'Third question' },
  ])).toEqual([
    { id: 'one', prompt: 'First question', response: 'First answer\n\nMore detail' },
    { id: 'two', prompt: 'Second question', response: '' },
    { id: 'three', prompt: 'Third question', response: '' },
  ]);
});

test('excerpts stay bounded without changing message source', () => {
  const source = [
    { id: 'one', role: 'user' as const, text: 'word '.repeat(100) },
    { id: 'a', role: 'assistant' as const, text: 'x'.repeat(2000) },
  ];
  const result = exchanges(source)[0];
  expect(result.prompt.length).toBeLessThanOrEqual(81);
  expect(result.prompt.split(' ').length).toBeLessThanOrEqual(8);
  expect(result.response.length).toBeLessThanOrEqual(1200);
  expect(source[1].text.length).toBe(2000);
  expect(exchanges([])).toEqual([]);
});

test('previews render emphasis, lists and code', () => {
  const html = previewMarkdown('*Useful* **answer**\n\n- Item\n\n`exact_key`\n\n```ts\nconst n = 1;\n```');
  expect(html).toContain('<em>Useful</em>');
  expect(html).toContain('<strong>answer</strong>');
  expect(html).toContain('<li>Item</li>');
  expect(html).toContain('<code>exact_key</code>');
  expect(html).toContain('<pre><code');
});

test('previews cannot execute HTML, load images or navigate links', () => {
  const html = previewMarkdown('<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n'
    + '![<unsafe>](https://example.test/image.png) [Link](https://example.test) '
    + '[Bad](javascript:alert(1))');
  const container = document.createElement('div');
  container.innerHTML = html;
  expect(container.querySelector('script, img, a, iframe, [onerror]')).toBeNull();
  expect(container.textContent).toContain('<script>alert(1)</script>');
  expect(container.textContent).toContain('<unsafe>');
  expect(container.textContent).toContain('Link');
});
