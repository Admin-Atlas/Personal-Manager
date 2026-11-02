// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The rich (Google Docs-style) note editor's document model (#1030): a ProseMirror schema for the
// note dialect, and the two halves of the bridge — `parseNote` (text → document) and `serializeNote`
// (document → text). The note itself never stops being Markdown text: the editor holds a document
// only while you edit, and every change is written back as text.
//
// The bridge is built to be EXACT, because a note is the only copy of what someone wrote. A note's
// text is a stack of lines, so the document is too: each plain line is a paragraph (a blank line an
// empty one), each list line an item that remembers its own marker as typed (". " "- " "[x] " "3. "
// "ii. "), and each heading, quote line, rule and fence keeps its raw prefix. Only what the editor
// cannot hold differently is normalised: list nesting is two spaces a level (what Tab writes), and a
// table's column padding (re-laid out, as Tab in a text table always did).
//
// `roundTrips` is the guard: a note whose text does not come back unchanged from parse → serialize
// (tables compared after re-layout) is not opened in the rich editor at all; it stays in the
// Markdown editor, so nothing the model can't express is ever rewritten.

import { Schema, type Mark, type Node as PMNode } from "prosemirror-model";
import { tableNodes } from "prosemirror-tables";
import { fenceBlocks } from "./noteFences";
import { inlineSpans, type InlineSpan } from "./livePreview";
import { isRomanToken, matchMarker, nextMarker } from "./notesMarkdown";
import {
  escapeCell,
  findTables,
  layout,
  normalizeTables,
  unescapeCell,
  type Align,
} from "./noteTables";

const tables = tableNodes({
  tableGroup: "block",
  cellContent: "inline*",
  cellAttributes: {},
});

export const noteSchema = new Schema({
  nodes: {
    doc: { content: "block+" },
    paragraph: {
      group: "block",
      content: "inline*",
      // A quote line's own prefix ("> ", ">  "), kept as typed; null outside a quote.
      attrs: { prefix: { default: null } },
      parseDOM: [{ tag: "p" }],
      toDOM: () => ["p", 0],
    },
    heading: {
      group: "block",
      content: "inline*",
      defining: true,
      // `prefix` is the marker as typed ("## "); null once the level changes, so it is rebuilt.
      attrs: { level: { default: 1 }, prefix: { default: null } },
      toDOM: (n) => [`h${n.attrs.level as number}`, 0],
    },
    blockquote: {
      group: "block",
      content: "paragraph+",
      toDOM: () => ["blockquote", 0],
    },
    code_block: {
      group: "block",
      content: "text*",
      marks: "",
      code: true,
      defining: true,
      // The fence lines as typed; `close` is null for a fence left open to the end of the note.
      attrs: { open: { default: "```" }, close: { default: "```" } },
      toDOM: () => ["pre", ["code", 0]],
    },
    horizontal_rule: {
      group: "block",
      attrs: { raw: { default: "---" } },
      toDOM: () => ["hr"],
    },
    list: {
      group: "block",
      content: "list_item+",
      toDOM: () => ["ul", { class: "pm-rich-list" }, 0],
    },
    list_item: {
      content: "paragraph list?",
      defining: true,
      // `token` is the line's marker as typed (". ", "- [x]", "12.") without the space after it;
      // `gap` is that space.
      attrs: { token: { default: "." }, gap: { default: " " } },
      toDOM: (n) => ["li", { "data-marker": markerKind(n.attrs.token as string) }, 0],
    },
    table: { ...tables.table, attrs: { aligns: { default: [] } } },
    table_row: tables.table_row,
    table_cell: tables.table_cell,
    table_header: tables.table_header,
    text: { group: "inline" },
  },
  marks: {
    link: {
      attrs: { href: {} },
      inclusive: false,
      // A data attribute, not `href`: a click inside the editor must never navigate. The editor
      // opens web addresses itself, on Ctrl/Cmd+click.
      toDOM: (m) => [
        "span",
        { class: "pm-rich-link", "data-href": m.attrs.href, title: m.attrs.href },
        0,
      ],
    },
    strong: { attrs: { delim: { default: "**" } }, toDOM: () => ["strong", 0] },
    em: { attrs: { delim: { default: "*" } }, toDOM: () => ["em", 0] },
    strike: { toDOM: () => ["s", 0] },
    code: { attrs: { ticks: { default: "`" } }, excludes: "_", toDOM: () => ["code", 0] },
  },
});

const S = noteSchema;

