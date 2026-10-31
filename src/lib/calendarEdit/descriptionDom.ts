// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description editor's box and the document model, both ways (#884, C19 §6). The editor's box is
// a contentEditable element whose children React doesn't own: it is seeded from a document made into
// nodes one by one, and what the user typed is read back as a document, whatever the engine made of
// it, so a save writes something PM can read back and edit.
//
// Nothing here parses HTML or writes an HTML string into the page (INVARIANTS.md I-23): elements are
// made by literal name, text goes in with `append`, and an address goes in with `setAttribute` only
// after the link rule took it. Reading uses `Text.data`, `localName` and `getAttribute("href")` only:
// never `textContent`, `innerText` or `outerHTML`, which CodeQL reads as page text an attacker may
// have shaped.

import {
  MAX_DEPTH,
  fromLoose,
  keptLink,
  normalise,
  shows,
  type DescDoc,
  type DescNode,
  type Loose,
  type Mark,
} from "./descriptionHtml";
import { BLOCKS, HIDDEN, asciiLower, isSpace } from "./descriptionTokens";

/** The document as DOM nodes for the editor's box, made one by one: elements by literal name, text
 *  as strings for `append` to make text nodes, a link's address with `setAttribute` only after the
 *  link rule. No `target` on links: `useExternalLinks` must not open a link the user is editing.
 *  Nothing deeper than MAX_DEPTH is made: its words and breaks come instead. */
export function docToNodes(doc: DescDoc, owner: Document): (Node | string)[] {
  return doc.flatMap((node) => nodeOf(node, owner, 1));
}

function nodeOf(node: DescNode, owner: Document, depth: number): (Node | string)[] {
  if (node.kind === "text") return [node.text];
  if (node.kind === "br") return [owner.createElement("br")];
  if (depth > MAX_DEPTH) return leavesOf(node, owner);
  const children = node.children.flatMap((child) => nodeOf(child, owner, depth + 1));
  // One literal name per arm: CodeQL reads createElement's argument as a place to inject markup.
  let el: HTMLElement;
  switch (node.kind) {
    case "b":
      el = owner.createElement("b");
      break;
    case "i":
      el = owner.createElement("i");
      break;
    case "u":
      el = owner.createElement("u");
      break;
    case "p":
      el = owner.createElement("p");
      break;
    case "ul":
      el = owner.createElement("ul");
      break;
    case "ol":
      el = owner.createElement("ol");
      break;
    case "li":
      el = owner.createElement("li");
      break;
    case "a": {
      // The model holds only addresses the link rule keeps; asked again, so a document built any
      // other way never puts an address the rule refuses into the page. A phone link is kept: the
      // editor's click guard stops it opening (C19 critique C2-H5).
      const href = keptLink(node.href);
      if (href === null) return children;
      el = owner.createElement("a");
      el.setAttribute("href", href);
      break;
    }
  }
  // One by one: spread as arguments, a huge description's list would pass the engine's limit.
  for (const child of children) el.append(child);
  return [el];
}

/** A subtree's words and breaks, in order, without recursing (it may be any depth). */
function leavesOf(node: DescNode, owner: Document): (Node | string)[] {
  const out: (Node | string)[] = [];
  const todo: DescNode[] = [node];
  while (todo.length > 0) {
    const next = todo.pop() as DescNode;
    if (next.kind === "text") out.push(next.text);
    else if (next.kind === "br") out.push(owner.createElement("br"));
    else for (let k = next.children.length - 1; k >= 0; k--) todo.push(next.children[k]);
  }
  return out;
}

/** What the editor's box holds, as a document: never fails, whatever the engine made of the user's
 *  typing. Text is read with `Text.data`, links with `getAttribute("href")`. Bold, italic and
 *  underline come from their elements (`strong` and `em` too) or from a style the engine left on a
 *  span; a `div` is a line; a link the rule refuses, a span, `font` and anything unknown are their
 *  words (an image has none); what never shows (a script, a style) is dropped with its content.
 *  A style that takes bold or italic off (Chromium's `font-weight: normal` inside bold) takes it off
 *  the words it holds, as they show. Then `normalise`, which makes it a document PM's own reader
 *  accepts as it is. */
