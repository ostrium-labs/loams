// The one query editor of the console (SQL in its dialects, and GQL): CodeMirror 6 with
// highlighting painted from the design tokens, schema-aware completion, signature hints for
// function calls, and the error span the engine named underlined in place.
//
// Keyboard: Ctrl/Cmd+Enter runs the whole text; Ctrl/Cmd+Shift+Enter runs the selection (or the
// statement under the cursor); Ctrl+Space opens completions, Enter or Tab accepts one, Escape
// closes it. Tab is not captured otherwise, so the keyboard can always leave the editor.

import {
  acceptCompletion,
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { bracketMatching, indentOnInput } from '@codemirror/language';
import { Compartment, EditorState, Prec, StateEffect, StateField } from '@codemirror/state';
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder as placeholderExt,
} from '@codemirror/view';
import { type Ref, useEffect, useImperativeHandle, useRef } from 'react';
import { cx } from './cx';
import { offsetOfPosition, type SqlPosition } from './sql-error';
import {
  EMPTY_SCHEMA,
  languageExtension,
  type SqlDialectName,
  type SqlSchema,
} from './sql-language';

export interface SqlEditorHandle {
  focus(): void;
  /** The selected text, or the statement under the cursor when nothing is selected. */
  selectionOrStatement(): string;
  /** Replaces the whole text (an undoable edit). */
  replace(text: string): void;
  /** Inserts text at the cursor (replacing the selection) and focuses the editor. */
  insert(text: string): void;
}

export interface SqlEditorProps {
  value: string;
  onChange: (text: string) => void;
  /** `selection` is true for Ctrl/Cmd+Shift+Enter: `text` is then the selection or statement. */
  onRun?: (text: string, selection: boolean) => void;
  dialect?: SqlDialectName;
  schema?: SqlSchema;
  /** The editor's accessible name. */
  label: string;
  describedBy?: string;
  invalid?: boolean;
  /** The span the engine named in its error, underlined until the text changes. */
  errorAt?: SqlPosition;
  placeholder?: string;
  readOnly?: boolean;
  /** Visible lines before the editor scrolls (it grows up to `maxLines`). */
  minLines?: number;
  maxLines?: number;
  className?: string;
  handle?: Ref<SqlEditorHandle>;
}

/** The statement around `pos`: the text between the semicolons on either side, trimmed. */
export function statementAt(text: string, pos: number): string {
  const bounds: number[] = [];
  let quote: string | undefined;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = undefined;
    } else if (c === "'" || c === '"' || c === '`') quote = c;
    else if (c === '-' && text[i + 1] === '-') {
      const nl = text.indexOf('\n', i);
      i = nl < 0 ? text.length : nl;
    } else if (c === ';') bounds.push(i);
  }
  let start = 0;
  let end = text.length;
  for (const b of bounds) {
    if (b < pos) start = b + 1;
    else {
      end = b;
      break;
    }
  }
  const piece = text.slice(start, end).trim();
  if (piece || bounds.length === 0) return piece;
  // The cursor sits after the last `;`: the statement before it.
  const prev = bounds.filter((b) => b < pos);
  const s = prev.length > 1 ? (prev[prev.length - 2] as number) + 1 : 0;
  return text.slice(s, prev[prev.length - 1]).trim();
}

const setError = StateEffect.define<{ from: number; to: number } | null>();

const errorField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    for (const e of tr.effects) {
      if (e.is(setError)) {
        if (!e.value) return Decoration.none;
        const { from, to } = e.value;
        return Decoration.set([
          Decoration.mark({ class: 'loams-cm-error', attributes: { 'data-error': 'true' } }).range(
            from,
            Math.max(to, from + 1),
          ),
        ]);
      }
    }
    // Any edit clears the mark: the span no longer matches what the engine saw.
    return tr.docChanged ? Decoration.none : deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