export type MarkerKind = "bullet" | "dash" | "task" | "ticked" | "number";

/** How a list item's marker is drawn. */
export function markerKind(token: string): MarkerKind {
  const box = /\[([ xX]?)\]$/.exec(token);
  if (box) return box[1].toLowerCase() === "x" ? "ticked" : "task";
  if (token === "." || token === "*") return "bullet";
  if (token === "-" || token === "+") return "dash";
  return "number";
}

/** A checkbox marker ticked or unticked, the way `toggleTaskAt` always did it: the note's own "[x]"
 *  goes back to "[]", a GFM "- [x]" to "- [ ]". */
export function toggleToken(token: string): string {
  const m = /^(.*)\[([ xX]?)\]$/.exec(token);
  if (!m) return token;
  const ticked = m[2].toLowerCase() === "x";
  if (m[1]) return `${m[1]}[${ticked ? " " : "x"}]`;
  return ticked ? "[]" : "[x]";
}

/** The marker the next item after one with `token` starts with: the note's own continuation rule
 *  (a number counts up, a checkbox starts unticked). */
export function nextToken(token: string): string {
  const gfm = /^([-+*])(\s*)\[[ xX]?\]$/.exec(token);
  if (gfm) return `${gfm[1]}${gfm[2]}[ ]`;
  return nextMarker(token)?.trimEnd() ?? token;
}

// --- text → document ---------------------------------------------------------------------------

