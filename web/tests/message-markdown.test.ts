// Checks that assistant Markdown renders richly and stays inert.
import { describe, expect, test } from 'vitest';
import { renderMessage } from '../src/message-markdown';

// Parses rendered HTML the way the page will, so assertions are about elements, not strings.
function dom(source: string) {
  const root = document.createElement('div');
  root.innerHTML = renderMessage(source);
  return root;
}

describe('assistant message Markdown', () => {
  test('renders formatting, lists and tables', () => {
    const root = dom('# Title\n\n**bold** and *soft*\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted');
    expect(root.querySelector('h1')?.textContent).toBe('Title');
    expect(root.querySelector('strong')?.textContent).toBe('bold');
    expect(root.querySelectorAll('li')).toHaveLength(2);
    expect(root.querySelectorAll('td')).toHaveLength(2);
    expect(root.querySelector('blockquote')).not.toBeNull();
  });

  test('single line breaks are kept, as in the plain-text view', () => {
    expect(dom('one\ntwo\nthree').querySelectorAll('br')).toHaveLength(2);
  });

  test('raw HTML stays text and can never become an element', () => {
    const root = dom('<img src=x onerror="alert(1)"> <script>alert(2)</script> <iframe src="https://x.example"></iframe>');
    expect(root.querySelector('img, script, iframe, [onerror]')).toBeNull();
    expect(root.textContent).toContain('<script>alert(2)</script>');
  });

  test('only https links become links; they open apart and show their address', () => {
    const root = dom('[docs](https://docs.rs/serde) [bad](javascript:alert(1)) [plain](http://example.com) '
      + '[file](file:///etc/passwd) https://svelte.dev and [mail](mailto:a@example.com)');
    const links = [...root.querySelectorAll('a')];
    expect(links.map(link => link.getAttribute('href'))).toEqual(['https://docs.rs/serde', 'https://svelte.dev']);
    for (const link of links) {
      expect(link.getAttribute('target')).toBe('_blank');
      expect(link.getAttribute('rel')).toBe('noopener noreferrer nofollow');
      expect(link.getAttribute('title')).toBe(link.getAttribute('href'));
    }
    expect(root.textContent).toContain('bad');
  });

  test('a link whose text names another site shows where it really goes', () => {
    const link = dom('[https://bank.example](https://evil.example/login)').querySelector('a')!;
    expect(link.getAttribute('title')).toBe('https://evil.example/login');
  });

  test('an image is a button naming its host until the user loads it', () => {
    const root = dom('![chart](https://img.example/a.png?d=secret) ![x](http://plain.example/a.png) ![y](data:image/png;base64,AAAA)');
    expect(root.querySelector('img')).toBeNull();
    const buttons = [...root.querySelectorAll('button[data-image]')];
    expect(buttons).toHaveLength(1);
    expect(buttons[0].getAttribute('data-image')).toBe('https://img.example/a.png?d=secret');
    expect(buttons[0].textContent).toContain('img.example');
    expect(buttons[0].getAttribute('data-alt')).toBe('chart');
  });

  test('code blocks are highlighted, escaped and carry a Copy button', () => {
    const root = dom('```js\nconst a = "<b>";\n```\n\n```\nplain <i>text</i>\n```\n\n```nonsense-lang\nx < y\n```');
    const blocks = [...root.querySelectorAll('div.md-code')];
    expect(blocks).toHaveLength(3);
    expect(blocks[0].querySelector('button[data-copy]')?.textContent).toBe('Copy');
    expect(blocks[0].querySelector('.hljs-keyword')?.textContent).toBe('const');
    expect(blocks[0].querySelector('code')?.textContent).toBe('const a = "<b>";\n');
    expect(blocks[0].querySelector('.md-code-language')?.textContent).toBe('js');
    expect(root.querySelector('b, i')).toBeNull();
    expect(blocks[2].querySelector('code')?.textContent).toBe('x < y\n');
  });

  test('a language name cannot inject markup or classes', () => {
    const root = dom('```"><img src=x>\ncode\n```');
    expect(root.querySelector('img')).toBeNull();
  });

  test('a very long message falls back to escaped plain text', () => {
    const root = dom(`**x** ${'a'.repeat(300_000)}`);
    expect(root.querySelector('strong')).toBeNull();
    expect(root.querySelector('pre.md-plain')).not.toBeNull();
  });

  test('a miniclaw-mcp-server block becomes a card that carries only checked fields', () => {
    const proposal = { name: 'notes', command: '/opt/homebrew/bin/node', args: ['/Users/me/mcp/notes/build/index.js'],
      folder: '/Users/me/mcp/notes', hosts: ['api.example.com'], env: ['NOTES_API_KEY'], unrestricted: true, token: 'leak' };
    const root = dom('Done.\n\n```miniclaw-mcp-server\n' + JSON.stringify(proposal) + '\n```');
    const card = root.querySelector('[data-mcp-card]');
    expect(card?.getAttribute('aria-label')).toBe('MCP server notes');
    expect(card?.textContent).toContain('/opt/homebrew/bin/node /Users/me/mcp/notes/build/index.js');
    expect(card?.textContent).toContain('NOTES_API_KEY');
    const button = card?.querySelector('button[data-mcp-add]');
    expect(button?.textContent).toBe('Add this server to MiniClaw');
    // The card never carries a request to run unrestricted, nor anything else it does not know.
    expect(JSON.parse(button?.getAttribute('data-mcp-add') ?? '{}')).toEqual({ name: 'notes', command: '/opt/homebrew/bin/node',
      args: ['/Users/me/mcp/notes/build/index.js'], folder: '/Users/me/mcp/notes', hosts: ['api.example.com'], ports: [], listen: [], env: ['NOTES_API_KEY'] });
    // A server an app connects to names the port it listens on; a bad port spoils the block.
    const listening = dom('```miniclaw-mcp-server\n' + JSON.stringify({ name: 'studio', command: '/bin/x', listen: [13469], ports: [9876] }) + '\n```');
    expect(listening.querySelector('[data-mcp-card]')?.textContent).toContain('Listens on13469');
    expect(dom('```miniclaw-mcp-server\n' + JSON.stringify({ name: 'studio', command: '/bin/x', listen: [80] }) + '\n```').querySelector('[data-mcp-card]')).toBeNull();
  });

  test('a block that is not a valid server stays an ordinary code block', () => {
    for (const bad of ['not json', '{"name":"Bad Name","command":"/bin/x"}', '{"name":"x","command":"relative"}',
      '{"name":"x","command":"/bin/x","args":"rm"}', '{"name":"x","command":"/bin/x","env":["BAD NAME"]}',
      '{"name":"x","command":"/bin/x","hosts":["http://a"]}', '{"name":"<img src=x onerror=alert(1)>","command":"/bin/x"}']) {
      const root = dom('```miniclaw-mcp-server\n' + bad + '\n```');
      expect(root.querySelector('[data-mcp-card]'), bad).toBeNull();
      expect(root.querySelector('.md-code'), bad).not.toBeNull();
      expect(root.querySelector('img'), bad).toBeNull();
    }
  });
  test('a miniclaw-skill block becomes a card that carries only checked fields', () => {
    const proposal = { name: 'Pull request review', description: 'Reviews a pull request.', instructions: 'When asked...\n1. Read <b>it</b>.',
      slash: true, replaces: null, active: true, id: 'x' };
    const root = dom('Here it is.\n\n```miniclaw-skill\n' + JSON.stringify(proposal) + '\n```');
    const card = root.querySelector('[data-skill-card]');
    expect(card?.getAttribute('aria-label')).toBe('Skill Pull request review');
    expect(card?.textContent).toContain('Reviews a pull request.');
    expect(card?.querySelector('b')).toBeNull();
    const button = card?.querySelector('button[data-skill-add]');
    expect(button?.textContent).toBe('Review this skill');
    // It can never arrive switched on, and nothing unknown travels with it.
    expect(JSON.parse(button?.getAttribute('data-skill-add') ?? '{}')).toEqual({ name: 'Pull request review',
      description: 'Reviews a pull request.', instructions: 'When asked...\n1. Read <b>it</b>.', slash: true, replaces: null });
    const changed = dom('```miniclaw-skill\n' + JSON.stringify({ ...proposal, replaces: 'Old review' }) + '\n```');
    expect(changed.querySelector('[data-skill-card]')?.textContent).toContain('New version of Old review');
  });

  test('a miniclaw-agent block becomes a card that lists each suggested permission with its reason', () => {
    const proposal = { name: 'Reviewer', instructions: 'You review code.', model: 'gpt-5.6-luna', effort: 'low', skills: ['Pull request review'],
      permissions: [{ kind: 'folder', path: '/Users/me/site', action: 'read', reason: 'It reads the code.' },
        { kind: 'feature', name: 'network', reason: 'It installs packages.' }, { kind: 'host', host: 'registry.npmjs.org', reason: 'npm.' },
        { kind: 'mcp_server', name: 'notes', reason: 'Looks up notes.' }, { kind: 'application', id: 'com.apple.Notes', action: 'observe', reason: 'Reads notes.' }],
      replaces: null, active: true, delegatesTo: ['x'] };
    const root = dom('```miniclaw-agent\n' + JSON.stringify(proposal) + '\n```');
    const card = root.querySelector('[data-agent-card]');
    expect(card?.getAttribute('aria-label')).toBe('Agent Reviewer');
    for (const text of ['Read folder /Users/me/site', 'It reads the code.', 'Network for commands', 'registry.npmjs.org', 'MCP server notes', 'Observe com.apple.Notes'])
      expect(card?.textContent).toContain(text);
    expect(card?.textContent).toContain('You approve each one yourself after saving');
    const carried = JSON.parse(card?.querySelector('button[data-agent-add]')?.getAttribute('data-agent-add') ?? '{}');
    expect(Object.keys(carried).sort()).toEqual(['effort', 'instructions', 'model', 'name', 'permissions', 'replaces', 'skills']);
    expect(carried.permissions).toHaveLength(5);
  });

  test('a skill or agent block that is not valid stays an ordinary code block', () => {
    const cases: [string, unknown][] = [['miniclaw-skill', { name: '', instructions: 'x' }], ['miniclaw-skill', { name: 'x', instructions: 'y'.repeat(16001) }],
      ['miniclaw-skill', { name: 'two\nlines', instructions: 'x' }], ['miniclaw-agent', { name: 'x' }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: [{ kind: 'folder', path: 'relative', action: 'read', reason: 'r' }] }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: [{ kind: 'folder', path: '/a/../b', action: 'read', reason: 'r' }] }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: [{ kind: 'folder', path: '/a', action: 'write' }] }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: [{ kind: 'feature', name: 'coding_session', reason: 'r' }] }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: [{ kind: 'command', text: 'rm -rf /', reason: 'r' }] }],
      ['miniclaw-agent', { name: 'x', instructions: 'y', permissions: Array(13).fill({ kind: 'feature', name: 'network', reason: 'r' }) }]];
    for (const [kind, bad] of cases) {
      const root = dom('```' + kind + '\n' + JSON.stringify(bad) + '\n```');
      expect(root.querySelector('[data-skill-card], [data-agent-card]'), JSON.stringify(bad).slice(0, 80)).toBeNull();
      expect(root.querySelector('.md-code')).not.toBeNull();
    }
  });
});

test('Windows MCP and permission proposals preserve native absolute paths', () => {
  const server = { name: 'notes', command: 'C:\\Program Files\\Node\\node.exe',
    folder: 'D:\\Work\\Åda notes', args: ['D:\\Work\\Åda notes\\server.js'] };
  const card = dom('```miniclaw-mcp-server\n' + JSON.stringify(server) + '\n```');
  expect(card.querySelector('[data-mcp-card]')).not.toBeNull();
  const proposal = { name: 'Windows builder', instructions: 'Build the selected project.',
    permissions: [{ kind: 'folder', path: 'D:\\Work\\Åda notes', action: 'write', reason: 'Edit project files' }] };
  expect(dom('```miniclaw-agent\n' + JSON.stringify(proposal) + '\n```').querySelector('[data-agent-card]')).not.toBeNull();
  for (const folder of ['C:relative', 'C:\\work\\..\\private', '\\\\server\\share', '\\\\?\\C:\\work', 'C:\\work\\data:stream']) {
    expect(dom('```miniclaw-mcp-server\n' + JSON.stringify({ ...server, folder }) + '\n```')
      .querySelector('[data-mcp-card]'), folder).toBeNull();
  }
});
