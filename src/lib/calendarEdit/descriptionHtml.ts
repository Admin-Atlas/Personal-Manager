// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Google event descriptions as PM reads and writes them (#884, C19). Google's `description` "can
// contain HTML": its own editor writes bold, italic, underline, lists and links. This is the document
// model of that formatting; one reader that says both what shows and whether PM can write it back
// with nothing Google shows lost; the writer; the comparison that tells an unchanged description
// from a changed one; the plain text "Replace with plain text…" leaves; and, for drawing only, the
// addresses written in a description's words.
//
// PM's second sanitising boundary, beside markdown.tsx's, for Google event descriptions only. Nothing
// here parses HTML with the browser: descriptionTokens.ts reads the characters, and the document is
// drawn as React elements or DOM nodes made one by one, never as an HTML string put into the page.
// An allow-list of Google's own output decides what PM writes back; anything outside it fails closed:
// its words still show, and the description stays as Google holds it unless the user replaces it.
// A link carries only an absolute web or email address (`linkTarget`), or a phone number.
//
// Every walk is bounded by MAX_DEPTH and every reader is one pass, so a hostile description costs
// linear time.

import { safeUrl } from "../markdown";
import {
  BLOCKS,
  HIDDEN,
  VOID,
  asciiLower,
  isAlpha,
  isDigit,
  isHex,
  isSpace,
  markdownDropsALine,
  tokens,
  type Attr,
  type Token,
} from "./descriptionTokens";

export type Mark = "b" | "i" | "u";
export type Block = "p" | "ul" | "ol" | "li";
export type DescNode =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "br" }
  | { readonly kind: Mark | Block; readonly children: DescDoc }
  /** `href` always passes `keptLink`: a link to anything else is never a node. */
  | { readonly kind: "a"; readonly href: string; readonly children: DescDoc };
export type DescDoc = readonly DescNode[];

export interface ParsedDescription {
  /** What shows. Allowed elements kept; any other element unwrapped (its text kept) or, if it never
   *  shows, dropped with its content. Always a document a browser would draw the same way. */
  readonly doc: DescDoc;
  /** Whether PM can write `doc` back with nothing Google shows lost: nothing was unwrapped, dropped or
   *  rearranged on the way. */
  readonly supported: boolean;
  /** Formatted (HTML) rather than plain lines; `looksLikeHtml`. */
  readonly html: boolean;
  /** It holds an element (any but a name with a ":", as Outlook's `<o:p>`) or a character reference:
   *  formatting to draw as such. An address in angle brackets alone isn't: Outlook and Teams write
   *  links that way into plain text, which draws as before (the event pop-up). So too any line Markdown
   *  would drop (`markdownDropsALine`), plain lines included: `doc` holds its words. */
  readonly markup: boolean;
}

/** The tags PM reads and writes; the fixture's `tags`, and Rust's `EDITABLE_TAGS`. */
export const EDITABLE_TAGS: readonly string[] = [
  "a",
  "b",
  "br",
  "div",
  "em",
  "html-blob",
  "i",
  "li",
  "ol",
  "p",
  "span",
  "strong",
  "u",
  "ul",
];

/** Deeper than this is never a description someone wrote; it bounds every walk below. */
export const MAX_DEPTH = 32;

/** Whether an element makes a description `markup`: any element but one whose name holds a ":"
 *  (Outlook's `<o:p>`). Markdown drops a block of HTML it doesn't know whole (`<center>`,
 *  `<html><body>…`), words and all, so only the formatted view shows such a description's words. An
 *  address in angle brackets is never an element (`literal`), so Outlook's and Teams' plain text keeps
 *  the Markdown path, and nor are other words Markdown shows (`<www.example.com>`, `<Room 4>`,
 *  `<José>`, `<Room A, B>`). A word Markdown reads as a tag and drops (`<Room>`) is dropped here too.
 *  The one way a word goes with the flip is a name the reader knows with something odd after it
 *  (`<Table 4>`), which Markdown shows and a browser draws as that element (`readsAsWords`). Words
 *  Markdown drops with their line (`<Option 2>` opening one) flip without a tag: `markdownDropsALine`. */
const isMarkup = (name: string) => !name.includes(":");

// --- telling formatting from plain text ---

/** Whether a description is formatted rather than plain lines: a tag (`<b>`, `</p>`, `<a href=…>`) with
 *  a ">" after it, or a character reference (`&amp;`, `&sup2;`, `&#39;`, `&#x27;`). Rust's `is_html`
 *  exactly (the fixture's `html` column pins both). A scan whose last ">" is found once, so a run of
 *  "<a<a<a" is linear. */
export function looksLikeHtml(description: string): boolean {
  const s = description;
  const lastGt = s.lastIndexOf(">");
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "<") {
      const name = s[i + 1] === "/" ? i + 2 : i + 1;
      if (isAlpha(s[name] ?? "") && lastGt > name) return true;
    } else if (c === "&" && isReference(s, i)) {
      return true;
    }
  }
  return false;
}

/** Whether a description holds a character reference, as `looksLikeHtml` reads one. */
function hasReference(s: string): boolean {
  for (let at = s.indexOf("&"); at >= 0; at = s.indexOf("&", at + 1)) {
    if (isReference(s, at)) return true;
  }
  return false;
}

/** "&", then a letter and up to seven letters or digits, "#" and digits, or "#x" and hex digits; then
 *  ";". Any name, known or not: Rust can't tell either, and both must agree. */
function isReference(s: string, from: number): boolean {
  let j = from + 1;
  if (isAlpha(s[j] ?? "")) {
    j++;
    while (j < s.length && j <= from + 8 && (isAlpha(s[j]) || isDigit(s[j]))) j++;
    return s[j] === ";";
  }
  if (s[j] !== "#") return false;
  j++;
  const hex = s[j] === "x" || s[j] === "X";
  if (hex) j++;
  const digits = j;
  while (j < s.length && (hex ? isHex(s[j]) : isDigit(s[j]))) j++;
  return j > digits && s[j] === ";";
}

// --- links ---

/** The address a link may carry: one starting http://, https:// or mailto: (any case) that `safeUrl`
 *  hands back unchanged, with no whitespace or control character; else null. `useExternalLinks` opens
 *  only `https?://`, so "https:example.com" (no "//") is refused rather than drawn as a dead link. */