const theme = (minLines: number, maxLines: number) =>
  EditorView.theme({
    '&': {
      fontSize: 'var(--op-text-body)',
      backgroundColor: 'var(--op-surface)',
      color: 'var(--op-ink)',
    },
    '.cm-scroller': {
      fontFamily: 'var(--loams-font-mono)',
      lineHeight: '1.6',
      minHeight: `calc(${minLines} * 1.6em + 12px)`,
      maxHeight: `calc(${maxLines} * 1.6em + 12px)`,
      overflow: 'auto',
    },
    '.cm-content': { padding: '6px 0', caretColor: 'var(--op-ink)' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--op-ink)' },
    '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': {
      backgroundColor: 'var(--op-term-selection)',
    },
    '.cm-gutters': {
      backgroundColor: 'var(--op-surface)',
      color: 'var(--op-faint)',
      border: 'none',
      borderRight: '1px solid var(--op-rule)',
    },
    '.cm-activeLine': { backgroundColor: 'var(--op-row-hover)' },
    '.cm-activeLineGutter': { backgroundColor: 'var(--op-row-hover)', color: 'var(--op-muted)' },
    '.cm-placeholder': { color: 'var(--op-faint)' },
    '&.cm-focused .cm-matchingBracket': {
      backgroundColor: 'var(--op-accent-soft)',
      outline: '1px solid var(--op-accent)',
    },
    '.cm-tooltip': {
      backgroundColor: 'var(--op-surface)',
      color: 'var(--op-ink)',
      border: '1px solid var(--op-rule)',
      borderRadius: 'var(--op-radius-md)',
      boxShadow: 'var(--op-shadow-pop)',
      overflow: 'hidden',
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul': {
      fontFamily: 'var(--loams-font-mono)',
      fontSize: 'var(--op-text-small)',
      maxHeight: '16em',
      minWidth: '18em',
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li': {
      padding: '3px 8px',
      lineHeight: '1.5',
    },
    '.cm-tooltip-autocomplete ul li[aria-selected]': {
      backgroundColor: 'var(--op-accent-soft)',
      color: 'var(--op-ink)',
    },
    '.cm-completionDetail': {
      marginLeft: '1.5em',
      color: 'var(--op-faint)',
      fontStyle: 'normal',
      float: 'right',
    },
    '.cm-completionMatchedText': {
      textDecoration: 'none',
      color: 'var(--op-accent-fg)',
      fontWeight: '600',
    },
    '.cm-completionIcon': { opacity: '0.8', color: 'var(--op-muted)' },
    '.cm-completionInfo': {
      padding: '6px 10px',
      maxWidth: '28em',
      whiteSpace: 'pre-wrap',
      fontFamily: 'var(--loams-font-sans)',
      fontSize: 'var(--op-text-small)',
      color: 'var(--op-muted)',
    },
  });

