// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A pinboard note as a Google Docs-style editor (#1030): no Markdown marker is ever on screen. Bobby,
// after live preview: "something like the table when editing im still in the default text editing
// ... lets add in what the second option was then, the more advanced, google option".
//
// Built on ProseMirror (model/state/view/commands/keymap/inputrules/schema-list/tables; no history
// — the board owns Ctrl+Z). The document model and the exact text bridge live in
// lib/pinboard/richNote.ts; this file is the editor around them: keys, typing shortcuts, the list
// marker views, table editing and the toolbar's commands.
//
// The note is still Markdown text. Every change is serialised back to text and handed up through
// `onChange`; a new `value` from outside (an undo, another device) is parsed in afresh. A note the
// bridge can't hold exactly never reaches this editor (NoteBody checks `roundTrips` first).

import { forwardRef, memo, useEffect, useImperativeHandle, useRef } from "react";
import type { Node as PMNode, NodeType } from "prosemirror-model";
import {
  EditorState,
  Plugin,
  Selection,
  TextSelection,
  type Command,
  type Transaction,
} from "prosemirror-state";
import { Decoration, DecorationSet, EditorView, type NodeView } from "prosemirror-view";
import {
  baseKeymap,
  chainCommands,
  setBlockType,
  toggleMark,
  wrapIn,
  lift,
} from "prosemirror-commands";
import { keymap } from "prosemirror-keymap";
import { InputRule, inputRules } from "prosemirror-inputrules";
import { liftListItem, sinkListItem, splitListItem, wrapInList } from "prosemirror-schema-list";
import {
  addRowAfter,
  goToNextCell,
  isInTable,
  selectedRect,
  tableEditing,
} from "prosemirror-tables";
import {
  markerKind,
  nextToken,
  noteSchema as S,
  parseNote,
  serializeNote,
  toggleToken,
} from "../lib/pinboard/richNote";
import { isRomanToken } from "../lib/pinboard/notesMarkdown";
import { openUrl } from "../lib/ipc";

export type RichFormat = "bold" | "italic" | "heading" | "bullet" | "number" | "checkbox";

export interface RichNoteEditorHandle {
  focus: () => void;
  format: (key: RichFormat) => void;
  insertTable: (cols: number, rows: number) => void;
}

// --- list helpers --------------------------------------------------------------------------------

/** The list item the selection is in, and where it starts, or null. */
function itemAt(state: EditorState): { node: PMNode; pos: number } | null {
  const $from = state.selection.$from;
  for (let d = $from.depth; d > 0; d--) {
    if ($from.node(d).type === S.nodes.list_item)
      return { node: $from.node(d), pos: $from.before(d) };
  }
  return null;
}

/** Set the marker of every list item the selection touches. */
function setTokens(token: (old: string) => string): Command {
  return (state, dispatch) => {
    const { from, to } = state.selection;
    const tr = state.tr;
    let found = false;
    state.doc.nodesBetween(from, to, (node, pos) => {
      if (node.type !== S.nodes.list_item) return true;
      found = true;
      tr.setNodeMarkup(pos, null, { ...node.attrs, token: token(node.attrs.token as string) });
      return true;
    });
    if (!found) return false;
    dispatch?.(tr);
    return true;
  };
}

const KIND_TOKEN: Record<"bullet" | "number" | "checkbox", string> = {
  bullet: ".",
  number: "1.",
  checkbox: "[]",
};

const isKind = (token: string, kind: "bullet" | "number" | "checkbox") => {
  const k = markerKind(token);
  if (kind === "bullet") return k === "bullet" || k === "dash";
  if (kind === "checkbox") return k === "task" || k === "ticked";
  return k === "number";
};

/** The toolbar's list buttons: out of a list when the item already is that kind, the item switched
 *  to it when it is another kind, and the line made a list when it isn't in one. */
