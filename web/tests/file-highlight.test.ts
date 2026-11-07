// Checks file detection and multiline token continuity.
import { expect, test } from 'vitest';
// Shares filename detection with the diff viewer.
import { languageOf } from '../src/diff';
// Produces safe numbered lines for file previews.
import { highlightLines } from '../src/message-markdown';

test('code filenames select their language automatically', () => {
  for (const [path, language] of Object.entries({ 'game.server.luau': 'lua', 'scene.lua': 'lua',
    'MAIN.CPP': 'cpp', 'main.c': 'c', 'Player.cs': 'csharp', 'App.java': 'java',
    'Gemfile': 'ruby', 'Dockerfile': 'dockerfile', 'Makefile': 'makefile',
    'Cargo.toml': 'ini', 'README.md': 'markdown', 'app.cjs': 'javascript' })) {
    expect(languageOf(path)).toBe(language);
  }
  expect(languageOf('notes.txt')).toBeUndefined();
});

test('file lines retain multiline syntax and inert source text', () => {
  const source = '--[[\n<script>alert(1)</script>\n]]\nlocal count = 3';
  const lines = highlightLines(source, 'lua');
  expect(lines).toHaveLength(4);
  const nodes = lines.map(html => {
    const node = document.createElement('div'); node.innerHTML = html; return node;
  });
  expect(nodes.map(n => n.textContent).join('\n')).toBe(source);
  expect(nodes[1].querySelector('.hljs-comment')?.textContent).toBe('<script>alert(1)</script>');
  expect(nodes[1].querySelector('script')).toBeNull();
  expect(nodes[3].querySelector('.hljs-keyword')?.textContent).toBe('local');
  expect(highlightLines('<img src=x>\nplain', undefined)).toEqual(['&lt;img src=x&gt;', 'plain']);
});
