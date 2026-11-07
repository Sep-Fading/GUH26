// Keeps source changes and decorations transactional.
import { Compartment, EditorState, StateField, type Range } from '@codemirror/state';
// Renders formatted text without interpreting HTML.
import { Decoration, EditorView, keymap, type DecorationSet } from '@codemirror/view';
// Supplies cursor commands and undo history.
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
// Parses Markdown and continues list markup.
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';

type Formatting = { visible: DecorationSet; hidden: DecorationSet };
const inline: Record<string, string> = {
  Emphasis: 'em', StrongEmphasis: 'strong', InlineCode: 'code', Strikethrough: 's',
};

// Styles parsed ranges while leaving every Markdown character in the document.
function format(state: EditorState): Formatting {
  const marks: Range<Decoration>[] = [];
  const hidden: Range<Decoration>[] = [];
  const tree = markdownLanguage.parser.parse(state.doc.toString());
  tree.iterate({ enter(node) {
    const { name, from, to } = node;
    if (inline[name]) marks.push(Decoration.mark({ tagName: inline[name] }).range(from, to));
    if (/^(EmphasisMark|CodeMark|StrikethroughMark)$/.test(name)) {
      hidden.push(Decoration.replace({}).range(from, to));
    }
    if (name.startsWith('ATXHeading')) {
      marks.push(Decoration.line({ attributes: { role: 'heading',
        'aria-level': name.slice(-1), class: 'instruction-heading' } }).range(state.doc.lineAt(from).from));
    }
    if (name === 'HeaderMark') {
      const end = state.doc.sliceString(to, to + 1) === ' ' ? to + 1 : to;
      hidden.push(Decoration.replace({}).range(from, end));
    }
    if (name === 'CodeInfo') hidden.push(Decoration.replace({}).range(from, to));
    if (name === 'CodeText') {
      marks.push(Decoration.mark({ tagName: 'code' }).range(from, to));
      for (let line = state.doc.lineAt(from); line.from <= to;) {
        marks.push(Decoration.line({ class: 'instruction-code' }).range(line.from));
        if (line.number === state.doc.lines) break;
        line = state.doc.line(line.number + 1);
      }
    }
    if (name === 'ListItem' || name === 'Blockquote') {
      marks.push(Decoration.line({ class: name === 'ListItem' ? 'instruction-list' : 'instruction-quote' })
        .range(state.doc.lineAt(from).from));
    }
  } });
  return { visible: Decoration.set([...marks, ...hidden], true), hidden: Decoration.set(hidden, true) };
}

const formatting = StateField.define<Formatting>({
  create: format,
  // Reparse text changes; selection changes retain the display ranges.
  update(value, transaction) { return transaction.docChanged ? format(transaction.state) : value; },
  provide: field => [EditorView.decorations.from(field, value => value.visible),
    EditorView.atomicRanges.of(view => view.state.field(field).hidden)],
});

// Converts a completed spaced italic shortcut outside literal code to standard Markdown.
function spacedItalic(view: EditorView, from: number, to: number, text: string): boolean {
  if (text !== '*' || from !== to || view.composing) return false;
  const line = view.state.doc.lineAt(from);
  const before = view.state.sliceDoc(line.from, from);
  const match = /(^|\s)\* ([^*\n]+?) $/.exec(before);
  if (!match) return false;
  const tree = markdownLanguage.parser.parse(view.state.doc.toString());
  for (let node = tree.resolveInner(from, -1); node; node = node.parent!) {
    if (['InlineCode', 'FencedCode', 'CodeBlock', 'HTMLBlock', 'HTMLTag'].includes(node.name)) return false;
  }
  const start = line.from + match.index + match[1].length;
  const insert = `*${match[2].trim()}*`;
  view.dispatch({ changes: { from: start, to, insert }, selection: { anchor: start + insert.length },
    userEvent: 'input.type' });
  return true;
}

// Locks editing without discarding selection or undo history.
export function editable(disabled: boolean) {
  return [EditorState.readOnly.of(disabled), EditorView.editable.of(!disabled),
    EditorView.contentAttributes.of({ 'aria-readonly': String(disabled) })];
}

// Builds a source-preserving state with bounded input and no resource loading.
export function editorState(source: string, label: string, lock: Compartment,
  disabled: boolean, onchange: (source: string) => void): EditorState {
  return EditorState.create({ doc: source, extensions: [
    markdown({ base: markdownLanguage, completeHTMLTags: false, pasteURLAsLink: false }),
    history(), keymap.of([...historyKeymap, ...defaultKeymap]), formatting,
    EditorView.inputHandler.of(spacedItalic), EditorView.lineWrapping,
    lock.of(editable(disabled)),
    EditorView.contentAttributes.of({ 'aria-labelledby': label, 'aria-multiline': 'true', spellcheck: 'true' }),
    EditorState.changeFilter.of(tr => !tr.docChanged || tr.newDoc.length <= 16384),
    EditorView.updateListener.of(update => {
      if (update.docChanged) onchange(update.state.sliceDoc());
    }),
    EditorState.lineSeparator.of(source.includes('\r\n') ? '\r\n' : '\n'),
  ] });
}

// Mounts an editor whose external updates do not enter the user's undo history.
export function mountEditor(parent: HTMLElement, source: string, label: string,
  disabled: boolean, onchange: (source: string) => void) {
  const lock = new Compartment();
  const view = new EditorView({ parent, state: editorState(source, label, lock, disabled, onchange) });
  return {
    // Loads a different draft; ordinary typing retains the current state and selection.
    setSource(value: string, disabled: boolean) {
      if (value !== view.state.sliceDoc()) view.setState(editorState(value, label, lock, disabled, onchange));
    },
    // Locks pending saves without clearing undo or moving the cursor.
    setDisabled(value: boolean) { view.dispatch({ effects: lock.reconfigure(editable(value)) }); },
    // Releases DOM observers and keyboard handlers when the dialog closes.
    destroy() { view.destroy(); },
  };
}