function toggleList(kind: "bullet" | "number" | "checkbox"): Command {
  return (state, dispatch, view) => {
    const item = itemAt(state);
    if (item) {
      if (isKind(item.node.attrs.token as string, kind))
        return liftListItem(S.nodes.list_item)(state, dispatch);
      return setTokens(() => KIND_TOKEN[kind])(state, dispatch);
    }
    if (!wrapInList(S.nodes.list)(state)) return false;
    if (dispatch) {
      wrapInList(S.nodes.list)(
        state,
        (tr) => {
          const next = state.apply(tr);
          setTokens(() => KIND_TOKEN[kind])(next, (tr2) => {
            for (const step of tr2.steps) tr.step(step);
          });
          dispatch(tr);
        },
        view,
      );
    }
    return true;
  };
}

/** Enter in a list: an empty item leaves the list (as it does in the text), any other starts the
 *  next item with the marker that follows this one's. */
const enterInList: Command = (state, dispatch) => {
  const item = itemAt(state);
  if (!item || !state.selection.empty) return false;
  const para = state.selection.$from.parent;
  if (para.content.size === 0 && state.selection.$from.node(-1) === item.node) {
    return liftListItem(S.nodes.list_item)(state, dispatch);
  }
  const token = item.node.attrs.token as string;
  return splitListItem(S.nodes.list_item, { token: nextToken(token), gap: item.node.attrs.gap })(
    state,
    dispatch,
  );
};

/** Backspace at the very start of a list item takes it out of the list, rather than joining it to
 *  the item above. */
const backspaceInList: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parentOffset > 0 || !itemAt(state)) return false;
  if ($from.node(-1).type !== S.nodes.list_item || $from.index(-1) !== 0) return false;
  return liftListItem(S.nodes.list_item)(state, dispatch);
};

// --- tables --------------------------------------------------------------------------------------

/** Tab in a table: the next cell, and a new row from the last one — as Tab did in the text. */
const tabInTable: Command = (state, dispatch) => {
  if (!isInTable(state)) return false;
  if (goToNextCell(1)(state, dispatch)) return true;
  if (!dispatch) return true;
  addRowAfter(state, (tr) => {
    const next = state.apply(tr);
    goToNextCell(1)(next, (tr2) => {
      for (const step of tr2.steps) tr.step(step);
      tr.setSelection(tr2.selection.map(tr.doc, tr.mapping.slice(tr.steps.length)));
    });
    dispatch(tr);
  });
  return true;
};

/** Enter in a table adds a row under the cursor's and moves down into it, same column. */
const enterInTable: Command = (state, dispatch) => {
  if (!isInTable(state)) return false;
  if (!dispatch) return true;
  const rect = selectedRect(state);
  addRowAfter(state, (tr) => {
    const table = tr.doc.nodeAt(rect.tableStart - 1)!;
    let pos = rect.tableStart;
    for (let r = 0; r < rect.bottom; r++) pos += table.child(r).nodeSize;
    const row = tr.doc.nodeAt(pos)!;
    let cellPos = pos + 1;
    for (let c = 0; c < rect.left; c++) cellPos += row.child(c).nodeSize;
    tr.setSelection(TextSelection.create(tr.doc, cellPos + 1));
    dispatch(tr.scrollIntoView());
  });
  return true;
};

function createTable(cols: number, rows: number): PMNode {
  const row = (header: boolean) =>
    S.nodes.table_row.create(
      null,
      Array.from({ length: cols }, () =>
        (header ? S.nodes.table_header : S.nodes.table_cell).create(),
      ),
    );
  return S.nodes.table.create({ aligns: [] }, [
    row(true),
    ...Array.from({ length: Math.max(0, rows - 1) }, () => row(false)),
  ]);
}

// --- typing shortcuts ----------------------------------------------------------------------------

/** Typing a marker at the start of a line makes it that block — the same markers the note has always
 *  understood, so muscle memory carries over. In a list item, it switches that item's marker. */
