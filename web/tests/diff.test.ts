// Checks how a unified diff becomes rows for the review views.
import { describe, expect, test } from 'vitest';
import { parseDiff, splitRows, languageOf } from '../src/diff';

const sample = '@@ -10,5 +10,6 @@ fn greet()\n /// Greets.\n-fn old_name(a: u8) {\n+fn new_name(a: u8) {\n     println!("hi");\n+    done();\n }\n\\ No newline at end of file\n';

describe('unified diff rows', () => {
  test('lines carry their old and new numbers', () => {
    const { rows, added, removed } = parseDiff(sample);
    expect([added, removed]).toEqual([2, 1]);
    expect(rows.map(row => [row.kind, row.before, row.after])).toEqual([
      ['hunk', null, null], ['context', 10, 10], ['removed', 11, null], ['added', null, 11],
      ['context', 12, 12], ['added', null, 13], ['context', 13, 14], ['note', null, null]]);
    expect(rows[0].text).toBe('@@ -10,5 +10,6 @@ fn greet()');
    expect(rows[2].text).toBe('fn old_name(a: u8) {');
  });

  test('a changed line marks exactly the characters that differ', () => {
    const { rows } = parseDiff(sample);
    const [removed, added] = [rows[2], rows[3]];
    expect(removed.text.slice(...removed.emphasis!)).toBe('old');
    expect(added.text.slice(...added.emphasis!)).toBe('new');
    // A line that was only added has nothing to compare with.
    expect(rows[5].emphasis).toBeUndefined();
  });

  test('a wholly rewritten line is not marked character by character', () => {
    const { rows } = parseDiff('@@ -1 +1 @@\n-alpha beta gamma\n+something else entirely\n');
    expect(rows[1].emphasis).toBeUndefined();
  });

  test('side by side pairs removed with added lines and leaves gaps elsewhere', () => {
    const pairs = splitRows(parseDiff(sample).rows);
    expect(pairs.map(pair => [pair.left?.kind ?? null, pair.right?.kind ?? null])).toEqual([
      ['hunk', 'hunk'], ['context', 'context'], ['removed', 'added'], ['context', 'context'],
      [null, 'added'], ['context', 'context'], ['note', 'note']]);
  });

  test('git headers before the first hunk are ignored, and an empty diff has no rows', () => {
    const { rows } = parseDiff('diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n');
    expect(rows.map(row => row.kind)).toEqual(['hunk', 'removed', 'added']);
    expect(parseDiff('').rows).toEqual([]);
  });

  test('the file name picks the syntax, and an unknown one picks none', () => {
    expect(languageOf('src/main.rs')).toBe('rust');
    expect(languageOf('web/App.svelte')).toBe('xml');
    expect(languageOf('Makefile')).toBe('makefile');
    expect(languageOf('notes.txt')).toBeUndefined();
  });
});