/** The query editor. Controlled: `value` is the text, `onChange` gets every edit. */
export function SqlEditor({
  value,
  onChange,
  onRun,
  dialect = 'generic',
  schema = EMPTY_SCHEMA,
  label,
  describedBy,
  invalid,
  errorAt,
  placeholder,
  readOnly,
  minLines = 6,
  maxLines = 18,
  className,
  handle,
}: SqlEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const comps = useRef({
    lang: new Compartment(),
    attrs: new Compartment(),
    ph: new Compartment(),
    ro: new Compartment(),
  });
  // The latest callbacks, read by the keymap and the update listener.
  const latest = useRef({ onChange, onRun });
  latest.current = { onChange, onRun };

  // oxlint-disable-next-line react/exhaustive-deps -- the view is made once; props sync below
  useEffect(() => {
    if (!host.current) return;
    const c = comps.current;
    const selectionOrStatement = (v: EditorView) => {
      const sel = v.state.selection.main;
      const doc = v.state.doc.toString();
      return sel.empty ? statementAt(doc, sel.head) : v.state.sliceDoc(sel.from, sel.to);
    };
    const runAll = (v: EditorView) => {
      latest.current.onRun?.(v.state.doc.toString(), false);
      return true;
    };
    const runPart = (v: EditorView) => {
      latest.current.onRun?.(selectionOrStatement(v), true);
      return true;
    };
    // Ctrl and Cmd both run, on every platform (CodeMirror's `Mod` is only one of them).
    const runKeys = Prec.highest(
      keymap.of([
        { key: 'Ctrl-Enter', run: runAll },
        { key: 'Meta-Enter', run: runAll },
        { key: 'Ctrl-Shift-Enter', run: runPart },
        { key: 'Meta-Shift-Enter', run: runPart },
        { key: 'Tab', run: acceptCompletion },
      ]),
    );
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          runKeys,
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          closeBrackets(),
          autocompletion({
            activateOnTyping: true,
            maxRenderedOptions: 60,
            icons: true,
            defaultKeymap: true,
            aboveCursor: false,
          }),
          keymap.of([
            ...closeBracketsKeymap,
            ...defaultKeymap,
            ...historyKeymap,
            ...completionKeymap,
          ]),
          EditorView.lineWrapping,
          errorField,
          theme(minLines, maxLines),
          c.lang.of(languageExtension(dialect, schema)),
          c.attrs.of(contentAttrs(label, describedBy, invalid)),
          c.ph.of(placeholder ? placeholderExt(placeholder) : []),
          c.ro.of(EditorState.readOnly.of(!!readOnly)),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) latest.current.onChange(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
  }, []);

  useImperativeHandle(
    handle,
    () => ({
      focus: () => view.current?.focus(),
      selectionOrStatement: () => {
        const v = view.current;
        if (!v) return value;
        const sel = v.state.selection.main;
        return sel.empty
          ? statementAt(v.state.doc.toString(), sel.head)
          : v.state.sliceDoc(sel.from, sel.to);
      },
      replace: (text: string) => {
        const v = view.current;
        if (!v) return;
        v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
        v.focus();
      },
      insert: (text: string) => {
        const v = view.current;
        if (!v) return;
        v.dispatch(v.state.replaceSelection(text));
        v.focus();
      },
    }),
    [value],
  );

  // A new value from outside (history, an example, a reset) replaces the text.
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const cur = v.state.doc.toString();
    if (cur !== value) v.dispatch({ changes: { from: 0, to: cur.length, insert: value } });
  }, [value]);

  useEffect(() => {
    view.current?.dispatch({
      effects: comps.current.lang.reconfigure(languageExtension(dialect, schema)),
    });
  }, [dialect, schema]);

  useEffect(() => {
    view.current?.dispatch({
      effects: comps.current.attrs.reconfigure(contentAttrs(label, describedBy, invalid)),
    });
  }, [label, describedBy, invalid]);

  useEffect(() => {
    view.current?.dispatch({
      effects: [
        comps.current.ph.reconfigure(placeholder ? placeholderExt(placeholder) : []),
        comps.current.ro.reconfigure(EditorState.readOnly.of(!!readOnly)),
      ],
    });
  }, [placeholder, readOnly]);

  useEffect(() => {
    const v = view.current;
    if (!v) return;
    if (!errorAt) {
      v.dispatch({ effects: setError.of(null) });
      return;
    }
    const doc = v.state.doc.toString();
    const from = offsetOfPosition(doc, errorAt);
    const to = Math.min(doc.length, from + Math.max(1, errorAt.length ?? 1));
    v.dispatch({ effects: setError.of({ from, to }) });
  }, [errorAt]);

  return (
    <div
      ref={host}
      className={cx('loams-sql-editor', invalid && 'loams-sql-editor-invalid', className)}
      data-dialect={dialect}
    />
  );
}

function contentAttrs(label: string, describedBy?: string, invalid?: boolean) {
  const attrs: Record<string, string> = { 'aria-label': label, spellcheck: 'false' };
  if (describedBy) attrs['aria-describedby'] = describedBy;
  if (invalid) attrs['aria-invalid'] = 'true';
  return EditorView.contentAttributes.of(attrs);
}

/**
 * Replaces the text of the editor that `element` belongs to (its labelled content element, or
 * any element inside it), as typing would. For tests and scripted input.
 */
export function setCodeEditorText(element: Element, text: string): void {
  const v = EditorView.findFromDOM(element as HTMLElement);
  if (!v) throw new Error('setCodeEditorText: the element is not inside a code editor');
  v.dispatch({
    changes: { from: 0, to: v.state.doc.length, insert: text },
    selection: { anchor: text.length },
    userEvent: 'input.type',
  });
}

/** The text of the editor that `element` belongs to. */
export function codeEditorText(element: Element): string {
  const v = EditorView.findFromDOM(element as HTMLElement);
  if (!v) throw new Error('codeEditorText: the element is not inside a code editor');
  return v.state.doc.toString();
}
