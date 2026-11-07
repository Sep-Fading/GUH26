<script lang="ts">
  // Draws a unified diff the way a code review does: numbered lines, whole-line colour, and the
  // changed characters picked out. It can also lay the change out side by side.
  import { parseDiff, splitRows, languageOf, type DiffRow } from './diff';
  // Colours code; its output is escaped HTML.
  import { highlightCode } from './message-markdown';
  import Columns2 from '@lucide/svelte/icons/columns-2';
  import Rows3 from '@lucide/svelte/icons/rows-3';

  let { diff, path = '', toolbar = true }: { diff: string; path?: string; toolbar?: boolean } = $props();

  // The layout is a reading preference, so it is remembered for this browser.
  let split = $state(false);
  try { split = localStorage.getItem('miniclaw.diff.layout') === 'split'; } catch { /* Storage may be off. */ }
  function layout(next: boolean) {
    split = next;
    try { localStorage.setItem('miniclaw.diff.layout', next ? 'split' : 'unified'); } catch { /* Shown for this visit. */ }
  }

  const parsed = $derived(parseDiff(diff));
  const pairs = $derived(splitRows(parsed.rows));
  const language = $derived(languageOf(path));

  // Colours a line, wrapping the characters that differ from its counterpart.
  function paint(row: DiffRow) {
    if (!row.emphasis) return highlightCode(row.text, language) || ' ';
    const [start, end] = row.emphasis;
    return `${highlightCode(row.text.slice(0, start), language)}<mark>${highlightCode(row.text.slice(start, end), language)}</mark>`
      + highlightCode(row.text.slice(end), language);
  }
  const sign = (row?: DiffRow) => row?.kind === 'added' ? '+' : row?.kind === 'removed' ? '−' : '';
</script>

<div class="diffview">
  {#if toolbar}
    <div class="diffview-bar">
      <span class="diffview-count"><span class="plus">+{parsed.added}</span> <span class="minus">−{parsed.removed}</span></span>
      <div role="group" aria-label="Diff layout" class="diffview-switch">
        <button aria-pressed={!split} title="One column" onclick={() => layout(false)}><Rows3 size={14} aria-hidden="true" />Unified</button>
        <button aria-pressed={split} title="Old and new side by side" onclick={() => layout(true)}><Columns2 size={14} aria-hidden="true" />Side by side</button>
      </div>
    </div>
  {/if}
  {#if !parsed.rows.length}
    <p class="diffview-empty">No changes to show.</p>
  {:else if split}
    <table class="diffview-table split" aria-label={`Changes to ${path || 'the file'}, side by side`}>
      <!-- A fixed table shares width equally unless told otherwise: numbers stay narrow. -->
      <colgroup><col class="narrow" /><col /><col class="narrow" /><col /></colgroup>
      <tbody>
        {#each pairs as pair, index (index)}
          {#if pair.left?.kind === 'hunk'}
            <tr class="hunk"><td colspan="4">{pair.left.text}</td></tr>
          {:else if pair.left?.kind === 'note'}
            <tr class="note"><td colspan="4">{pair.left.text}</td></tr>
          {:else}
            <tr>
              <td class="number {pair.left?.kind ?? 'gap'}">{pair.left?.before ?? ''}</td>
              <td class="code {pair.left?.kind ?? 'gap'}"><span class="sign">{sign(pair.left)}</span>{#if pair.left}{@html paint(pair.left)}{/if}</td>
              <td class="number {pair.right?.kind ?? 'gap'}">{pair.right?.after ?? ''}</td>
              <td class="code {pair.right?.kind ?? 'gap'}"><span class="sign">{sign(pair.right)}</span>{#if pair.right}{@html paint(pair.right)}{/if}</td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {:else}
    <table class="diffview-table" aria-label={`Changes to ${path || 'the file'}`}>
      <tbody>
        {#each parsed.rows as row, index (index)}
          {#if row.kind === 'hunk' || row.kind === 'note'}
            <tr class={row.kind}><td colspan="3">{row.text}</td></tr>
          {:else}
            <tr>
              <td class="number {row.kind}">{row.before ?? ''}</td>
              <td class="number {row.kind}">{row.after ?? ''}</td>
              <td class="code {row.kind}"><span class="sign">{sign(row)}</span>{@html paint(row)}</td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {/if}
</div>