export function nodesToDoc(root: Node): DescDoc {
  const doc: Loose[] = [];
  const reading: Reading = { ended: true, plain: false };
  readInto(root, doc, 1, reading);
  return normalise(fromLoose(doc, reading.plain));
}

/** Where the reading is, in the order things show. */
interface Reading {
  /** The line has ended, or nothing has shown yet: the last thing that shows was a break or a
   *  block's edge, through any formatting around it. Whitespace leaves it as it was. */
  ended: boolean;
  /** A `plain` node was made: the marks need redrawing around it. */
  plain: boolean;
}

const MODEL_BLOCKS = new Map<string, "p" | "ul" | "ol" | "li">([
  ["p", "p"],
  ["ul", "ul"],
  ["ol", "ol"],
  ["li", "li"],
]);
const MARK_NAMES = new Map<string, Mark>([
  ["b", "b"],
  ["strong", "b"],
  ["i", "i"],
  ["em", "i"],
  ["u", "u"],
]);

/** `parent`'s children, read into `out`. */
function readInto(parent: Node, out: Loose[], depth: number, reading: Reading): void {
  for (let child = parent.firstChild; child !== null; child = child.nextSibling) {
    readNode(child, out, depth, reading);
  }
}

/** `Node.ELEMENT_NODE` and `Node.TEXT_NODE`, as numbers: the box may belong to another window. */
const ELEMENT = 1;
const TEXT = 3;

function readNode(node: Node, out: Loose[], depth: number, reading: Reading): void {
  if (node.nodeType === TEXT) {
    const text = (node as Text).data;
    out.push({ kind: "text", text });
    if (shows(text)) reading.ended = false;
    return;
  }
  // Comments and anything else that isn't an element show nothing.
  if (node.nodeType !== ELEMENT) return;
  const el = node as Element;
  const name = asciiLower(el.localName);
  if (name === "br") {
    out.push({ kind: "br" });
    reading.ended = true;
    return;
  }
  if (HIDDEN.has(name)) return;
  if (depth > MAX_DEPTH) {
    const leaves: DescNode[] = [];
    readLeaves(el, leaves);
    for (const leaf of leaves) {
      if (leaf.kind === "text") {
        out.push({ kind: "text", text: leaf.text });
        if (shows(leaf.text)) reading.ended = false;
      } else {
        out.push({ kind: "br" });
        reading.ended = true;
      }
    }
    return;
  }
  // A line of its own, as browsers draw a block that isn't one of the model's: its edges break the
  // line (§2.3), its words kept.
  if (BLOCKS.has(name) && !MODEL_BLOCKS.has(name)) {
    edge(out, reading);
    readInto(el, out, depth + 1, reading);
    edge(out, reading);
    return;
  }
  // What it draws itself, outermost first, as the reader opens them: a block, a link, what its style
  // takes off, then marks. A mark its style takes off isn't drawn (`<b style="font-weight: normal">`).
  const block = MODEL_BLOCKS.get(name);
  const href = name === "a" ? keptLink(el.getAttribute("href") ?? "") : null;
  const style = styleOf(el);
  const marks: Mark[] = [];
  const mark = MARK_NAMES.get(name);
  for (const m of [mark, ...style.marks]) {
    if (m !== undefined && !marks.includes(m) && !style.off.includes(m)) marks.push(m);
  }

  if (block !== undefined) reading.ended = true; // a paragraph, list or item starts a line
  const children: Loose[] = [];
  readInto(el, children, depth + 1, reading);
  if (block !== undefined) reading.ended = true; // and ends its last one
  let made: Loose[] = children;
  for (const m of [...marks].reverse()) made = [{ kind: m, children: made }];
  if (style.off.length > 0) {
    reading.plain = true;
    made = [{ kind: "plain", marks: style.off, children: made }];
  }
  if (href !== null) made = [{ kind: "a", href, children: made }];
  if (block !== undefined) made = [{ kind: block, children: made }];
  // One by one: spread as arguments, a huge box's list would pass the engine's limit.
  for (const piece of made) out.push(piece);
}

