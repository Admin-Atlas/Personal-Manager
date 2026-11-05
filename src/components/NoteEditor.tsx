// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A pinboard note's text, formatted as you type it (live preview). It used to be a plain textarea that
// showed raw Markdown until you clicked away, when a separate render took over. Bobby: "it is starting
// to get annoying how the formating only takes place after i click off and i have to edit in plain".
//
// Built on CodeMirror 6 (state + view only: no language packages, no keymaps). The note stays one
// Markdown string; `previewMarks` decides how to draw it, and this file only turns those marks into
// decorations. Every edit PM makes itself — the toolbar, the shortcuts, list continuation, the table
// keys — is still a pure `TextEdit` from notesMarkdown / noteTables, applied as one transaction.
//
// Undo stays the board's (PinboardView owns Ctrl+Z for the whole board), so there is no editor
// history here: an undo arrives as a new `value`, applied as the smallest change that gets there,
// which lets CodeMirror carry the cursor across it.

import { forwardRef, memo, useEffect, useImperativeHandle, useRef } from "react";
import {
  Annotation,
  EditorState,
  StateEffect,
  StateField,
  type Extension,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  placeholder as placeholderExt,
  type DecorationSet,
} from "@codemirror/view";
import { minimalChange as diff, previewMarks, type PreviewMark } from "../lib/pinboard/livePreview";
import { tableCellOffset, unescapeCell } from "../lib/pinboard/noteTables";
import { toggleTaskAt, type TextEdit } from "../lib/pinboard/notesMarkdown";
import { openLink } from "../lib/linkFailure";

/** A transaction that brings the editor in line with `value` from outside, so it isn't echoed back. */
const external = Annotation.define<boolean>();
const setFocused = StateEffect.define<boolean>();

/** Whether the editor has focus: off it, every line renders, with no markers showing. */
const focusField = StateField.define<boolean>({
  create: () => false,
  update: (v, tr) => {
    for (const e of tr.effects) if (e.is(setFocused)) return e.value;
    return v;
  },
});

class GlyphWidget extends WidgetType {
  constructor(readonly glyph: "bullet" | "dash" | "box" | "ticked") {
    super();
  }
  eq(o: GlyphWidget) {
    return o.glyph === this.glyph;
  }
  toDOM(view: EditorView) {
    const el = document.createElement("span");
    el.className = `pm-cm-glyph pm-cm-${this.glyph}`;
    if (this.glyph === "bullet") el.textContent = "•";
    else if (this.glyph === "dash") el.textContent = "–";
    else {
      // A checkbox you can tick without opening the note: the click rewrites its line with the same
      // toggleTaskAt the rendered note always used ("[x]" back to "[]", "- [x]" to "- [ ]"), and
      // leaves the cursor (and focus) where they were.
      el.setAttribute("role", "checkbox");
      el.setAttribute("aria-checked", String(this.glyph === "ticked"));
      el.setAttribute("aria-label", this.glyph === "ticked" ? "Done" : "Not done");
      el.addEventListener("mousedown", (e) => {
        e.preventDefault();
        const line = view.state.doc.lineAt(view.posAtDOM(el));
        const next = toggleTaskAt(line.text, 0);
        if (next != null)
          view.dispatch({ changes: { from: line.from, to: line.to, insert: next } });
      });
    }
    return el;
  }
  ignoreEvent() {
    return true;
  }
}

class TableWidget extends WidgetType {
  constructor(
    readonly rows: string[][],
    readonly index: number,
  ) {
    super();
  }
  eq(o: TableWidget) {
    return o.index === this.index && JSON.stringify(o.rows) === JSON.stringify(this.rows);
  }
  toDOM(view: EditorView) {
    const wrap = document.createElement("div");
    wrap.className = "pm-cm-table";
    const table = document.createElement("table");
    const cols = Math.max(...this.rows.map((r) => r.length));
    this.rows.forEach((cells, r) => {
      const tr = table.insertRow();
      for (let c = 0; c < cols; c++) {
        const cell = r === 0 ? document.createElement("th") : document.createElement("td");
        // Cell text as text — never markup — so what a note holds can't become HTML here.
        cell.textContent = unescapeCell(cells[c] ?? "");
        tr.appendChild(cell);
      }
    });
    // A click on a cell puts the cursor in that cell's source, which turns the table back to text.
    table.addEventListener("mousedown", (e) => {
      const cell = (e.target as Element).closest<HTMLTableCellElement>("td, th");
      const row = cell?.parentElement as HTMLTableRowElement | null;
      if (!cell || !row) return;
      e.preventDefault();
      const at = tableCellOffset(
        view.state.doc.toString(),
        this.index,
        row.rowIndex,
        cell.cellIndex,
      );
      if (at == null) return;
      view.dispatch({ selection: { anchor: at }, scrollIntoView: true });
      view.focus();
    });
    wrap.appendChild(table);
    return wrap;
  }
  ignoreEvent() {
    return true;
  }
}

const hidden = Decoration.replace({});