export function linkTarget(href: string): string | null {
  if (!unbroken(href)) return null;
  const head = asciiLower(href.slice(0, 8));
  const known =
    head.startsWith("http://") || head.startsWith("https://") || head.startsWith("mailto:");
  // markdown.tsx's rule takes every address the prefix test takes today; asking it too means a
  // change to either rule can't let through an address the other refuses.
  return known && safeUrl(href) === href ? href : null;
}

/** An address a link in the model may hold: a `linkTarget` one, or a phone number ("tel:" and digits,
 *  "+", "-", ".", "(", ")", ",", ";", "#" and "*"), which PM keeps but shows as its words. */
export function keptLink(href: string): string | null {
  return linkTarget(href) ?? (PHONE.test(href) && /\d/.test(href) ? href : null);
}
const PHONE = /^tel:[0-9+\-.(),;#*]+$/i;

/** What a typed address means: an address `linkTarget` takes, as it is; "name@example.com" as
 *  mailto:name@example.com; "example.com/path" as https://example.com/path; else null. Google's own
 *  editor stores links without "https://" ("www.example.org"); they read through this too. */
export function linkFromTyped(typed: string): string | null {
  const s = typed.trim();
  if (s.length > MAX_TYPED || !unbroken(s)) return null;
  const direct = linkTarget(s);
  if (direct !== null) return direct;
  // An email address, split at its "@" with indexOf rather than a pattern that could backtrack; a
  // second "@" fails the domain's pattern.
  const at = s.indexOf("@");
  if (at > 0 && LOCAL.test(s.slice(0, at)) && DOMAIN.test(s.slice(at + 1))) {
    return linkTarget(`mailto:${s}`);
  }
  return WEB.test(s) ? linkTarget(`https://${s}`) : null;
}
const MAX_TYPED = 2000;
/** An email address's part before the "@": RFC 5322's atom characters and dots, without "%" or "/". */
const LOCAL = /^[a-z0-9.!#$&'*+=?^_`{|}~-]+$/i;
/** Labels split by a literal "." their class excludes, so neither pattern can backtrack. */
const DOMAIN = /^[a-z0-9-]+(\.[a-z0-9-]+)+$/i;
const WEB = /^[a-z0-9-]+(\.[a-z0-9-]+)+([/?#]\S*)?$/i;

/** Words, or words that are a link, as a description's text is drawn. */
export interface TextPiece {
  readonly text: string;
  /** An address `linkTarget` takes, or null for words. */
  readonly href: string | null;
}

/** A description's text with the addresses written in it made links, as Google Calendar draws them:
 *  runs starting "http://", "https://" or "www.", and "name@example.com" runs. For drawing only,
 *  never in the model, so a save writes them back as the words they are. One pass: a run ends at
 *  whitespace, "<", ">" or '"'; an opening bracket or quote before it is left out, and so are
 *  ".,;:!?", a closing quote, and a ")" or "]" the run doesn't open, after it. What's left is an
 *  address `linkFromTyped` reads, or stays words. */
export function textLinks(text: string): TextPiece[] {
  const pieces: TextPiece[] = [];
  let words = 0; // where the words no piece holds yet start
  let at = 0;
  while (at < text.length) {
    if (RUN_ENDS.test(text[at])) {
      at++;
      continue;
    }
    let end = at;
    while (end < text.length && !RUN_ENDS.test(text[end])) end++;
    const link = runLink(text, at, end);
    if (link !== null) {
      if (link.from > words) pieces.push({ text: text.slice(words, link.from), href: null });
      pieces.push({ text: text.slice(link.from, link.to), href: link.href });
      words = link.to;
    }
    at = end;
  }
  if (words < text.length) pieces.push({ text: text.slice(words), href: null });
  return pieces;
}
const RUN_ENDS = /[\s<>"]/;
const OPENERS = "(['‘“";
const TRAILING = ".,;:!?'’”";

/** The link a run of text makes, without the punctuation around it, or null. */
function runLink(
  text: string,
  start: number,
  end: number,
): { from: number; to: number; href: string } | null {
  let from = start;
  let to = end;
  while (from < to && OPENERS.includes(text[from])) from++;
  const run = text.slice(from, to);
  const head = asciiLower(run.slice(0, 8));
  const address =
    head.startsWith("http://") ||
    head.startsWith("https://") ||
    head.startsWith("www.") ||
    run.includes("@");
  if (!address) return null;
  // Trimming takes only closing characters off, so whether the run opens a bracket can't change.
  const opensParen = run.includes("(");
  const opensBracket = run.includes("[");
  while (to > from) {
    const c = text[to - 1];
    if (TRAILING.includes(c) || (c === ")" && !opensParen) || (c === "]" && !opensBracket)) to--;
    else break;
  }
  // Every address linkFromTyped gives is one linkTarget took.
  const href = linkFromTyped(text.slice(from, to));
  return href === null ? null : { from, to, href };
}

/** No whitespace (Unicode's included) and no control character. */
function unbroken(s: string): boolean {
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    if (code <= 0x20 || (code >= 0x7f && code <= 0x9f)) return false;
  }
  return !/\s/.test(s);
}

/** The address an `a` element's `href` gives the model: one it keeps as it is, or an address Google
 *  stored without its scheme, in full; else null. */
function linkOfHref(href: string): string | null {
  return keptLink(href) ?? (unbroken(href) ? linkFromTyped(href) : null);
}

// --- reading formatted descriptions ---

/** A node while reading: the model's, or `plain`, whose content is drawn without the marks it names
 *  however many enclose it (`font-weight: normal`). Only ever in a description PM can't write back,
 *  or in the editor's box, where Chromium can leave one inside bold (descriptionDom.ts). */
export type Loose =
  | { readonly kind: "text"; text: string }
  | { readonly kind: "br" }
  | { readonly kind: Mark | Block; readonly children: Loose[] }
  | { readonly kind: "a"; readonly href: string; readonly children: Loose[] }
  | { readonly kind: "plain"; readonly marks: readonly Mark[]; readonly children: Loose[] };

/** What holds a frame's content. */
type Holder = "root" | Mark | Block | "a" | "plain";

/** An open element. Unwrapped and line elements get one too, so their end tags find them. */
interface Frame {
  /** The tag that opened it. */
  readonly name: string;
  /** The children it sits among (for its line edges). */
  readonly outer: Loose[];
  /** Where its content goes. */
  readonly into: Loose[];
  readonly holder: Holder;
  /** Whether its tag reads as bold, italic, underline or a link that is drawn: an element browsers
   *  carry into the next list item, so closing one early changes what shows. */
  readonly formatting: boolean;
  /** A `div` or another block that isn't in the model: its edges break the line. */
  readonly line: boolean;
  /** Nodes between the document and `into`. */
  readonly depth: number;
}

const MARK_TAGS = new Map<string, Mark>([
  ["b", "b"],
  ["strong", "b"],
  ["i", "i"],
  ["em", "i"],
  ["u", "u"],
]);
const BLOCK_TAGS = new Map<string, Block>([
  ["p", "p"],
  ["ul", "ul"],
  ["ol", "ol"],
  ["li", "li"],
]);
/** Unwrapped, and written back without them: they draw nothing themselves. */
const SPANS: ReadonlySet<string> = new Set(["span", "html-blob"]);
/** Elements the HTML tree builder stops at when an `li` looks for the item it closes. */
const LIST_STOPS: ReadonlySet<string> = new Set(
  [...BLOCKS].filter((name) => name !== "div" && name !== "p" && name !== "address"),
);
/** Attributes that draw nothing PM must keep: Google's own bookkeeping on links and spans. */
const DROPPED: ReadonlySet<string> = new Set(["id", "target", "rel", "lang", "__is_owner"]);
const BR: Loose = { kind: "br" };
const BLOCK_KINDS: ReadonlySet<string> = new Set(["p", "ul", "ol", "li"]);
const isMark = (kind: string): kind is Mark => kind === "b" || kind === "i" || kind === "u";

/** What an element's attributes say, for writing back and for what shows. */
interface ElementRead {
  /** Every attribute is one PM writes back or may drop. */
  readonly ok: boolean;
  /** An `a`'s address, when the link rule gives one. */
  readonly href: string | null;
  /** Marks its style puts on its content. */
  readonly marks: readonly Mark[];
  /** Bold or italic its style takes off its content, enclosing elements' included. */
  readonly plain: readonly Mark[];
  /** Its style takes off its own underline (an enclosing one still draws through it, as in CSS). */
  readonly noUnderline: boolean;
}

/** The style declarations PM reads, each `;`-separated declaration lower-cased, trimmed and without
 *  `!important`. A colour or background colour that is only a variable (`var(--…)`, Google's own
 *  theme colours) draws nothing PM must keep: no other page defines Google's variables. A variable
 *  on any other property fails closed, as it may draw there. Weight, slant and underline read as
 *  marks; every other
 *  declaration fails closed. Turning a mark off (`font-weight: normal`, Google Docs' paste wrapper)
 *  shows as plain but fails closed too. Later declarations of a property win, as in CSS. */
function readStyle(style: string): Omit<ElementRead, "href"> {
  let ok = true;
  let weight: boolean | null = null;
  let slant: boolean | null = null;
  let line: boolean | null = null;
  for (const part of style.split(";")) {
    const declaration = trimSpace(asciiLower(part));
    if (declaration === "") continue;
    const colon = declaration.indexOf(":");
    if (colon < 0) {
      ok = false;
      continue;
    }
    const property = trimSpace(declaration.slice(0, colon));
    const value = trimSpace(declaration.slice(colon + 1).replace(/!\s*important$/, ""));
    if ((property === "color" || property === "background-color") && VARIABLE.test(value)) {
      continue;
    }
    if (property === "font-weight") {
      const number = /^\d{1,4}$/.test(value) ? Number(value) : null;
      if (value === "bold" || value === "bolder" || (number !== null && number >= 600)) {
        weight = true;
        if (number === null || number <= 900) continue;
      } else if (value === "normal" || value === "lighter" || (number !== null && number > 0)) {
        weight = false;
      }
    } else if (property === "font-style") {
      if (value === "italic" || value === "oblique") {
        slant = true;
        continue;
      }
      if (value === "normal") slant = false;
    } else if (property === "text-decoration" || property === "text-decoration-line") {
      if (value === "underline") {
        line = true;
        continue;
      }
      if (value === "none") line = false;
    }
    ok = false;
  }
  const marks: Mark[] = [];
  if (weight === true) marks.push("b");
  if (slant === true) marks.push("i");
  if (line === true) marks.push("u");
  const plain: Mark[] = [];
  if (weight === false) plain.push("b");
  if (slant === false) plain.push("i");
  return { ok, marks, plain, noUnderline: line === false };
}
const VARIABLE = /^var\(\s*--[a-z0-9_-]+\s*\)$/;

/** HTML's whitespace trimmed from both ends (a no-break space is a character, not whitespace, in CSS). */
function trimSpace(s: string): string {
  let from = 0;
  let to = s.length;
  while (from < to && isSpace(s[from])) from++;
  while (to > from && isSpace(s[to - 1])) to--;
  return s.slice(from, to);
}

function readElement(name: string, attrs: readonly Attr[]): ElementRead {
  let ok = true;
  let href: string | null = null;
  let style: Omit<ElementRead, "href"> = { ok: true, marks: [], plain: [], noUnderline: false };
  let hasHref = false;
  for (const { name: attr, value } of attrs) {
    if (attr === "href" && name === "a") {
      hasHref = true;
      href = linkOfHref(value);
    } else if (attr === "style") {
      style = readStyle(value);
    } else if (!(DROPPED.has(attr) || attr.startsWith("data-") || attr.startsWith("aria-"))) {
      ok = false; // `class`, `title`, `align`, `dir`, `onclick`…: they may draw, or do, something
    }
  }
  if (name === "a" && (!hasHref || href === null)) ok = false;
  return { ...style, ok: ok && style.ok, href };
}

/** One pass over the tokens, building the tree on a stack of open elements. Every rule says what
 *  shows and, where it isn't what PM's writer would make, that PM can't write it back. */
class Reader {
  readonly root: Loose[] = [];
  supported = true;
  /** It met a `MARKUP` element, start or end. */
  markup = false;
  /** A `plain` node was made: the marks need redrawing around it. */
  plain = false;
  private readonly stack: Frame[];
  private openP = 0;
  private openA = 0;
  /** The space owed after a table cell, written only when more text follows on the same line. */
  private cellGap = false;
  /** The line has ended, or nothing has shown yet: the last thing read that shows, in the order it
   *  shows, was a break or a block's edge, through any formatting and links around it. Whitespace
   *  shows nothing, so it leaves this as it was. */
  private ended = true;
  /** The last text read ended in whitespace: read here, not from the text built so far, which the
   *  engine would copy whole each time (critique: a table's every cell). */
  private spaceEnd = false;

  constructor() {
    this.stack = [
      {
        name: "",
        outer: this.root,
        into: this.root,
        holder: "root",
        formatting: false,
        line: false,
        depth: 0,
      },
    ];
  }

  private get top(): Frame {
    return this.stack[this.stack.length - 1];
  }

  read(all: readonly Token[]): void {
    for (const token of all) {
      if (token.kind === "cut") {
        this.supported = false; // a mirrored description cut at its last character
        break;
      }
      switch (token.kind) {
        case "unsure":
          // A reference left as written that a browser may draw as a character: a save would write
          // the code it shows here.
          this.supported = false;
          break;
        case "text":
          // Whether Google draws a raw line break inside formatting is probe E11-1's (H9). A decoded
          // carriage return ("&#13;") is one too: browsers draw it as a space, and `normalise` would
          // make it a break.
          if (/[\r\n]/.test(token.text)) this.supported = false;
          this.text(token.text);
          break;
        case "literal":
          // "<https://…>": Google may draw it as words, a link or nothing (E11-1's H8).
          this.supported = false;
          this.text(token.text);
          break;
        case "comment":
          this.supported = false; // it may be a marker another app reads
          break;
        case "start":
          if (isMarkup(token.name)) this.markup = true;
          this.start(token.name, token.attrs);
          break;
        case "end":
          if (isMarkup(token.name)) this.markup = true;
          this.end(token.name);
          break;
      }
    }
    while (this.stack.length > 1) this.pop(); // closed, as browsers close them
  }

  private text(words: string): void {
    // The tokenizer reads a raw carriage return as a line feed, so one here was written as a
    // reference ("&#13;"), which browsers draw as a space. Only ever shown: that text is ✗.
    const text = words.replace(/\r/g, " ");
    const into = this.top.into;
    const owed = this.cellGap && !isSpace(text[0] ?? "");
    this.cellGap = false;
    const add = owed ? ` ${text}` : text;
    if (add === "") return;
    if (shows(add)) this.ended = false;
    this.spaceEnd = isSpace(add[add.length - 1]);
    const last = into[into.length - 1];
    if (last?.kind === "text") last.text += add;
    else into.push({ kind: "text", text: add });
  }

  private br(): void {
    this.cellGap = false;
    this.ended = true;
    this.top.into.push(BR);
  }

  /** A line's edge, as browsers draw a block's: a break unless the line is already empty or ended,
   *  however deep in formatting its last break is (`<b>a</b><div><b><br></b></div>`), and whatever
   *  whitespace follows it. `container` is where the next thing read goes. */
  private edge(container: Loose[]): void {
    this.cellGap = false;
    if (!this.ended) container.push(BR);
    this.ended = true;
  }

  private start(name: string, attrs: readonly Attr[]): void {
    // `selfClosing` means nothing but on `br`: browsers read `<b/>x` as a bold x.
    if (name === "br") {
      if (!readElement(name, attrs).ok) this.supported = false;
      this.br();
      return;
    }
    if (HIDDEN.has(name)) {
      this.supported = false; // dropped with its content: the tokenizer never read it
      return;
    }
    const mark = MARK_TAGS.get(name);
    const block = BLOCK_TAGS.get(name);
    const known = mark !== undefined || block !== undefined || name === "a" || SPANS.has(name);
    if (!known && name !== "div") this.supported = false;
    const read = readElement(name, attrs);
    if (!read.ok) this.supported = false;

    if (block === "li") this.closeItem();
    if (BLOCKS.has(name) && this.openP > 0) {
      this.supported = false; // a block inside a paragraph closes it
      this.popTo("p");
    }
    if (name === "a" && this.openA > 0) {
      this.supported = false; // a link inside a link closes the outer one
      this.popTo("a");
    }
    if (block === "li" && !["root", "ul", "ol"].includes(this.top.holder)) {
      this.supported = false; // an item inside formatting or a link: §2.4 step 5 lifts it out
    }

    if (VOID.has(name)) {
      if (BLOCKS.has(name)) this.edge(this.top.into); // <hr>
      return;
    }

    // The marks it draws itself: its tag's, then its style's, less any its style turns off.
    const own: Mark[] = [];
    for (const m of [mark, ...read.marks]) {
      if (m === undefined || own.includes(m) || read.plain.includes(m)) continue;
      if (m === "u" && read.noUnderline) continue;
      own.push(m);
    }
    const link = name === "a" ? read.href : null;
    const made =
      (block !== undefined ? 1 : 0) +
      (link !== null ? 1 : 0) +
      (read.plain.length > 0 ? 1 : 0) +
      own.length;
    const top = this.top;
    if (this.stack.length > MAX_DEPTH || top.depth + made > MAX_DEPTH) {
      this.supported = false; // deeper elements are unwrapped, with no frame
      return;
    }

    const line = block === undefined && BLOCKS.has(name);
    if (line) this.edge(top.into);
    let into = top.into;
    let holder: Holder = top.holder;
    let depth = top.depth;
    const open = (node: Loose & { children: Loose[] }, kind: Holder) => {
      into.push(node);
      into = node.children;
      holder = kind;
      depth++;
    };
    if (block !== undefined) {
      open({ kind: block, children: [] }, block);
      this.ended = true; // a paragraph, list or item starts a line of its own
    }
    if (link !== null) open({ kind: "a", href: link, children: [] }, "a");
    if (read.plain.length > 0) {
      this.plain = true;
      open({ kind: "plain", marks: read.plain, children: [] }, "plain");
    }
    for (const m of own) open({ kind: m, children: [] }, m);
    this.stack.push({
      name,
      outer: top.into,
      into,
      holder,
      formatting: (mark !== undefined && own.includes(mark)) || link !== null,
      line,
      depth,
    });
    if (name === "p") this.openP++;
    if (name === "a") this.openA++;
  }

  private end(name: string): void {
    if (name === "br") {
      this.br(); // browsers read "</br>" as "<br>"
      return;
    }
    if (name === "td" || name === "th") {
      // A gap is owed after a cell's words, formatted ones too, unless they end in a space.
      const into = this.top.into;
      const last = into[into.length - 1];
      this.cellGap =
        last !== undefined &&
        last.kind !== "br" &&
        !BLOCK_KINDS.has(last.kind) &&
        !(last.kind === "text" && this.spaceEnd);
    }
    // The stack never holds more than MAX_DEPTH elements, so this search is bounded.
    let at = this.stack.length - 1;
    while (at >= 1 && this.stack[at].name !== name) at--;
    if (at < 1) {
      this.supported = false; // nothing to close: ignored
      return;
    }
    if (at !== this.stack.length - 1) this.supported = false; // misnested: closes what's inside
    while (this.stack.length > at) this.pop();
  }

  /** An `li` closes the open item it belongs to, as the HTML tree builder does: through inline
   *  elements, but not out of a list or a block like a table. */
  private closeItem(): void {
    let crossed = false;
    for (let at = this.stack.length - 1; at >= 1; at--) {
      const frame = this.stack[at];
      if (frame.name === "li") {
        // Browsers carry formatting it closes into the new item; PM's writer never would.
        if (crossed) this.supported = false;
        while (this.stack.length > at) this.pop();
        return;
      }
      if (LIST_STOPS.has(frame.name)) return;
      crossed ||= frame.formatting || frame.name === "a";
    }
  }

  private popTo(name: string): void {
    let at = this.stack.length - 1;
    while (at >= 1 && this.stack[at].name !== name) at--;
    while (at >= 1 && this.stack.length > at) this.pop();
  }

  private pop(): void {
    const frame = this.stack.pop();
    if (frame === undefined) return;
    if (frame.line) this.edge(frame.outer);
    if (BLOCK_TAGS.has(frame.name)) this.ended = true; // and ends its last one
    if (frame.name === "p") this.openP--;
    if (frame.name === "a") this.openA--;
  }
}

/** The model from the reader's nodes. With a `plain` node in it, each piece of text is drawn with
 *  exactly the marks it shows with, outermost bold, then italic, then underline: the description
 *  can't be written back anyway (or, from the editor's box, is written as it shows), so only what
 *  shows matters. */
export function fromLoose(nodes: readonly Loose[], redraw: boolean): DescNode[] {
  if (!redraw) return nodes.flatMap(asNodes);
  const items: Item[] = [];
  flatten(nodes, [], items);
  return rebuild(items);
}

/** The model's nodes as they are (no `plain` node among them). */
function asNodes(node: Loose): DescNode[] {
  switch (node.kind) {
    case "text":
      return [{ kind: "text", text: node.text }];
    case "br":
      return [node];
    case "a":
      return [{ kind: "a", href: node.href, children: node.children.flatMap(asNodes) }];
    case "plain":
      return node.children.flatMap(asNodes);
    default:
      return [{ kind: node.kind, children: node.children.flatMap(asNodes) }];
  }
}

type Item =
  { readonly leaf: DescNode; readonly marks: readonly Mark[] } | { readonly node: DescNode };

function flatten(nodes: readonly Loose[], on: readonly Mark[], items: Item[]): void {
  for (const node of nodes) {
    switch (node.kind) {
      case "text":
        items.push({ leaf: { kind: "text", text: node.text }, marks: on });
        break;
      case "br":
        items.push({ leaf: node, marks: on });
        break;
      case "b":
      case "i":
      case "u":
        flatten(node.children, on.includes(node.kind) ? on : [...on, node.kind], items);
        break;
      case "plain":
        flatten(
          node.children,
          on.filter((m) => !node.marks.includes(m)),
          items,
        );
        break;
      case "a": {
        const inner: Item[] = [];
        flatten(node.children, on, inner);
        items.push({ node: { kind: "a", href: node.href, children: rebuild(inner) } });
        break;
      }
      default: {
        const inner: Item[] = [];
        flatten(node.children, on, inner);
        items.push({ node: { kind: node.kind, children: rebuild(inner) } });
      }
    }
  }
}

function rebuild(items: readonly Item[]): DescNode[] {
  const out: DescNode[] = [];
  let i = 0;
  while (i < items.length) {
    const item = items[i];
    if ("node" in item) {
      out.push(item.node);
      i++;
      continue;
    }
    const key = (["b", "i", "u"] as const).filter((m) => item.marks.includes(m));
    const leaves: DescNode[] = [];
    while (i < items.length) {
      const next = items[i];
      if ("node" in next || !sameMarks(next.marks, key)) break;
      leaves.push(next.leaf);
      i++;
    }
    let wrapped: DescNode[] = leaves;
    for (const m of [...key].reverse()) wrapped = [{ kind: m, children: wrapped }];
    // One by one: spread as arguments, a huge description's list would pass the engine's limit.
    for (const node of wrapped) out.push(node);
  }
  return out;
}

const sameMarks = (a: readonly Mark[], b: readonly Mark[]) =>
  a.length === b.length && b.every((m) => a.includes(m));

/** A description as PM reads it: one parse giving what shows, and whether PM can write it back. */
export function parseDescription(description: string): ParsedDescription {
  if (!looksLikeHtml(description)) {
    const text = description.replace(/\r\n?/g, "\n");
    return {
      doc: normalise([{ kind: "text", text }]),
      supported: true,
      html: false,
      markup: markdownDropsALine(description),
    };
  }
  const reader = new Reader();
  reader.read(tokens(description));
  const doc = normalise(fromLoose(reader.root, reader.plain));
  const markup = reader.markup || hasReference(description) || markdownDropsALine(description);
  return { doc, supported: reader.supported, html: true, markup };
}

// --- normalising ---

type Parent = "root" | Mark | Block | "a";
interface Context {
  readonly parent: Parent;
  /** Inside a paragraph, through formatting and links: no block may be. */
  readonly inP: boolean;
  /** Inside a link: no link may be. */
  readonly inLink: boolean;
}
const ROOT: Context = { parent: "root", inP: false, inLink: false };

/** A node as normalised, and what it carries that its container may not hold: an item, or a block
 *  (a paragraph or list, alone or inside formatting). */
interface Piece {
  readonly node: DescNode;
  readonly carries: "li" | "block" | null;
}

type Element = Exclude<DescNode, { kind: "text" } | { kind: "br" }>;

/** The document every reader hands on: this one, plain text, and the editor's (§2.4). Neighbouring
 *  texts merged; whitespace HTML draws nothing for dropped; raw line breaks made breaks; empty
 *  formatting dropped and doubled formatting undone; then made one PM's reader accepts as it is:
 *  a block inside a paragraph splits it, a link inside a link is its words, and an item inside
 *  formatting, a link or another item is lifted out (the formatting goes inside it). Last, the
 *  breaks that end the document, a paragraph or an item after its words, which draw nothing. Idempotent,
 *  and nothing deeper than MAX_DEPTH survives as anything but its text. */
export function normalise(doc: DescDoc): DescDoc {
  return tidy(
    pieces(doc, ROOT, 1).map((p) => p.node),
    ROOT,
  );
}

function pieces(nodes: DescDoc, ctx: Context, depth: number): Piece[] {
  const out: Piece[] = [];
  // Never `push(...list)`: spread as arguments, a huge description's list passes the engine's limit.
  for (const node of nodes) for (const piece of piecesOf(node, ctx, depth)) out.push(piece);
  return out;
}

function piecesOf(node: DescNode, ctx: Context, depth: number): Piece[] {
  if (node.kind === "text" || node.kind === "br") return [{ node, carries: null }];
  if (depth > MAX_DEPTH) return flat(node).map((leaf) => ({ node: leaf, carries: null }));
  // A link inside a link is its words; a mark directly inside the same mark is one mark.
  if (node.kind === "a" && ctx.inLink) return pieces(node.children, ctx, depth);
  if (isMark(node.kind) && ctx.parent === node.kind) return pieces(node.children, ctx, depth);

  const inline = isMark(node.kind) || node.kind === "a";
  const inner: Context = {
    parent: node.kind,
    inP: node.kind === "p" || (ctx.inP && inline),
    inLink: ctx.inLink || node.kind === "a",
  };
  const out: Piece[] = [];
  let run: DescNode[] = [];
  let split = false;
  const close = () => {
    const children = tidy(run, inner);
    run = [];
    // Empty formatting draws nothing; nor does an empty part of a paragraph or item split around a
    // block. A paragraph or item that was empty to begin with stays.
    if (children.length === 0 && (inline || split)) return;
    out.push(pieceOf(withChildren(node, children)));
  };
  for (const item of pieces(node.children, inner, depth + 1)) {
    if (!leaves(item, inner)) {
      run.push(item.node);
      continue;
    }
    split = true;
    close();
    for (const piece of lifted(item, node, ctx, depth)) out.push(piece);
  }
  close();
  return out;
}

function pieceOf(node: DescNode): Piece {
  const carries = node.kind === "li" ? "li" : BLOCK_KINDS.has(node.kind) ? "block" : null;
  return { node, carries };
}

/** Whether a piece may not stay among the children of a container described by `ctx`. */
function leaves(piece: Piece, ctx: Context): boolean {
  if (piece.carries === "li")
    return ctx.parent !== "root" && ctx.parent !== "ul" && ctx.parent !== "ol";
  if (piece.carries === "block") return ctx.parent === "p" || ctx.inP;
  return false;
}

/** A piece that left `from` (itself among children described by `ctx`), as it sits beside it. An item
 *  takes formatting or a link inside it; a block takes it around it; out of a paragraph or an item,
 *  either goes as it is. */
function lifted(piece: Piece, from: Element, ctx: Context, depth: number): Piece[] {
  const inline = isMark(from.kind) || from.kind === "a";
  if (!inline) return [piece];
  if (piece.carries === "li" && piece.node.kind === "li") {
    const item: DescNode = { kind: "li", children: [withChildren(from, piece.node.children)] };
    return piecesOf(item, ctx, depth);
  }
  return [{ node: withChildren(from, [piece.node]), carries: piece.carries }];
}

function withChildren(node: Element, children: DescDoc): DescNode {
  return node.kind === "a"
    ? { kind: "a", href: node.href, children }
    : { kind: node.kind, children };
}

/** The text and breaks of a subtree, without recursing (it may be any depth). */
function flat(node: DescNode): DescNode[] {
  const out: DescNode[] = [];
  const todo: DescNode[] = [node];
  while (todo.length > 0) {
    const next = todo.pop() as DescNode;
    if (next.kind === "text" || next.kind === "br") out.push(next);
    else for (let k = next.children.length - 1; k >= 0; k--) todo.push(next.children[k]);
  }
  return out;
}

const isWhitespace = (text: string) => [...text].every(isSpace);

/** Whether text draws anything: a character other than HTML's whitespace. */
export function shows(text: string): boolean {
  for (let k = 0; k < text.length; k++) if (!isSpace(text[k])) return true;
  return false;
}

/** One container's children made tidy (§2.4 steps 1-3, 6 and 7). */
function tidy(nodes: readonly DescNode[], ctx: Context): DescNode[] {
  // 1. Neighbouring texts merged, empty ones dropped.
  const merged = mergeTexts(nodes);
  // 2. Whitespace HTML draws nothing for: inside a list, or beside a paragraph, list or item.
  const kept = merged.filter((node, k) => {
    if (node.kind !== "text" || !isWhitespace(node.text)) return true;
    if (ctx.parent === "ul" || ctx.parent === "ol") return false;
    const before = merged[k - 1];
    const after = merged[k + 1];
    return !(
      (before !== undefined && BLOCK_KINDS.has(before.kind)) ||
      (after !== undefined && BLOCK_KINDS.has(after.kind))
    );
  });
  // 3. A raw line break in text (CR LF and a lone CR too, as the tokenizer reads them) is a break.
  const lines: DescNode[] = [];
  for (const node of kept) {
    if (node.kind !== "text" || !/[\r\n]/.test(node.text)) {
      lines.push(node);
      continue;
    }
    node.text.split(/\r\n?|\n/).forEach((part, k) => {
      if (k > 0) lines.push(BR_NODE);
      if (part !== "") lines.push({ kind: "text", text: part });
    });
  }
  // 6. The break that ends the document, a paragraph or an item right after its words draws nothing:
  // it ends a line that would end there anyway ("a<br>" is one line). One after another break, or
  // after a block, draws an empty line ("a<br><br>" is two; "<p><br></p>" is one), so it stays.
  // Its line ends wherever the last thing that shows is, inside formatting too: after
  // "<b>x<br></b>" it is a second, empty line, so it stays.
  if (ctx.parent === "root" || ctx.parent === "p" || ctx.parent === "li") {
    const n = lines.length;
    if (lines[n - 1]?.kind === "br" && lineEnded(lines, n - 1, 1) === false) {
      lines.length = n - 1;
    }
  }
  // 7. Texts merged again.
  return mergeTexts(lines);
}
const BR_NODE: DescNode = { kind: "br" };

/** Whether the line has ended where `nodes[0..to)` end: true after a break or a block, false after
 *  words, looked for through formatting and links (`<b>x<br></b>` ends one), past whitespace, which
 *  shows nothing; null when nothing there shows. Looks no deeper than MAX_DEPTH. */
function lineEnded(nodes: DescDoc, to: number, depth: number): boolean | null {
  for (let k = to - 1; k >= 0; k--) {
    const node = nodes[k];
    if (node.kind === "text") {
      if (shows(node.text)) return false;
      continue;
    }
    if (node.kind === "br" || BLOCK_KINDS.has(node.kind)) return true;
    if (depth > MAX_DEPTH) return false;
    const inner = lineEnded(node.children, node.children.length, depth + 1);
    if (inner !== null) return inner;
  }
  return null;
}

function mergeTexts(nodes: readonly DescNode[]): DescNode[] {
  const out: DescNode[] = [];
  for (const node of nodes) {
    if (node.kind === "text") {
      if (node.text === "") continue;
      const last = out[out.length - 1];
      if (last?.kind === "text") {
        out[out.length - 1] = { kind: "text", text: last.text + node.text };
        continue;
      }
    }
    out.push(node);
  }
  return out;
}

// --- writing ---

const ESCAPES: Readonly<Record<string, string>> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
};
/** One global replace over one class: a sanitiser CodeQL recognises, and no chain to double-escape. */
const escape = (s: string) => s.replace(/[&<>"]/g, (c) => ESCAPES[c]);

/** A description to save, from a document. Nothing to show is `""`, which clears it. No formatting is
 *  plain lines, unless those lines would read as formatting (`<b>` or `&amp;` typed as words). Anything
 *  else is Google's subset: `<b>`, `<i>`, `<u>`, `<p>`, `<ul>`, `<ol>`, `<li>`, `<br>` and
 *  `<a href="…">`, text and addresses escaped, no other attribute. */
export function serialiseDescription(doc: DescDoc): string {
  if (isEmptyDoc(doc)) return "";
  if (doc.every((node) => node.kind === "text" || node.kind === "br")) {
    const plain = doc.map((node) => (node.kind === "text" ? node.text : "\n")).join("");
    if (!looksLikeHtml(plain)) return plain;
  }
  let out = "";
  // Without recursing: a closing tag waits on the stack behind its children.
  const todo: (DescNode | string)[] = [...doc].reverse();
  while (todo.length > 0) {
    const next = todo.pop() as DescNode | string;
    if (typeof next === "string") {
      out += next;
      continue;
    }
    switch (next.kind) {
      case "text":
        // The model holds no raw line break, but a break is what one would mean.
        out += next.text.split("\n").map(escape).join("<br>");
        break;
      case "br":
        out += "<br>";
        break;
      case "a":
        if (keptLink(next.href) === next.href) {
          out += `<a href="${escape(next.href)}">`;
          todo.push("</a>");
        }
        for (let k = next.children.length - 1; k >= 0; k--) todo.push(next.children[k]);
        break;
      default:
        out += `<${next.kind}>`;
        todo.push(`</${next.kind}>`);
        for (let k = next.children.length - 1; k >= 0; k--) todo.push(next.children[k]);
    }
  }
  return out;
}

// --- comparing ---

/** Whether a document shows nothing: no text but whitespace and no-break spaces. */
export function isEmptyDoc(doc: DescDoc): boolean {
  const todo: DescNode[] = [...doc];
  while (todo.length > 0) {
    const next = todo.pop() as DescNode;
    if (next.kind === "text") {
      // Besides HTML's whitespace, the character compared is a no-break space (U+00A0), which an
      // emptied editor often leaves behind.
      if (![...next.text].every((c) => isSpace(c) || c === " ")) return false;
    } else if (next.kind !== "br") {
      for (const child of next.children) todo.push(child);
    }
  }
  return true;
}

/** Whether two documents are the same, node for node. */
export function docsEqual(a: DescDoc, b: DescDoc): boolean {
  const todo: [DescDoc, DescDoc][] = [[a, b]];
  while (todo.length > 0) {
    const [x, y] = todo.pop() as [DescDoc, DescDoc];
    if (x.length !== y.length) return false;
    for (let k = 0; k < x.length; k++) {
      const m = x[k];
      const n = y[k];
      if (m.kind !== n.kind) return false;
      if (m.kind === "text" || n.kind === "text") {
        if (m.kind !== "text" || n.kind !== "text" || m.text !== n.text) return false;
      } else if (m.kind !== "br" && n.kind !== "br") {
        if (m.kind === "a" && n.kind === "a" && m.href !== n.href) return false;
        todo.push([m.children, n.children]);
      }
    }
  }
  return true;
}

/** Whether two descriptions show the same thing: equal text (CR LF = LF, as the draft's `sameText`),
 *  or both read by PM to equal documents, or both showing nothing. A description PM can't write back
 *  compares by text only, so one with an image and nothing else still differs from an empty one. */
export function sameDescription(a: string, b: string): boolean {
  if (a.replace(/\r\n?/g, "\n") === b.replace(/\r\n?/g, "\n")) return true;
  const x = parsedOf(a);
  const y = parsedOf(b);
  if (!x.supported || !y.supported) return false;
  return (isEmptyDoc(x.doc) && isEmptyDoc(y.doc)) || docsEqual(x.doc, y.doc);
}

/** The last few descriptions read for a comparison. The editor compares Google's copy with the draft
 *  on every keystroke, so the copy is read once rather than each time (the documents never change). */
const recent: { text: string; parsed: ParsedDescription }[] = [];
const RECENT = 4;

function parsedOf(text: string): ParsedDescription {
  const hit = recent.find((r) => r.text === text);
  if (hit) return hit.parsed;
  const parsed = parseDescription(text);
  recent.unshift({ text, parsed });
  recent.length = Math.min(recent.length, RECENT);
  return parsed;
}

// --- replacing ---

/** "Replace with plain text…": the words and lines of what shows, a list's items as "• " and "1. "
 *  lines (two spaces more per level), and each link's address after its words, "(…)", unless the
 *  words are the address. Runs of three or more line breaks become two, and the ends are trimmed, as
 *  the editor's old read-only text was. */
export function replacementText(doc: DescDoc): string {
  const plain = new PlainText();
  plain.nodes(doc, 0, 1);
  return plain
    .text()
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** "Replace with plain text…" on `description` (Google's copy), as the description it saves.
 *  `replacementText` is words: plain lines, unless they would read as formatting, which goes escaped,
 *  lines as breaks, as typed words that look like formatting are written (`serialiseDescription`):
 *  "&lt;div&gt;" or "&amp;amp;" Google showed as words stays words rather than going to Google as
 *  live markup, and "&lt;ann@example.com&gt;" stays escaped. An address the description itself writes
 *  in angle brackets ("<https://…>") is kept as written (critique C1-H1, until probe H8), when
 *  nothing else in the words would read otherwise. */
export function replacementDescription(description: string): string {
  const text = replacementText(parseDescription(description).doc);
  if (!looksLikeHtml(text)) return text;
  const literal = tokens(description).some((token) => token.kind === "literal");
  if (literal && replacementText(parseDescription(text).doc) === text) return text;
  return serialiseDescription(normalise([{ kind: "text", text }]));
}

class PlainText {
  /** What is written so far, in pieces joined once at the end: asking a string built by `+=` how it
   *  ends makes the engine copy it whole each time, which a long description would make quadratic. */
  private readonly parts: string[] = [];
  /** The last character written, "" before any. */
  private last = "";
  /** What a line inside the current item starts with, after its first. */
  private indent = "";
  /** The current item's marker, until its first words take it. */
  private marker: string | null = null;

  text(): string {
    return this.parts.join("");
  }

  nodes(doc: DescDoc, level: number, depth: number): void {
    for (const node of doc) this.node(node, level, depth);
  }

  private node(node: DescNode, level: number, depth: number): void {
    if (node.kind === "text") {
      node.text.split("\n").forEach((part, k) => {
        if (k > 0) this.put("\n");
        this.words(part);
      });
      return;
    }
    if (node.kind === "br") {
      this.put("\n");
      return;
    }
    // Deeper than the model ever is: the words alone.
    if (depth > MAX_DEPTH) {
      for (const leaf of flat(node)) this.node(leaf, level, depth);
      return;
    }
    switch (node.kind) {
      case "p":
        this.newLine();
        this.nodes(node.children, level, depth + 1);
        this.newLine();
        return;
      case "ul":
      case "ol": {
        this.newLine();
        let n = 0;
        for (const child of node.children) {
          if (child.kind !== "li") this.node(child, level, depth + 1);
          else this.item(child, node.kind === "ol" ? `${++n}. ` : "• ", level + 1, depth + 1);
        }
        this.newLine();
        return;
      }
      case "li":
        // An item outside a list, as Google may hold one, is a bulleted line.
        this.item(node, "• ", level + 1, depth);
        return;
      case "a": {
        this.nodes(node.children, level, depth + 1);
        const said = shownAddress(node.href);
        const words = wordsOf(node.children).trim();
        if (words !== said && linkFromTyped(words) !== node.href) this.words(` (${said})`);
        return;
      }
      default:
        this.nodes(node.children, level, depth + 1);
    }
  }

  /** A list item: its marker on its first line, its later lines under its words. */
  private item(node: { readonly children: DescDoc }, marker: string, level: number, depth: number) {
    const outer = this.indent;
    const pad = "  ".repeat(level - 1);
    this.newLine();
    this.marker = pad + marker;
    this.indent = pad + "  ";
    this.nodes(node.children, level, depth + 1);
    this.newLine();
    this.marker = null;
    this.indent = outer;
  }

  /** Words, after the item's marker or a line's indent when they start a line. */
  private words(text: string): void {
    if (text === "") return;
    if (this.atLineStart()) {
      this.put(this.marker ?? this.indent);
      this.marker = null;
    }
    this.put(text);
  }

  private newLine(): void {
    if (!this.atLineStart()) this.put("\n");
  }

  private atLineStart(): boolean {
    return this.last === "" || this.last === "\n";
  }

  private put(s: string): void {
    if (s === "") return;
    this.parts.push(s);
    this.last = s[s.length - 1];
  }
}

/** A link's address as plain text says it: a mail or phone address without its scheme. */
function shownAddress(href: string): string {
  const scheme = asciiLower(href.slice(0, 7));
  if (scheme === "mailto:") return href.slice(7);
  if (scheme.startsWith("tel:")) return href.slice(4);
  return href;
}

/** A document's words, a break as a space, without recursing. */
function wordsOf(doc: DescDoc): string {
  let words = "";
  const todo: DescNode[] = [...doc].reverse();
  for (let next = todo.pop(); next !== undefined; next = todo.pop()) {
    if (next.kind === "text") words += next.text;
    else if (next.kind === "br") words += " ";
    else for (let k = next.children.length - 1; k >= 0; k--) todo.push(next.children[k]);
  }
  return words;
}