const HEADING_RE = /^( {0,3})(#{1,6})(\s+)(.*)$/;
const RULE_RE = /^ {0,3}([-*_])( *\1){2,} *$/;
const LIST_TOKENS = /^([.*+-]|\[[ xX]?\]|\d+\.)$/;

/** A line's inline content as text nodes with marks. */
function inlineNodes(text: string): PMNode[] {
  if (!text) return [];
  const spans = inlineSpans(text);
  // Characters that are delimiters (not content) are dropped; every other character carries the
  // marks of each span whose content covers it.
  const isDelim = new Array<boolean>(text.length).fill(false);
  for (const sp of spans) {
    for (let i = sp.from - sp.delim.length; i < sp.from; i++) isDelim[i] = true;
    for (let i = sp.to; i < sp.to + sp.end.length; i++) isDelim[i] = true;
  }
  const markFor = (sp: InlineSpan): Mark => {
    switch (sp.cls) {
      case "link":
        return S.marks.link.create({ href: sp.href });
      case "strong":
        return S.marks.strong.create({ delim: sp.delim });
      case "em":
        return S.marks.em.create({ delim: sp.delim });
      case "strike":
        return S.marks.strike.create();
      case "code":
        return S.marks.code.create({ ticks: sp.delim });
    }
  };
  const nodes: PMNode[] = [];
  let run = "";
  let runKey = "";
  let runMarks: readonly Mark[] = [];
  const flush = () => {
    if (run) nodes.push(S.text(run, runMarks));
    run = "";
  };
  for (let i = 0; i < text.length; i++) {
    if (isDelim[i]) continue;
    const covering = spans.filter((sp) => i >= sp.from && i < sp.to);
    const key = covering.map((sp) => `${sp.cls}:${sp.from}`).join("|");
    if (key !== runKey) {
      flush();
      runKey = key;
      runMarks = covering.reduce<readonly Mark[]>((set, sp) => markFor(sp).addToSet(set), []);
    }
    run += text[i];
  }
  flush();
  return nodes;
}

const para = (text: string, prefix: string | null = null) =>
  S.nodes.paragraph.create({ prefix }, inlineNodes(text));

interface ItemDraft {
  depth: number;
  token: string;
  gap: string;
  text: string;
  children: ItemDraft[];
}

/** A run of list lines as nested lists. A line's depth is its indent in two-space steps; one deeper
 *  than its predecessor nests under it. Anything else (odd indents, a jump of two levels) is laid out
 *  the nearest legal way, which `roundTrips` then refuses — the note stays in the Markdown editor. */
function listNode(lines: ItemDraft[]): PMNode {
  const roots: ItemDraft[] = [];
  const stack: ItemDraft[] = [];
  for (const it of lines) {
    while (stack.length && stack[stack.length - 1].depth >= it.depth) stack.pop();
    const parent = stack[stack.length - 1];
    if (parent) parent.children.push(it);
    else roots.push(it);
    stack.push(it);
  }
  const build = (items: ItemDraft[]): PMNode =>
    S.nodes.list.create(
      null,
      items.map((it) =>
        S.nodes.list_item.create({ token: it.token, gap: it.gap }, [
          para(it.text),
          ...(it.children.length ? [build(it.children)] : []),
        ]),
      ),
    );
  return build(roots);
}

/** A list line's parts, or null when the line isn't a list item. A GFM task ("- [ ] x") keeps its
 *  bullet and brackets together as one marker. */
function listLine(line: string): ItemDraft | null {
  const m = matchMarker(line);
  if (!m || m.token === ">") return null;
  if (!LIST_TOKENS.test(m.token) && !isRomanToken(m.token)) return null;
  const bodyAt = line.length - m.content.length;
  let token = m.token;
  let gap = line.slice(m.indent.length + token.length, bodyAt);
  let text = m.content;
  const gfm = "-+*".includes(m.token) ? /^\[([ xX])\](\s+)/.exec(text) : null;
  if (gfm) {
    token = line.slice(m.indent.length, bodyAt + 3);
    gap = gfm[2];
    text = text.slice(gfm[0].length);
  }
  return { depth: Math.floor(m.indent.length / 2), token, gap, text, children: [] };
}

/** The note's text as a document. */
export function parseNote(text: string): PMNode {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: PMNode[] = [];
  const fences = new Map(fenceBlocks(lines).map((f) => [f.start, f]));
  const tableAt = new Map(findTables(lines).map((t) => [t.start, t]));
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = fences.get(i);
    if (fence) {
      const body = lines.slice(i + 1, fence.closed ? fence.end - 1 : fence.end).join("\n");
      blocks.push(
        S.nodes.code_block.create(
          { open: line, close: fence.closed ? lines[fence.end - 1] : null },
          body ? S.text(body) : [],
        ),
      );
      i = fence.end;
      continue;
    }
    const table = tableAt.get(i);
    if (table) {
      const cols = Math.max(table.aligns.length, ...table.rows.map((r) => r.length));
      blocks.push(
        S.nodes.table.create(
          { aligns: table.aligns },
          table.rows.map((row, r) =>
            S.nodes.table_row.create(
              null,
              Array.from({ length: cols }, (_, c) =>
                (r === 0 ? S.nodes.table_header : S.nodes.table_cell).create(
                  null,
                  inlineNodes(unescapeCell(row[c] ?? "")),
                ),
              ),
            ),
          ),
        ),
      );
      i = table.end;
      continue;
    }
    if (RULE_RE.test(line)) {
      blocks.push(S.nodes.horizontal_rule.create({ raw: line }));
      i++;
      continue;
    }
    const h = HEADING_RE.exec(line);
    if (h) {
      blocks.push(
        S.nodes.heading.create(
          { level: h[2].length, prefix: h[1] + h[2] + h[3] },
          inlineNodes(h[4]),
        ),
      );
      i++;
      continue;
    }
    const quote = matchMarker(line);
    if (quote && quote.token === ">") {
      const quoted: PMNode[] = [];
      while (i < lines.length) {
        const q = matchMarker(lines[i]);
        if (!q || q.token !== ">" || fences.has(i) || tableAt.has(i)) break;
        const at = lines[i].length - q.content.length;
        quoted.push(para(q.content, lines[i].slice(0, at)));
        i++;
      }
      blocks.push(S.nodes.blockquote.create(null, quoted));
      continue;
    }
    if (listLine(line)) {
      const items: ItemDraft[] = [];
      while (i < lines.length && !fences.has(i) && !tableAt.has(i)) {
        const it = listLine(lines[i]);
        if (!it) break;
        items.push(it);
        i++;
      }
      blocks.push(listNode(items));
      continue;
    }
    blocks.push(para(line));
    i++;
  }
  return S.nodes.doc.create(null, blocks.length ? blocks : [para("")]);
}

// --- document → text ---------------------------------------------------------------------------

const openMark = (m: Mark): string => {
  switch (m.type.name) {
    case "link":
      return "[";
    case "strong":
    case "em":
      return m.attrs.delim as string;
    case "strike":
      return "~~";
    case "code":
      return m.attrs.ticks as string;
  }
  return "";
};

const closeMark = (m: Mark): string =>
  m.type.name === "link" ? `](${m.attrs.href as string})` : openMark(m);

// The order a new set of marks opens in, outermost first: a link can hold emphasis, emphasis can't
// hold a link's brackets; code is innermost because nothing formats inside it.
const MARK_ORDER = ["link", "strong", "em", "strike", "code"];
const byOrder = (a: Mark, b: Mark) =>
  MARK_ORDER.indexOf(a.type.name) - MARK_ORDER.indexOf(b.type.name);