function decorate(state: EditorState): DecorationSet {
  const doc = state.doc;
  const active = new Set<number>();
  if (state.field(focusField)) {
    for (const r of state.selection.ranges) {
      const a = doc.lineAt(r.from).number - 1;
      const b = doc.lineAt(r.to).number - 1;
      for (let i = a; i <= b; i++) active.add(i);
    }
  }
  // An empty span (two delimiters with nothing between) has nothing to draw, and CodeMirror refuses
  // an empty mark.
  const marks = previewMarks(doc.toString(), active).filter(
    (m) => m.kind === "line" || m.from < m.to,
  );
  const ranges = marks.map((m: PreviewMark) => {
    switch (m.kind) {
      case "line":
        return Decoration.line({ class: `pm-cm-${m.cls}` }).range(doc.line(m.line + 1).from);
      case "hide":
        return hidden.range(m.from, m.to);
      case "glyph":
        return Decoration.replace({ widget: new GlyphWidget(m.glyph) }).range(m.from, m.to);
      case "table":
        return Decoration.replace({ widget: new TableWidget(m.rows, m.index), block: true }).range(
          m.from,
          m.to,
        );
      case "style":
        return Decoration.mark({
          class: `pm-cm-${m.cls}`,
          attributes: m.href ? { "data-href": m.href, title: m.href } : undefined,
        }).range(m.from, m.to);
    }
  });
  return Decoration.set(ranges, true);
}

/** The decorations, recomputed when the text, the cursor or focus changes. A StateField (not a view
 *  plugin) because a table's block widget spans lines, which only a field may provide. */
const preview = StateField.define<DecorationSet>({
  create: (state) => decorate(state),
  update(deco, tr) {
    const focusMoved = tr.effects.some((e) => e.is(setFocused));
    if (tr.docChanged || tr.selection || focusMoved) return decorate(tr.state);
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export interface NoteEditorHandle {
  focus: () => void;
  /** Apply a pure edit at the current selection, as one change, and focus. */
  apply: (make: (value: string, selStart: number, selEnd: number) => TextEdit) => void;
}

interface Props {
  value: string;
  onChange: (next: string) => void;
  onFocus: () => void;
  /** A keypress PM handles itself (formatting, lists, tables): the edit, or null to let it through. */
  keyEdit: (e: KeyboardEvent, value: string, selStart: number, selEnd: number) => TextEdit | null;
  placeholder: string;
}

export const NoteEditor = memo(
  forwardRef<NoteEditorHandle, Props>(function NoteEditor(
    { value, onChange, onFocus, keyEdit, placeholder },
    ref,
  ) {
    const host = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    // The callbacks change identity with the note's text; the editor is built once and reads them here.
    const cb = useRef({ onChange, onFocus, keyEdit });
    cb.current = { onChange, onFocus, keyEdit };

    function apply(view: EditorView, edit: TextEdit) {
      const now = view.state.doc.toString();
      view.dispatch({
        changes: diff(now, edit.text),
        selection: { anchor: edit.selStart, head: edit.selEnd },
        scrollIntoView: true,
      });
    }

    useImperativeHandle(ref, () => ({
      focus: () => viewRef.current?.focus(),
      apply: (make) => {
        const view = viewRef.current;
        if (!view) return;
        const sel = view.state.selection.main;
        apply(view, make(view.state.doc.toString(), sel.from, sel.to));
        view.focus();
      },
    }));

    useEffect(() => {
      const extensions: Extension[] = [
        focusField,
        preview,
        EditorView.lineWrapping,
        placeholderExt(placeholder),
        EditorView.contentAttributes.of({ "aria-label": "Note text", spellcheck: "true" }),
        EditorView.focusChangeEffect.of((_s, focusing) => setFocused.of(focusing)),
        EditorView.updateListener.of((u) => {
          if (u.focusChanged && u.view.hasFocus) cb.current.onFocus();
          if (!u.docChanged) return;
          if (u.transactions.some((t) => t.annotation(external))) return;
          cb.current.onChange(u.state.doc.toString());
        }),
        EditorView.domEventHandlers({
          keydown(e, view) {
            const sel = view.state.selection.main;
            const edit = cb.current.keyEdit(e, view.state.doc.toString(), sel.from, sel.to);
            if (!edit) return false;
            e.preventDefault();
            apply(view, edit);
            return true;
          },
          // A link opens with Ctrl/Cmd+click, or with a plain click while the note isn't being edited
          // (a plain click while typing just puts the cursor there). Web addresses only. One that
          // didn't open says so in the link notice.
          mousedown(e, view) {
            const href = (e.target as Element).closest?.("[data-href]")?.getAttribute("data-href");
            if (!href || !/^https?:\/\//i.test(href)) return false;
            if (!(e.metaKey || e.ctrlKey) && view.hasFocus) return false;
            e.preventDefault();
            openLink(href);
            return true;
          },
        }),
      ];
      const view = new EditorView({
        parent: host.current!,
        state: EditorState.create({ doc: value, extensions }),
      });
      viewRef.current = view;
      return () => {
        view.destroy();
        viewRef.current = null;
      };
      // Built once per mount; `value` changes arrive through the effect below.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // A change from outside (undo, redo, another device's sync) arrives as a new value.
    useEffect(() => {
      const view = viewRef.current;
      if (!view) return;
      const now = view.state.doc.toString();
      if (now === value) return;
      view.dispatch({ changes: diff(now, value), annotations: external.of(true) });
    }, [value]);

    return <div ref={host} className="pm-note-editor min-h-0 flex-1" data-note-editor="" />;
  }),
);