function markerRule(re: RegExp, token: (m: RegExpMatchArray) => string): InputRule {
  return new InputRule(re, (state, match, start, end) => {
    const $start = state.doc.resolve(start);
    if ($start.parent.type !== S.nodes.paragraph || $start.parentOffset !== 0) return null;
    const tr = state.tr.delete(start, end);
    const item = itemAt(state);
    if (item) {
      tr.setNodeMarkup(item.pos, null, { ...item.node.attrs, token: token(match) });
      return tr;
    }
    const next = state.apply(tr);
    let out: Transaction | null = null;
    wrapInList(S.nodes.list)(next, (wrap) => {
      for (const step of wrap.steps) tr.step(step);
      const after = state.apply(tr);
      setTokens(() => token(match))(after, (set) => {
        for (const step of set.steps) tr.step(step);
      });
      out = tr;
    });
    return out;
  });
}

function blockRule(re: RegExp, type: NodeType, attrs: (m: RegExpMatchArray) => object): InputRule {
  return new InputRule(re, (state, match, start, end) => {
    const $start = state.doc.resolve(start);
    if ($start.parent.type !== S.nodes.paragraph || itemAt(state)) return null;
    const tr = state.tr.delete(start, end);
    tr.setBlockType(start, start, type, attrs(match));
    return tr;
  });
}

function markRule(re: RegExp, mark: () => ReturnType<typeof S.marks.strong.create>): InputRule {
  return new InputRule(re, (state, match, start, end) => {
    const inner = match[1];
    const at = start + match[0].indexOf(inner);
    const tr = state.tr;
    tr.addMark(at, at + inner.length, mark());
    tr.delete(at + inner.length, end);
    tr.delete(start, at);
    tr.removeStoredMark(mark().type);
    return tr;
  });
}