/** A line's edge: a break, unless the line is empty or has already ended, however deep in formatting
 *  its last break is (`<b>a</b><div><b><br></b></div>`, Bold, Enter, Enter, in both engines), and
 *  whatever whitespace follows it. */
function edge(out: Loose[], reading: Reading): void {
  if (!reading.ended) out.push({ kind: "br" });
  reading.ended = true;
}

/** The marks an element's style draws, and the bold or italic it takes off: what WebKit leaves on a
 *  span (ApplyStyleCommand), or Chromium on a merge or inside bold made into a list. Read from the
 *  style the engine parsed, never the attribute's text, as the description reader reads a style. */
function styleOf(el: Element): { marks: Mark[]; off: Mark[] } {
  const style = (el as Partial<HTMLElement>).style;
  if (style === undefined) return { marks: [], off: [] };
  const marks: Mark[] = [];
  const off: Mark[] = [];
  // What shows bold, as the description reader draws a weight.
  const weight = asciiLower(style.fontWeight ?? "");
  const number = /^\d{1,4}$/.test(weight) ? Number(weight) : null;
  if (weight === "bold" || weight === "bolder" || (number !== null && number >= 600)) {
    marks.push("b");
  } else if (weight === "normal" || weight === "lighter" || (number !== null && number > 0)) {
    off.push("b");
  }
  const slant = asciiLower(style.fontStyle ?? "");
  if (slant === "italic" || slant === "oblique") marks.push("i");
  else if (slant === "normal") off.push("i");
  // The longhand where the engine has it, else the shorthand, which may name a style or colour too.
  const line = asciiLower(style.textDecorationLine || style.textDecoration || "");
  if (line.split(/\s+/).includes("underline")) marks.push("u");
  return { marks, off };
}

/** The words a range of the box holds, as the link panel's Text field shows them: each text node's
 *  `data`, one space where a line ends between words (a break, or a block's edge: list items,
 *  paragraphs and the engine's `div` lines), what never shows left out. Read from a copy of the
 *  range's content, never `Selection.toString()` or `textContent`, which CodeQL reads as page text an
 *  attacker shaped. */
export function rangeWords(range: Range): string {
  const copy = range.cloneContents();
  // In pieces joined once: asking a string built by `+=` how it ends copies it whole each time.
  const words: string[] = [];
  let last = "";
  // A line ended since the last words: a space is owed before the next.
  let gap = false;
  // Nodes still to read, last first; `null` is a block's end.
  const todo: (Node | null)[] = [];
  for (let child = copy.lastChild; child !== null; child = child.previousSibling) todo.push(child);
  while (todo.length > 0) {
    const next = todo.pop() as Node | null;
    if (next === null) {
      gap = true;
      continue;
    }
    if (next.nodeType === TEXT) {
      const text = (next as Text).data;
      if (text === "") continue;
      if (gap && last !== "" && !isSpace(last) && !isSpace(text[0])) words.push(" ");
      gap = false;
      words.push(text);
      last = text[text.length - 1];
      continue;
    }
    if (next.nodeType !== ELEMENT) continue;
    const name = asciiLower((next as Element).localName);
    if (name === "br") {
      gap = true;
      continue;
    }
    if (HIDDEN.has(name)) continue;
    if (BLOCKS.has(name)) {
      gap = true;
      todo.push(null);
    }
    for (let child = next.lastChild; child !== null; child = child.previousSibling) {
      todo.push(child);
    }
  }
  return words.join("");
}

/** An element's words and breaks, without recursing: what survives of anything too deep. */
function readLeaves(root: ChildNode, out: DescNode[]): void {
  const todo: Node[] = [root];
  while (todo.length > 0) {
    const next = todo.pop() as Node;
    if (next.nodeType === TEXT) {
      out.push({ kind: "text", text: (next as Text).data });
      continue;
    }
    if (next.nodeType !== ELEMENT) continue;
    const name = asciiLower((next as Element).localName);
    if (name === "br") {
      out.push({ kind: "br" });
      continue;
    }
    if (HIDDEN.has(name)) continue;
    for (let child = next.lastChild; child !== null; child = child.previousSibling) {
      todo.push(child);
    }
  }
}
