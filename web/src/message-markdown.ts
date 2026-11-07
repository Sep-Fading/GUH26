// Renders assistant messages; raw HTML is always escaped.
import MarkdownIt from 'markdown-it';
// Colours code without loading every language.
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import go from 'highlight.js/lib/languages/go';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import sql from 'highlight.js/lib/languages/sql';
import swift from 'highlight.js/lib/languages/swift';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import java from 'highlight.js/lib/languages/java';
import lua from 'highlight.js/lib/languages/lua';
import ruby from 'highlight.js/lib/languages/ruby';
import php from 'highlight.js/lib/languages/php';
import ini from 'highlight.js/lib/languages/ini';
import markdownGrammar from 'highlight.js/lib/languages/markdown';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import makefile from 'highlight.js/lib/languages/makefile';

for (const [name, language] of Object.entries({ bash, css, diff, go, javascript, json, python,
  rust, sql, swift, typescript, xml, yaml, c, cpp, csharp, java, lua, ruby, php, ini, markdown: markdownGrammar, dockerfile, makefile })) hljs.registerLanguage(name, language);
hljs.registerAliases(['js', 'mjs', 'jsx'], { languageName: 'javascript' });
hljs.registerAliases(['ts', 'tsx'], { languageName: 'typescript' });
hljs.registerAliases(['sh', 'zsh', 'shell'], { languageName: 'bash' });
hljs.registerAliases(['html', 'svelte'], { languageName: 'xml' });
hljs.registerAliases(['py'], { languageName: 'python' });
hljs.registerAliases(['rs'], { languageName: 'rust' });
hljs.registerAliases(['yml'], { languageName: 'yaml' });
hljs.registerAliases(['luau'], { languageName: 'lua' });

// Beyond this a message is shown as plain text, so one reply cannot stall the page.
const LIMIT = 200_000;

// `html: false` makes markdown-it escape every tag a model writes. `breaks` keeps single line
// breaks, as the plain-text view did, so a reply written without Markdown looks unchanged.
const markdown = new MarkdownIt({ html: false, linkify: true, typographer: false, breaks: true });
const escape = markdown.utils.escapeHtml;

// Returns the address only when it is a well-formed https URL.
function https(address: string) {
  try { return new URL(address).protocol === 'https:' ? address : undefined; }
  catch { return undefined; }
}

// markdown-it already refuses javascript:, vbscript:, file: and most data: addresses. Messages
// go further: anything that is not https stays plain text.
markdown.validateLink = address => https(address) !== undefined;
// Bare addresses are linked only when they spell out https themselves.
markdown.linkify.set({ fuzzyLink: false, fuzzyEmail: false, fuzzyIP: false });

// A link opens apart from MiniClaw and shows its real address, whatever its text says.
markdown.renderer.rules.link_open = (tokens, index, options, _env, self) => {
  const token = tokens[index];
  token.attrSet('target', '_blank');
  token.attrSet('rel', 'noopener noreferrer nofollow');
  token.attrSet('title', String(token.attrGet('href') ?? ''));
  return self.renderToken(tokens, index, options);
};

// A remote image would let a reply make the browser contact any server, with whatever the
// model put in the address. It is a button naming the host until the user asks for it.
markdown.renderer.rules.image = (tokens, index) => {
  const token = tokens[index];
  const address = https(String(token.attrGet('src') ?? ''));
  const alt = token.content;
  if (!address) return escape(alt);
  return `<button type="button" class="md-image" data-image="${escape(address)}" data-alt="${escape(alt)}">`
    + `Load image from ${escape(new URL(address).host)}</button>`;
};