const rules = inputRules({
  rules: [
    markerRule(/^([.*])\s$/, (m) => m[1]),
    markerRule(/^([-+])\s$/, (m) => m[1]),
    markerRule(/^(\d+\.)\s$/, (m) => m[1]),
    markerRule(/^(\[[ xX]?\])\s$/, (m) => m[1]),
    markerRule(/^([ivxlcdm]+\.)\s$/i, (m) => (isRomanToken(m[1]) ? m[1] : "1.")),
    blockRule(/^(#{1,6})\s$/, S.nodes.heading, (m) => ({ level: m[1].length, prefix: null })),
    new InputRule(/^>\s$/, (state, _m, start, end) => {
      if (itemAt(state)) return null;
      const tr = state.tr.delete(start, end);
      let out: Transaction | null = null;
      wrapIn(S.nodes.blockquote)(state.apply(tr), (w) => {
        for (const step of w.steps) tr.step(step);
        out = tr;
      });
      return out;
    }),
    new InputRule(/^([-*_])\1\1$/, (state, _m, start, end) => {
      const $start = state.doc.resolve(start);
      if ($start.parent.type !== S.nodes.paragraph || itemAt(state)) return null;
      return state.tr.replaceRangeWith(
        $start.before(),
        end,
        S.nodes.horizontal_rule.create({ raw: "---" }),
      );
    }),
    markRule(/\*\*([^*\s](?:[^*]*[^*\s])?)\*\*$/, () => S.marks.strong.create()),
    markRule(/(?<![*\w])\*([^*\s](?:[^*]*[^*\s])?)\*$/, () => S.marks.em.create()),
    markRule(/~~([^~\s](?:[^~]*[^~\s])?)~~$/, () => S.marks.strike.create()),
    markRule(/`([^`]+)`$/, () => S.marks.code.create()),
  ],
});

// --- the list item's drawn marker ----------------------------------------------------------------

/** A list item drawn as its marker (a bullet, a dash, a number, a checkbox you can tick) beside its
 *  text. The marker is not editable; the token behind it is what the note's text keeps. */
class ItemView implements NodeView {
  dom: HTMLElement;
  contentDOM: HTMLElement;
  private marker: HTMLElement;
  constructor(
    private node: PMNode,
    private view: EditorView,
    private getPos: () => number | undefined,
  ) {
    this.dom = document.createElement("li");
    this.marker = document.createElement("span");
    this.marker.contentEditable = "false";
    this.marker.addEventListener("mousedown", (e) => {
      const kind = markerKind(this.node.attrs.token as string);
      if (kind !== "task" && kind !== "ticked") return;
      e.preventDefault();
      const pos = this.getPos();
      if (pos == null) return;
      this.view.dispatch(
        this.view.state.tr.setNodeMarkup(pos, null, {
          ...this.node.attrs,
          token: toggleToken(this.node.attrs.token as string),
        }),
      );
    });
    this.contentDOM = document.createElement("div");
    this.contentDOM.className = "pm-rich-item";
    this.dom.append(this.marker, this.contentDOM);
    this.draw();
  }
  private draw() {
    const token = this.node.attrs.token as string;
    const kind = markerKind(token);
    this.dom.dataset.marker = kind;
    this.marker.className = `pm-rich-marker pm-rich-${kind}`;
    this.marker.textContent =
      kind === "bullet" ? "•" : kind === "dash" ? "–" : kind === "number" ? token : "";
    if (kind === "task" || kind === "ticked") {
      this.marker.setAttribute("role", "checkbox");
      this.marker.setAttribute("aria-checked", String(kind === "ticked"));
      this.marker.setAttribute("aria-label", kind === "ticked" ? "Done" : "Not done");
    } else {
      this.marker.removeAttribute("role");
      this.marker.removeAttribute("aria-checked");
      this.marker.removeAttribute("aria-label");
    }
  }
  update(node: PMNode) {
    if (node.type !== this.node.type) return false;
    this.node = node;
    this.draw();
    return true;
  }
  stopEvent(e: Event) {
    return this.marker.contains(e.target as Node);
  }
  ignoreMutation(m: MutationRecord | { type: "selection"; target: Node }) {
    return this.marker.contains(m.target);
  }
}

/** The empty note's prompt, drawn as a decoration so it is never part of the text. */
function placeholder(text: string) {
  return new Plugin({
    props: {
      decorations(state) {
        const doc = state.doc;
        if (doc.childCount !== 1 || doc.firstChild!.content.size > 0) return null;
        if (doc.firstChild!.type !== S.nodes.paragraph) return null;
        return DecorationSet.create(doc, [
          Decoration.node(0, doc.firstChild!.nodeSize, {
            class: "pm-rich-empty",
            "data-placeholder": text,
          }),
        ]);
      },
    },
  });
}

// --- the component -------------------------------------------------------------------------------

interface Props {
  value: string;
  onChange: (next: string) => void;
  onFocus: () => void;
  placeholder: string;
}

function headingToggle(): Command {
  return (state, dispatch) => {
    const parent = state.selection.$from.parent;
    if (parent.type === S.nodes.heading) return setBlockType(S.nodes.paragraph)(state, dispatch);
    return setBlockType(S.nodes.heading, { level: 1, prefix: null })(state, dispatch);
  };
}

const FORMAT: Record<RichFormat, Command> = {
  bold: toggleMark(S.marks.strong),
  italic: toggleMark(S.marks.em),
  heading: headingToggle(),
  bullet: toggleList("bullet"),
  number: toggleList("number"),
  checkbox: toggleList("checkbox"),
};

const keys = keymap({
  "Mod-b": FORMAT.bold,
  "Mod-i": FORMAT.italic,
  "Mod-Shift-h": FORMAT.heading,
  "Mod-Shift-8": FORMAT.bullet,
  "Mod-Shift-7": FORMAT.number,
  "Mod-Shift-9": FORMAT.checkbox,
  Enter: chainCommands(enterInTable, enterInList),
  // A line break inside a list item would split the note's line; Shift+Enter leaves the list
  // instead, the way an unmarked line under a list item always did.
  "Shift-Enter": chainCommands(
    (state, dispatch) => (itemAt(state) ? liftListItem(S.nodes.list_item)(state, dispatch) : false),
    baseKeymap.Enter,
  ),
  Tab: chainCommands(tabInTable, sinkListItem(S.nodes.list_item), () => true),
  "Shift-Tab": chainCommands(goToNextCell(-1), liftListItem(S.nodes.list_item), lift, () => true),
  Backspace: backspaceInList,
});

export const RichNoteEditor = memo(
  forwardRef<RichNoteEditorHandle, Props>(function RichNoteEditor(
    { value, onChange, onFocus, placeholder: prompt },
    ref,
  ) {
    const host = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    // The text this editor last handed up, so its own change coming back as `value` isn't re-parsed.
    const emitted = useRef(value);
    const cb = useRef({ onChange, onFocus });
    cb.current = { onChange, onFocus };

    useImperativeHandle(ref, () => ({
      focus: () => viewRef.current?.focus(),
      format: (key) => {
        const view = viewRef.current;
        if (!view) return;
        FORMAT[key](view.state, view.dispatch, view);
        view.focus();
      },
      insertTable: (cols, rows) => {
        const view = viewRef.current;
        if (!view) return;
        const { state } = view;
        // After the block the cursor is in (never inside a list or a cell), cursor in the first cell.
        const $from = state.selection.$from;
        const at = $from.depth > 0 ? $from.after(1) : state.doc.content.size;
        const table = createTable(cols, rows);
        const tr = state.tr.insert(at, table);
        tr.setSelection(TextSelection.create(tr.doc, at + 3));
        view.dispatch(tr.scrollIntoView());
        view.focus();
      },
    }));

    useEffect(() => {
      const state = EditorState.create({
        doc: parseNote(value),
        plugins: [
          rules,
          keys,
          keymap(baseKeymap),
          tableEditing(),
          placeholder(prompt),
          new Plugin({
            props: {
              nodeViews: {
                list_item: (node, view, getPos) => new ItemView(node, view, getPos),
              },
              handleDOMEvents: {
                focus: () => {
                  cb.current.onFocus();
                  return false;
                },
                // A link opens with Ctrl/Cmd+click, or a plain click while the note isn't being
                // edited. Web addresses only.
                mousedown: (view, e) => {
                  const href = (e.target as Element)
                    .closest?.("[data-href]")
                    ?.getAttribute("data-href");
                  if (!href || !/^https?:\/\//i.test(href)) return false;
                  if (!(e.metaKey || e.ctrlKey) && view.hasFocus()) return false;
                  e.preventDefault();
                  void openUrl(href).catch(() => {});
                  return true;
                },
              },
              attributes: { "aria-label": "Note text", spellcheck: "true" },
            },
          }),
        ],
      });
      const view = new EditorView(host.current!, {
        state,
        dispatchTransaction(tr) {
          const next = view.state.apply(tr);
          view.updateState(next);
          if (!tr.docChanged) return;
          const text = serializeNote(next.doc);
          if (text === emitted.current) return;
          emitted.current = text;
          cb.current.onChange(text);
        },
      });
      viewRef.current = view;
      return () => {
        view.destroy();
        viewRef.current = null;
      };
      // Built once per mount; `value` changes arrive through the effect below.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // A change from outside (undo, redo, another device) is parsed in afresh. The cursor stays at the
    // same place in the document, or as near as the new one allows.
    useEffect(() => {
      const view = viewRef.current;
      if (!view || value === emitted.current) return;
      emitted.current = value;
      const doc = parseNote(value);
      const at = Math.min(view.state.selection.from, doc.content.size);
      const next = EditorState.create({ doc, plugins: view.state.plugins });
      view.updateState(next.apply(next.tr.setSelection(Selection.near(next.doc.resolve(at)))));
    }, [value]);

    return <div ref={host} className="pm-rich-editor min-h-0 flex-1" />;
  }),
);
