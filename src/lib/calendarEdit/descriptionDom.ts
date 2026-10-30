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
  keptLink,
  normalise,
  type DescDoc,
  type DescNode,
  type Mark,
} from "./descriptionHtml";
import { BLOCKS, HIDDEN, asciiLower } from "./descriptionTokens";

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
  el.append(...children);
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
 *  Then `normalise`, which makes it a document PM's own reader accepts as it is. */
export function nodesToDoc(root: Node): DescDoc {
  const doc: DescNode[] = [];
  readInto(root, doc, 1);
  return normalise(doc);
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
function readInto(parent: Node, out: DescNode[], depth: number): void {
  for (let child = parent.firstChild; child !== null; child = child.nextSibling) {
    readNode(child, out, depth);
  }
}

/** `Node.ELEMENT_NODE` and `Node.TEXT_NODE`, as numbers: the box may belong to another window. */
const ELEMENT = 1;
const TEXT = 3;

function readNode(node: Node, out: DescNode[], depth: number): void {
  if (node.nodeType === TEXT) {
    out.push({ kind: "text", text: (node as Text).data });
    return;
  }
  // Comments and anything else that isn't an element show nothing.
  if (node.nodeType !== ELEMENT) return;
  const el = node as Element;
  const name = asciiLower(el.localName);
  if (name === "br") {
    out.push({ kind: "br" });
    return;
  }
  if (HIDDEN.has(name)) return;
  if (depth > MAX_DEPTH) {
    readLeaves(el, out);
    return;
  }
  // A line of its own, as browsers draw a block that isn't one of the model's: its edges break the
  // line (§2.3), its words kept.
  if (BLOCKS.has(name) && !MODEL_BLOCKS.has(name)) {
    edge(out);
    readInto(el, out, depth + 1);
    edge(out);
    return;
  }
  // What it draws itself, outermost first, as the reader opens them: a block, a link, then marks.
  const block = MODEL_BLOCKS.get(name);
  const href = name === "a" ? keptLink(el.getAttribute("href") ?? "") : null;
  const marks: Mark[] = [];
  const mark = MARK_NAMES.get(name);
  if (mark !== undefined) marks.push(mark);
  for (const m of styleMarks(el)) if (!marks.includes(m)) marks.push(m);

  const children: DescNode[] = [];
  readInto(el, children, depth + 1);
  let made: DescNode[] = children;
  for (const m of [...marks].reverse()) made = [{ kind: m, children: made }];
  if (href !== null) made = [{ kind: "a", href, children: made }];
  if (block !== undefined) made = [{ kind: block, children: made }];
  out.push(...made);
}

/** A line's edge: a break, unless the line is empty or already ends at a break or a block. */
function edge(out: DescNode[]): void {
  const last = out[out.length - 1];
  if (last === undefined || last.kind === "br" || MODEL_BLOCKS.has(last.kind)) return;
  out.push({ kind: "br" });
}

/** The marks an element's style draws: what WebKit leaves on a span (ApplyStyleCommand), or Chromium
 *  on a merge. Read from the style the engine parsed, never the attribute's text. */
function styleMarks(el: Element): Mark[] {
  const style = (el as Partial<HTMLElement>).style;
  if (style === undefined) return [];
  const marks: Mark[] = [];
  // What shows bold, as the description reader draws a weight.
  const weight = asciiLower(style.fontWeight ?? "");
  const number = /^\d{1,4}$/.test(weight) ? Number(weight) : null;
  if (weight === "bold" || weight === "bolder" || (number !== null && number >= 600)) {
    marks.push("b");
  }
  const slant = asciiLower(style.fontStyle ?? "");
  if (slant === "italic" || slant === "oblique") marks.push("i");
  // The longhand where the engine has it, else the shorthand, which may name a style or colour too.
  const line = asciiLower(style.textDecorationLine || style.textDecoration || "");
  if (line.split(/\s+/).includes("underline")) marks.push("u");
  return marks;
}

/** The words a range of the box holds, as the link panel's Text field shows them: each text node's
 *  `data`, a break as a space, what never shows left out. Read from a copy of the range's content,
 *  never `Selection.toString()` or `textContent`, which CodeQL reads as page text an attacker shaped. */
export function rangeWords(range: Range): string {
  const out: DescNode[] = [];
  const copy = range.cloneContents();
  for (let child = copy.firstChild; child !== null; child = child.nextSibling) {
    readLeaves(child, out);
  }
  return out.map((node) => (node.kind === "text" ? node.text : " ")).join("");
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