// Every fenced or indented block gets a bar with its language and a Copy button. highlight.js
// escapes what it colours; an unknown language is escaped here.
function block(source: string, info: string) {
  const name = info.trim().split(/\s+/)[0] ?? '';
  const known = /^[\w+#.-]{1,32}$/.test(name) && hljs.getLanguage(name) ? name : '';
  const body = known ? hljs.highlight(source, { language: known, ignoreIllegals: true }).value : escape(source);
  const label = /^[\w+#.-]{1,32}$/.test(name) ? name : '';
  return `<div class="md-code"><div class="md-code-bar"><span class="md-code-language">${escape(label)}</span>`
    + `<button type="button" data-copy>Copy</button></div><pre><code class="hljs">${body}</code></pre></div>\n`;
}
// Recognizes local absolute names for proposal cards; the backend decides filesystem access.
function localAbsolutePath(text: string): boolean {
  if (text.startsWith('/') && !text.startsWith('//')) {
    return !text.split('/').some(part => part === '.' || part === '..');
  }
  if (!/^[A-Za-z]:[\\/]/.test(text)) return false;
  return text.slice(3).split(/[\\/]/).every(part => part !== '.' && part !== '..'
    && !/[<>:"|?*]/.test(part) && !/[. ]$/.test(part));
}

// What an agent may propose as an MCP server. Only these fields are read, and each is checked
// with the same rules the backend applies; a request to run unrestricted is never part of it.
export type ServerProposal = { name: string; command: string; args: string[]; folder: string | null; hosts: string[]; ports: number[]; listen: number[]; env: string[] };

// Reads a `miniclaw-mcp-server` block. Anything unexpected makes it an ordinary code block.
export function serverProposal(source: string): ServerProposal | null {
  let value: unknown;
  try { value = JSON.parse(source); } catch { return null; }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const { name, command, args = [], folder = null, hosts = [], ports = [], listen = [], env = [] } = value as Record<string, unknown>;
  const line = (text: unknown, most: number): text is string =>
    typeof text === 'string' && text.length > 0 && text.length <= most && !/[\u0000-\u001f\u007f]/.test(text);
  const list = (items: unknown, most: number, each: (item: string) => boolean): items is string[] =>
    Array.isArray(items) && items.length <= most && items.every(item => typeof item === 'string' && each(item));
  if (!line(name, 40) || !/^[a-z0-9][a-z0-9_-]*$/.test(name) || name.startsWith('miniclaw')) return null;
  if (!line(command, 1024) || !localAbsolutePath(command)) return null;
  if (!list(args, 64, arg => arg.length <= 4096 && !/[\u0000-\u001f\u007f]/.test(arg))) return null;
  if (folder !== null && (!line(folder, 1024) || !localAbsolutePath(folder))) return null;
  if (!list(hosts, 16, host => /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]*$/.test(host))) return null;
  if (!list(env, 32, item => /^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(item))) return null;
  // Ports are whole numbers above the privileged range; the backend refuses MiniClaw's own.
  const portList = (items: unknown): items is number[] => Array.isArray(items) && items.length <= 8
    && items.every(item => Number.isInteger(item) && (item as number) >= 1024 && (item as number) <= 65535);
  if (!portList(ports) || !portList(listen)) return null;
  return { name, command, args, folder, hosts, ports, listen, env };
}

// Shows a proposed server as a card. The button only opens the server form filled in; the user
// reads every field there and saves it themselves.
function card(proposal: ServerProposal) {
  const row = (label: string, text: string) => text
    ? `<div class="md-mcp-row"><dt>${label}</dt><dd>${escape(text)}</dd></div>` : '';
  return `<div class="md-mcp-card" role="group" data-mcp-card aria-label="MCP server ${escape(proposal.name)}">`
    + `<p class="md-mcp-title">MCP server <strong>${escape(proposal.name)}</strong></p><dl>`
    + row('Runs', [proposal.command, ...proposal.args].join(' ')) + row('Folder', proposal.folder ?? '')
    + row('Hosts', proposal.hosts.join(', ')) + row('Connects to ports', proposal.ports.join(', ')) + row('Listens on', proposal.listen.join(', '))
    + row('Needs', proposal.env.join(', ')) + '</dl>'
    + '<p class="md-mcp-note">It is added sandboxed and switched on nowhere. You check every field before saving.</p>'
    + `<button type="button" data-mcp-add="${escape(JSON.stringify(proposal))}">Add this server to MiniClaw</button></div>\n`;
}

// A skill an agent wrote for the user to review. It never says whether the skill is active.
export type SkillProposal = { name: string; description: string; instructions: string; slash: boolean; replaces: string | null };
// One permission an agent suggests for a new agent, with the reason the user judges it by. Each
// becomes one rule only when the user approves it, after the agent is saved.
export type SuggestedPermission = { reason: string } & (
  | { kind: 'feature'; name: 'network' | 'web_fetch' | 'subagents' | 'task_tracking' }
  | { kind: 'folder' | 'file'; path: string; action: 'read' | 'write' }
  | { kind: 'host'; host: string } | { kind: 'domain'; domain: string }
  | { kind: 'mcp_server'; name: string }
  | { kind: 'application'; id: string; action: 'observe' | 'control' });
export type AgentProposal = { name: string; instructions: string; model: string | null; effort: string | null; skills: string[];
  permissions: SuggestedPermission[]; replaces: string | null };

const oneLine = (text: unknown, most: number): text is string =>
  typeof text === 'string' && text.trim().length > 0 && text.length <= most && !/[\u0000-\u001f\u007f]/.test(text);
// Instructions are many lines of Markdown: tabs and line breaks are fine, other control characters are not.
const body = (text: unknown): text is string =>
  typeof text === 'string' && text.trim().length > 0 && text.length <= 16000 && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(text);
const record = (source: string) => {
  let value: unknown;
  try { value = JSON.parse(source); } catch { return null; }
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
};
const hostName = (text: unknown): text is string =>
  typeof text === 'string' && /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]*$/.test(text);

// Reads a `miniclaw-skill` block. Anything unexpected makes it an ordinary code block.
export function skillProposal(source: string): SkillProposal | null {
  const value = record(source);
  if (!value) return null;
  const { name, description = '', instructions, slash = false, replaces = null } = value;
  if (!oneLine(name, 80) || !body(instructions) || typeof slash !== 'boolean') return null;
  if (description !== '' && !oneLine(description, 300)) return null;
  if (replaces !== null && !oneLine(replaces, 80)) return null;
  return { name: name.trim(), description: (description as string).trim(), instructions, slash, replaces: replaces as string | null };
}

// Reads one suggested permission, keeping only the fields of its kind.
function suggested(value: unknown): SuggestedPermission | null {
  if (!value || typeof value !== 'object') return null;
  const item = value as Record<string, unknown>;
  if (!oneLine(item.reason, 300)) return null;
  const reason = item.reason.trim();
  const path = (text: unknown): text is string => oneLine(text, 1024) && localAbsolutePath(text) && text !== '/' && !/^[A-Za-z]:[\\/]$/.test(text);
  if (item.kind === 'feature' && (item.name === 'network' || item.name === 'web_fetch'
    || item.name === 'subagents' || item.name === 'task_tracking')) return { kind: 'feature', name: item.name, reason };
  if ((item.kind === 'folder' || item.kind === 'file') && path(item.path) && (item.action === 'read' || item.action === 'write'))
    return { kind: item.kind, path: item.path, action: item.action, reason };
  if (item.kind === 'host' && hostName(item.host)) return { kind: 'host', host: item.host, reason };
  if (item.kind === 'domain' && hostName(item.domain)) return { kind: 'domain', domain: item.domain, reason };
  if (item.kind === 'mcp_server' && oneLine(item.name, 40)) return { kind: 'mcp_server', name: item.name, reason };
  if (item.kind === 'application' && oneLine(item.id, 200) && (item.action === 'observe' || item.action === 'control'))
    return { kind: 'application', id: item.id, action: item.action, reason };
  return null;
}

// Reads a `miniclaw-agent` block. One bad permission spoils the whole block, so nothing is dropped silently.
export function agentProposal(source: string): AgentProposal | null {
  const value = record(source);
  if (!value) return null;
  const { name, instructions, model = null, effort = null, skills = [], permissions = [], replaces = null } = value;
  if (!oneLine(name, 80) || !body(instructions)) return null;
  if ((model !== null && !oneLine(model, 100)) || (effort !== null && !oneLine(effort, 32))) return null;
  if (!Array.isArray(skills) || skills.length > 8 || !skills.every(skill => oneLine(skill, 80))) return null;
  if (!Array.isArray(permissions) || permissions.length > 12) return null;
  const checked = permissions.map(suggested);
  if (checked.some(item => !item)) return null;
  if (replaces !== null && !oneLine(replaces, 80)) return null;
  return { name: name.trim(), instructions, model: model as string | null, effort: effort as string | null, skills: skills as string[],
    permissions: checked as SuggestedPermission[], replaces: replaces as string | null };
}

// Says one suggested permission in the words the permission settings use.
export function permissionLabel(item: SuggestedPermission) {
  switch (item.kind) {
    case 'feature': return { network: 'Network for commands', web_fetch: 'Web fetch',
      subagents: 'Subagents', task_tracking: 'Task tracking' }[item.name];
    case 'folder': case 'file': return `${item.action === 'read' ? 'Read' : 'Write'} ${item.kind} ${item.path}`;
    case 'host': return `Reach host ${item.host}`;
    case 'domain': return `Reach domain ${item.domain}`;
    case 'mcp_server': return `MCP server ${item.name}`;
    case 'application': return `${item.action === 'observe' ? 'Observe' : 'Control'} ${item.id}`;
  }
}

// Shows a proposed skill. The button opens the skill form filled in; nothing is saved from here.
function skillCard(proposal: SkillProposal) {
  return `<div class="md-mcp-card" role="group" data-skill-card aria-label="Skill ${escape(proposal.name)}">`
    + `<p class="md-mcp-title">Skill <strong>${escape(proposal.name)}</strong></p>`
    + (proposal.replaces ? `<p class="md-mcp-note">New version of ${escape(proposal.replaces)}</p>` : '')
    + (proposal.description ? `<p>${escape(proposal.description)}</p>` : '')
    + `<details><summary>Instructions</summary><pre class="md-plain">${escape(proposal.instructions)}</pre></details>`
    + '<p class="md-mcp-note">Nothing is saved until you review it and press Save. It arrives inactive.</p>'
    + `<button type="button" data-skill-add="${escape(JSON.stringify(proposal))}">Review this skill</button></div>\n`;
}

// Shows a proposed agent with every suggested permission and its reason.
function agentCard(proposal: AgentProposal) {
  const rows = proposal.permissions.map(item =>
    `<div class="md-mcp-row"><dt>${escape(permissionLabel(item))}</dt><dd>${escape(item.reason)}</dd></div>`).join('');
  return `<div class="md-mcp-card" role="group" data-agent-card aria-label="Agent ${escape(proposal.name)}">`
    + `<p class="md-mcp-title">Agent <strong>${escape(proposal.name)}</strong></p>`
    + (proposal.replaces ? `<p class="md-mcp-note">New version of ${escape(proposal.replaces)}</p>` : '')
    + `<dl><div class="md-mcp-row"><dt>Model</dt><dd>${escape(proposal.model ?? 'Default')}${proposal.effort ? `, ${escape(proposal.effort)}` : ''}</dd></div>`
    + (proposal.skills.length ? `<div class="md-mcp-row"><dt>Skills</dt><dd>${escape(proposal.skills.join(', '))}</dd></div>` : '') + '</dl>'
    + `<details><summary>Instructions</summary><pre class="md-plain">${escape(proposal.instructions)}</pre></details>`
    + (rows ? `<p class="md-mcp-title">Suggested permissions</p><dl>${rows}</dl>`
      + '<p class="md-mcp-note">None of these is granted. You approve each one yourself after saving the agent.</p>'
      : '<p class="md-mcp-note">It suggests no permissions. Nothing is saved until you review it and press Save.</p>')
    + `<button type="button" data-agent-add="${escape(JSON.stringify(proposal))}">Review this agent</button></div>\n`;
}

markdown.renderer.rules.fence = (tokens, index) => {
  const { content, info } = tokens[index];
  const kind = info.trim();
  const server = kind === 'miniclaw-mcp-server' ? serverProposal(content) : null;
  if (server) return card(server);
  const skill = kind === 'miniclaw-skill' ? skillProposal(content) : null;
  if (skill) return skillCard(skill);
  const agent = kind === 'miniclaw-agent' ? agentProposal(content) : null;
  return agent ? agentCard(agent) : block(content, info);
};
markdown.renderer.rules.code_block = (tokens, index) => block(tokens[index].content, '');

// Colours one piece of code for a known language, or escapes it. The result is inert HTML.
export function highlightCode(source: string, language?: string) {
  return language && hljs.getLanguage(language)
    ? hljs.highlight(source, { language, ignoreIllegals: true }).value : escape(source);
}

// Highlights a complete file, then balances token spans on each numbered display line.
export function highlightLines(source: string, language?: string): string[] {
  source = source.replace(/\n$/, '');
  const html = source.length > LIMIT ? escape(source) : highlightCode(source, language);
  const spans: string[] = [];
  return html.split('\n').map(line => {
    const prefix = spans.join('');
    for (const tag of line.match(/<\/?span\b[^>]*>/g) ?? []) {
      if (tag.startsWith('</')) spans.pop(); else spans.push(tag);
    }
    return prefix + line + '</span>'.repeat(spans.length);
  });
}

// Turns one assistant message into inert HTML for the page.
export function renderMessage(source: string) {
  if (source.length > LIMIT) return `<pre class="md-plain">${escape(source)}</pre>`;
  return markdown.render(source);
}