/** Whitespace at the edge of bold, italic or struck text is moved outside it: Markdown can't open or
 *  close those spans against a space, so "**a **" would come back as plain text next time. */
function expelSpaces(node: PMNode): { text: string; marks: readonly Mark[] }[] {
  const kids: PMNode[] = [];
  node.forEach((child) => kids.push(child));
  const emphasis = (m: Mark) => m.type.name !== "link" && m.type.name !== "code";
  const out: { text: string; marks: readonly Mark[] }[] = [];
  kids.forEach((child, k) => {
    const text = child.text ?? "";
    // Only a span's real edge matters: a space between two pieces of the same bold run is inside it.
    const opening = child.marks.filter(
      (m) => emphasis(m) && !(k > 0 && m.isInSet(kids[k - 1].marks)),
    );
    const closing = child.marks.filter(
      (m) => emphasis(m) && !(k + 1 < kids.length && m.isInSet(kids[k + 1].marks)),
    );
    const m = /^(\s*)([\s\S]*?)(\s*)$/.exec(text)!;
    const lead = opening.length ? m[1] : "";
    const trail = closing.length ? m[3] : "";
    const body = text.slice(lead.length, text.length - trail.length);
    if (lead) out.push({ text: lead, marks: child.marks.filter((x) => !x.isInSet(opening)) });
    if (body) out.push({ text: body, marks: child.marks });
    if (trail) out.push({ text: trail, marks: child.marks.filter((x) => !x.isInSet(closing)) });
  });
  return out;
}

/** A block's inline content as Markdown. */
export function inlineText(node: PMNode): string {
  let out = "";
  const active: Mark[] = [];
  for (const { text, marks } of expelSpaces(node)) {
    let keep = 0;
    while (keep < active.length && active[keep].isInSet(marks)) keep++;
    while (active.length > keep) out += closeMark(active.pop()!);
    for (const m of [...marks].filter((m) => !m.isInSet(active)).sort(byOrder)) {
      out += openMark(m);
      active.push(m);
    }
    out += text;
  }
  while (active.length) out += closeMark(active.pop()!);
  return out;
}

function listLines(list: PMNode, depth: number, out: string[]) {
  list.forEach((item) => {
    const first = item.firstChild!;
    out.push(
      `${"  ".repeat(depth)}${item.attrs.token as string}${item.attrs.gap as string}${inlineText(first)}`,
    );
    item.forEach((child, _o, idx) => {
      if (idx > 0 && child.type === S.nodes.list) listLines(child, depth + 1, out);
    });
  });
}

/** The document as the note's text. */
export function serializeNote(doc: PMNode): string {
  const out: string[] = [];
  doc.forEach((block) => {
    switch (block.type.name) {
      case "paragraph":
        // A quote prefix only means something inside a quote: a line lifted out of one drops it.
        out.push(inlineText(block));
        break;
      case "heading": {
        const level = block.attrs.level as number;
        const prefix = block.attrs.prefix as string | null;
        out.push(
          (prefix && prefix.trim() === "#".repeat(level) ? prefix : `${"#".repeat(level)} `) +
            inlineText(block),
        );
        break;
      }
      case "blockquote":
        block.forEach((p) => out.push(((p.attrs.prefix as string | null) ?? "> ") + inlineText(p)));
        break;
      case "code_block":
        out.push(block.attrs.open as string);
        if (block.textContent) out.push(...block.textContent.split("\n"));
        if (block.attrs.close != null) out.push(block.attrs.close as string);
        break;
      case "horizontal_rule":
        out.push(block.attrs.raw as string);
        break;
      case "list":
        listLines(block, 0, out);
        break;
      case "table": {
        const rows: string[][] = [];
        block.forEach((row) => {
          const cells: string[] = [];
          row.forEach((cell) => cells.push(escapeCell(inlineText(cell))));
          rows.push(cells);
        });
        out.push(layout(rows, block.attrs.aligns as Align[]).text);
        break;
      }
    }
  });
  return out.join("\n");
}

/** Whether the rich editor can hold this note without changing a character of it (tables compared
 *  after re-layout). The gate on opening a note rich. */
export function roundTrips(text: string): boolean {
  const norm = normalizeTables(text.replace(/\r\n?/g, "\n"));
  return normalizeTables(serializeNote(parseNote(text))) === norm;
}
