// Names the file of a native path on either platform.
import { fileName } from './paths';
// Turns a unified diff into rows a review view can draw: numbered, typed, and with the exact
// characters that changed inside a rewritten line.

export type DiffRow = { kind: 'hunk' | 'context' | 'added' | 'removed' | 'note'; text: string;
  before: number | null; after: number | null;
  // The [start, end) of the part of `text` that differs from its counterpart line.
  emphasis?: [number, number] };
export type DiffPair = { left?: DiffRow; right?: DiffRow };

// Finds what differs between two lines by their common start and end. Lines that share too
// little are left unmarked: marking nearly everything says nothing.
function differing(before: string, after: string): [[number, number], [number, number]] | undefined {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let end = 0;
  while (end < before.length - start && end < after.length - start
    && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  const shared = start + end;
  if (shared < Math.max(before.length, after.length) * 0.4) return undefined;
  return [[start, before.length - end], [start, after.length - end]];
}

// Reads hunks only; whatever a tool prints before the first `@@` is not part of the change.
export function parseDiff(diff: string) {
  const rows: DiffRow[] = [];
  let before = 0, after = 0, added = 0, removed = 0, started = false;
  const lines = diff.split('\n');
  if (lines.at(-1) === '') lines.pop();
  for (const line of lines) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      started = true; before = Number(hunk[1]); after = Number(hunk[2]);
      rows.push({ kind: 'hunk', text: line, before: null, after: null });
    } else if (!started) continue;
    else if (line.startsWith('+')) { rows.push({ kind: 'added', text: line.slice(1), before: null, after: after++ }); added++; }
    else if (line.startsWith('-')) { rows.push({ kind: 'removed', text: line.slice(1), before: before++, after: null }); removed++; }
    else if (line.startsWith('\\') || line.startsWith('...')) rows.push({ kind: 'note', text: line.replace(/^\\ /, ''), before: null, after: null });
    else rows.push({ kind: 'context', text: line.slice(1), before: before++, after: after++ });
  }
  // A run of removed lines followed by as many added lines is a rewrite, compared line by line.
  for (let index = 0; index < rows.length; index++) {
    if (rows[index].kind !== 'removed') continue;
    let gone = index;
    while (rows[gone]?.kind === 'removed') gone++;
    let came = gone;
    while (rows[came]?.kind === 'added') came++;
    if (gone - index === came - gone) {
      for (let offset = 0; offset < gone - index; offset++) {
        const marks = differing(rows[index + offset].text, rows[gone + offset].text);
        if (marks) [rows[index + offset].emphasis, rows[gone + offset].emphasis] = marks;
      }
    }
    index = came - 1;
  }
  return { rows, added, removed };
}

// Arranges rows for a side-by-side view: old on the left, new on the right.
export function splitRows(rows: DiffRow[]): DiffPair[] {
  const pairs: DiffPair[] = [];
  for (let index = 0; index < rows.length;) {
    const row = rows[index];
    if (row.kind === 'removed' || row.kind === 'added') {
      const gone: DiffRow[] = [], came: DiffRow[] = [];
      while (rows[index]?.kind === 'removed') gone.push(rows[index++]);
      while (rows[index]?.kind === 'added') came.push(rows[index++]);
      for (let offset = 0; offset < Math.max(gone.length, came.length); offset++) {
        pairs.push({ left: gone[offset], right: came[offset] });
      }
    } else { pairs.push({ left: row, right: row }); index++; }
  }
  return pairs;
}

const languages: Record<string, string> = { rs: 'rust', ts: 'typescript', tsx: 'typescript', js: 'javascript',
  mjs: 'javascript', jsx: 'javascript', json: 'json', py: 'python', go: 'go', swift: 'swift', css: 'css',
  html: 'xml', svelte: 'xml', xml: 'xml', yml: 'yaml', yaml: 'yaml', sh: 'bash', zsh: 'bash', sql: 'sql', diff: 'diff',
  c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', cs: 'csharp', java: 'java',
  lua: 'lua', luau: 'lua', rb: 'ruby', php: 'php', ini: 'ini', toml: 'ini',
  md: 'markdown', markdown: 'markdown', cjs: 'javascript', mts: 'typescript', cts: 'typescript' };

// Picks syntax from familiar filenames or a case-insensitive extension.
export function languageOf(path: string) {
  const name = fileName(path);
  const known: Record<string, string> = { dockerfile: 'dockerfile', makefile: 'makefile',
    gnumakefile: 'makefile', gemfile: 'ruby', rakefile: 'ruby' };
  if (known[name.toLowerCase()]) return known[name.toLowerCase()];
  return name.includes('.') ? languages[name.split('.').pop()!.toLowerCase()] : undefined;
}
