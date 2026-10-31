// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The tokens of an event description Google holds as HTML (#884, C19): what its text, tags and
// attributes are, for the reader that shows and edits it (descriptionHtml.ts).
//
// A character scanner, not the browser's parser: nothing here hands the description to DOMParser,
// innerHTML or a <template>, and the readers built on it make React elements or text, so no markup in
// a description can run, load or render. (CodeQL rightly treats any HTML parse of text that can come
// from a text field as a sink; an inert parse was still a parse.) New readers of formatted text extend
// this file rather than parse; ESLint refuses the parsers outright.
//
// It reads markup as the HTML tokenizer does wherever that changes what shows: line endings are
// normalised, a ">" inside a quoted attribute value doesn't end its tag, a tag the text ends inside
// takes the rest with it, and a "<" that can't open a tag is text. The content of <script>, <style> and
// the other never-shown elements is never read. A character reference is decoded once, onto the output
// and never back into the scanner, so "&lt;b&gt;" is the text "<b>", never a tag.
//
// One deliberate difference: an address in angle brackets, "<https://…>" or "<ann@example.com>" (how
// Outlook and other apps write a link into plain text), is kept as its own words rather than read as an
// element named "https:", which would make the address vanish. Whether Google draws it as words or as a
// link is probe E11-1's; the readers treat it as formatting PM can't write back until then. So too any
// other words in angle brackets that Markdown reads as words and a browser as an unknown element
// ("<www.example.com>", "<José>", "<Room 4>", "<Room A, B>"): a plain-text event never loses them. A
// name the reader knows ("<a href=…;>", "<b 4>") stays the element a browser draws (`KNOWN`).
//
// One pass that never steps back, so a hostile description costs linear time.

/** One attribute: its name lower-cased, its value with character references decoded once ("" when it
 *  has none). */
export interface Attr {
  readonly name: string;
  readonly value: string;
}

export type Token =
  /** Text, decoded once: "&lt;b&gt;" is the text "<b>", never a tag. */
  | { readonly kind: "text"; readonly text: string }
  /** `<name …>`. A never-shown element's start is followed directly by its end: its content is never
   *  read, as the HTML tokenizer reads <script> and <style>. */
  | {
      readonly kind: "start";
      readonly name: string;
      readonly attrs: readonly Attr[];
      readonly selfClosing: boolean;
    }
  | { readonly kind: "end"; readonly name: string }
  /** An address in angle brackets ("<https://…>", "<mailto:…>", "<ann@example.com>"), or other words
   *  in them (`readsAsWords`): its source from "<" to ">", decoded once. Words, not an element. */
  | { readonly kind: "literal"; readonly text: string }
  /** A comment, `<!…>`, `<?…>` or `</` not followed by a letter. Nothing of it shows. */
  | { readonly kind: "comment" }
  /** The text ended inside a tag; always the last token. */
  | { readonly kind: "cut" }
  /** Somewhere, a reference left as written that a browser would, or may, draw as a character
   *  ("&check;", "&copy 2026"); always the first token, and at most one. */
  | { readonly kind: "unsure" };

/** Elements whose content never shows: skipped to their end tag, or to the end if nothing ends them. */
export const HIDDEN: ReadonlySet<string> = new Set([
  "iframe",
  "noembed",
  "noframes",
  "noscript",
  "script",
  "style",
  "template",
  "title",
]);

/** Elements whose edges break the line. */
export const BLOCKS: ReadonlySet<string> = new Set([
  "address",
  "article",
  "aside",
  "blockquote",
  "dd",
  "div",
  "dl",
  "dt",
  "figcaption",
  "figure",
  "footer",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "header",
  "hr",
  "li",
  "main",
  "nav",
  "ol",
  "p",
  "pre",
  "section",
  "table",
  "tr",
  "ul",
]);

/** Elements with no content or end tag. */
export const VOID: ReadonlySet<string> = new Set([
  "area",
  "base",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "keygen",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

/** The names the reader has a rule for, the tags PM writes among them (a test holds `EDITABLE_TAGS`
 *  to it): always elements (`readsAsWords`), as a browser reads them. HTML defines more, but those are
 *  words where Markdown says so: `<Code 1234>`, `<Option 2>`. */
export const KNOWN: ReadonlySet<string> = new Set([
  ...HIDDEN,
  ...BLOCKS,
  ...VOID,
  "a",
  "b",
  "br",
  "em",
  "html-blob",
  "i",
  "span",
  "strong",
  "td",
  "th",
  "u",
]);

/** HTML 4's Latin-1 names, in code-point order from U+00A0: "&eacute;" is how an API writer that
 *  escapes everything spells "é". */
const LATIN1 = (
  "nbsp iexcl cent pound curren yen brvbar sect uml copy ordf laquo not shy reg macr deg plusmn " +
  "sup2 sup3 acute micro para middot cedil sup1 ordm raquo frac14 frac12 frac34 iquest " +
  "Agrave Aacute Acirc Atilde Auml Aring AElig Ccedil Egrave Eacute Ecirc Euml " +
  "Igrave Iacute Icirc Iuml ETH Ntilde Ograve Oacute Ocirc Otilde Ouml times " +
  "Oslash Ugrave Uacute Ucirc Uuml Yacute THORN szlig " +
  "agrave aacute acirc atilde auml aring aelig ccedil egrave eacute ecirc euml " +
  "igrave iacute icirc iuml eth ntilde ograve oacute ocirc otilde ouml divide " +
  "oslash ugrave uacute ucirc uuml yacute thorn yuml"
).split(" ");

/** The rest of HTML 4's names (its specials, Greek, maths, arrows and suits), with HTML 5's upper-case
 *  escapes and &apos;, as "name hex-code-point" pairs; ⟨ and ⟩ at the code points browsers use. Code
 *  points rather than characters, as several are invisible. HTML 5's other names, and the old forms
 *  written without the ";" ("&copy 2026"), stay as written: encoders that escape a description write
 *  HTML 4's names with the ";", Google included. A browser may draw those as a character, though, so
 *  the tokens say one was met (`unsure`) and the reader fails closed: a save would write it back as
 *  the code it shows here. */
const OTHERS = (
  "quot 22 QUOT 22 amp 26 AMP 26 apos 27 lt 3c LT 3c gt 3e GT 3e " +
  "OElig 152 oelig 153 Scaron 160 scaron 161 Yuml 178 fnof 192 circ 2c6 tilde 2dc " +
  "ensp 2002 emsp 2003 thinsp 2009 zwnj 200c zwj 200d lrm 200e rlm 200f ndash 2013 mdash 2014 " +
  "lsquo 2018 rsquo 2019 sbquo 201a ldquo 201c rdquo 201d bdquo 201e dagger 2020 Dagger 2021 " +
  "bull 2022 hellip 2026 permil 2030 prime 2032 Prime 2033 lsaquo 2039 rsaquo 203a euro 20ac " +
  "Alpha 391 Beta 392 Gamma 393 Delta 394 Epsilon 395 Zeta 396 Eta 397 Theta 398 Iota 399 " +
  "Kappa 39a Lambda 39b Mu 39c Nu 39d Xi 39e Omicron 39f Pi 3a0 Rho 3a1 Sigma 3a3 Tau 3a4 " +
  "Upsilon 3a5 Phi 3a6 Chi 3a7 Psi 3a8 Omega 3a9 alpha 3b1 beta 3b2 gamma 3b3 delta 3b4 " +
  "epsilon 3b5 zeta 3b6 eta 3b7 theta 3b8 iota 3b9 kappa 3ba lambda 3bb mu 3bc nu 3bd xi 3be " +
  "omicron 3bf pi 3c0 rho 3c1 sigmaf 3c2 sigma 3c3 tau 3c4 upsilon 3c5 phi 3c6 chi 3c7 psi 3c8 " +
  "omega 3c9 thetasym 3d1 upsih 3d2 piv 3d6 oline 203e frasl 2044 image 2111 weierp 2118 " +
  "real 211c trade 2122 alefsym 2135 larr 2190 uarr 2191 rarr 2192 darr 2193 harr 2194 " +
  "crarr 21b5 lArr 21d0 uArr 21d1 rArr 21d2 dArr 21d3 hArr 21d4 forall 2200 part 2202 " +
  "exist 2203 empty 2205 nabla 2207 isin 2208 notin 2209 ni 220b prod 220f sum 2211 minus 2212 " +
  "lowast 2217 radic 221a prop 221d infin 221e ang 2220 and 2227 or 2228 cap 2229 cup 222a " +
  "int 222b there4 2234 sim 223c cong 2245 asymp 2248 ne 2260 equiv 2261 le 2264 ge 2265 " +
  "sub 2282 sup 2283 nsub 2284 sube 2286 supe 2287 oplus 2295 otimes 2297 perp 22a5 sdot 22c5 " +
  "lceil 2308 rceil 2309 lfloor 230a rfloor 230b lang 27e8 rang 27e9 loz 25ca " +
  "spades 2660 clubs 2663 hearts 2665 diams 2666"
).split(" ");

/** Name → character. A Map, so "&toString;" can't find an Object property. */
const NAMED = new Map<string, string>(
  LATIN1.map((name, i): [string, string] => [name, String.fromCodePoint(0xa0 + i)]),
);
for (let k = 0; k + 1 < OTHERS.length; k += 2) {
  NAMED.set(OTHERS[k], String.fromCodePoint(parseInt(OTHERS[k + 1], 16)));
}

/** As long as the longest name above ("thetasym"). */
const MAX_NAME = 8;

/** The names a browser reads without their ";" (HTML's legacy ones: HTML 4's Latin-1 names, and the
 *  escapes in both cases), as the start of a longer run too: "&copy2026" draws "©2026". The
 *  fixture's `legacy`, and Rust's `LEGACY_NAMES`. */
export const LEGACY: ReadonlySet<string> = new Set([
  ...LATIN1,
  ...["amp", "lt", "gt", "quot", "AMP", "LT", "GT", "QUOT", "COPY", "REG"],
]);
/** The shortest and longest of them ("lt", "frac12"). */
const LEGACY_MIN = 2;
const LEGACY_MAX = 6;
/** As long as HTML 5's longest name ("CounterClockwiseContourIntegral"), and a little more: a run
 *  with a ";" within this is a name some browser may know. */
const MAX_ANY_NAME = 32;

/** What browsers make of a numeric reference to 0x80-0x9F: the Windows-1252 character it meant
 *  ("&#146;" is ’). The five codes Windows-1252 leaves empty stay as they are. */
const C1 = new Map<number, number>([
  [0x80, 0x20ac],
  [0x82, 0x201a],
  [0x83, 0x0192],
  [0x84, 0x201e],
  [0x85, 0x2026],
  [0x86, 0x2020],
  [0x87, 0x2021],
  [0x88, 0x02c6],
  [0x89, 0x2030],
  [0x8a, 0x0160],
  [0x8b, 0x2039],
  [0x8c, 0x0152],
  [0x8e, 0x017d],
  [0x91, 0x2018],
  [0x92, 0x2019],
  [0x93, 0x201c],
  [0x94, 0x201d],
  [0x95, 0x2022],
  [0x96, 0x2013],
  [0x97, 0x2014],
  [0x98, 0x02dc],
  [0x99, 0x2122],
  [0x9a, 0x0161],
  [0x9b, 0x203a],
  [0x9c, 0x0153],
  [0x9e, 0x017e],
  [0x9f, 0x0178],
]);

/** HTML's whitespace: the characters that end a tag name, an attribute or an unquoted value. */
export const isSpace = (c: string) =>
  c === " " || c === "\n" || c === "\t" || c === "\f" || c === "\r";
export const isAlpha = (c: string) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
export const isDigit = (c: string) => c >= "0" && c <= "9";
export const isHex = (c: string) => isDigit(c) || (c >= "a" && c <= "f") || (c >= "A" && c <= "F");

/** ASCII letters lower-cased, and nothing else: the HTML tokenizer's rule for names, so a Unicode
 *  letter that lower-cases to an ASCII one (the Kelvin sign to "k") can't spell a known element. */
export const asciiLower = (s: string) => s.replace(/[A-Z]+/g, (m) => m.toLowerCase());

const COMMENT: Token = { kind: "comment" };
const CUT: Token = { kind: "cut" };
const UNSURE: Token = { kind: "unsure" };

/** What the references read so far met: one left as written that a browser may draw otherwise. */
interface Note {
  unsure: boolean;
}
/** For what the reader drops, whose references change nothing it keeps: heard by nobody. */
const UNHEARD: Note = { unsure: false };

/** The tokens of an HTML description, in one pass that never steps back. */
export function tokens(description: string): Token[] {
  const note: Note = { unsure: false };
  const out = read(description, note);
  return note.unsure ? [UNSURE, ...out] : out;
}

function read(description: string, note: Note): Token[] {
  // The tokenizer reads CR LF and a lone CR as LF before anything else.
  const html = description.replace(/\r\n?/g, "\n");
  const n = html.length;
  const out: Token[] = [];
  let text = "";
  const flush = () => {
    if (text !== "") out.push({ kind: "text", text });
    text = "";
  };
  const ahead = new Ahead(html);
  let i = 0;
  while (i < n) {
    const c = html[i];
    if (c === "&") {
      const [character, next] = reference(html, i, note, mayBeDrawn);
      text += character;
      i = next;
      continue;
    }
    if (c !== "<") {
      let j = i + 1;
      while (j < n && html[j] !== "&" && html[j] !== "<") j++;
      text += html.slice(i, j);
      i = j;
      continue;
    }
    const after = html[i + 1] ?? "";
    if (after === "!") {
      flush();
      out.push(COMMENT);
      i = afterDeclaration(html, i);
      continue;
    }
    if (after === "?") {
      flush();
      out.push(COMMENT);
      i = afterChar(html, i, ">");
      continue;
    }
    const closing = after === "/";
    const nameAt = closing ? i + 2 : i + 1;
    if (!isAlpha(html[nameAt] ?? "")) {
      if (!closing) {
        text += "<"; // opens nothing: it's text
        i++;
      } else {
        flush(); // "</>" or "</ x>": dropped
        out.push(COMMENT);
        i = afterChar(html, i, ">");
      }
      continue;
    }
    if (!closing) {
      const end = ahead.addressEnd(i);
      if (end >= 0) {
        flush();
        out.push({ kind: "literal", text: decoded(html.slice(i, end + 1), note, mayBeDrawn) });
        i = end + 1;
        continue;
      }
    }
    let j = nameAt;
    while (j < n && !isSpace(html[j]) && html[j] !== "/" && html[j] !== ">") j++;
    const name = asciiLower(html.slice(nameAt, j));
    // An end tag's attributes mean nothing, so neither do their references.
    const tag = readTag(html, j, closing ? UNHEARD : note);
    flush();
    if (tag === null) {
      out.push(CUT); // the text ends inside the tag: it and the rest are dropped
      return out;
    }
    if (readsAsWords(name, html.slice(j, tag.end - 1), closing)) {
      out.push({ kind: "literal", text: decoded(html.slice(i, tag.end), note, mayBeDrawn) });
      i = tag.end;
      continue;
    }
    if (closing) {
      out.push({ kind: "end", name }); // an end tag's attributes mean nothing
      i = tag.end;
      continue;
    }
    out.push({ kind: "start", name, attrs: tag.attrs, selfClosing: tag.selfClosing });
    i = tag.end;
    if (HIDDEN.has(name)) {
      out.push({ kind: "end", name });
      i = afterEndTag(html, i, name);
    }
  }
  flush();
  return out;
}

/** What lies ahead of a "<", found once per stretch of text so a run of "<a<a<a" stays linear: the
 *  next ">", the next whitespace and the next "@", each searched again only once passed. */
class Ahead {
  private gt = -1;
  private space = -1;
  private at = -1;
  constructor(private readonly s: string) {}

  /** Where an address in angle brackets that opens at `from` ends (its ">"), or -1: "<" then
   *  http:, https: or mailto: (any case), or a run holding "@", with no whitespace before the ">". */
  addressEnd(from: number): number {
    const s = this.s;
    if (this.gt <= from) this.gt = next(s, from + 1, (c) => c === ">");
    if (this.gt >= s.length) return -1;
    if (this.space <= from) this.space = next(s, from + 1, isSpace);
    if (this.space < this.gt) return -1;
    const head = s.slice(from + 1, from + 8).toLowerCase();
    if (head.startsWith("http:") || head.startsWith("https:") || head.startsWith("mailto:")) {
      return this.gt;
    }
    if (this.at <= from) this.at = next(s, from + 1, (c) => c === "@");
    return this.at < this.gt ? this.gt : -1;
  }
}

/** Whether a tag is words in angle brackets, as Markdown reads them, rather than an element. A browser
 *  would hide them as an unknown element, and Google's text can hold them unescaped. A name the reader
 *  knows is always an element, whatever follows it: a browser draws `<a href="…";>` as a link and
 *  `<b 4>` as bold, and skips `<style 1>`'s content. Any other name is an element only where Markdown
 *  reads a tag too (and drops it): a name of ASCII letters, digits, "-" or ":" (Outlook's `<o:p>`,
 *  which Markdown doesn't read but no one writes as words), and after it `rest`, all there is before
 *  the ">", as CommonMark allows. So `<Room>` and `<Room A>` are elements, and `<José>`, `<first.last>`,
 *  `<Room 4>` and `<Room A, B>` words. */
function readsAsWords(name: string, rest: string, closing: boolean): boolean {
  if (KNOWN.has(name)) return false;
  if (!/^[a-z0-9:-]+$/.test(name)) return true;
  if (BLANK_LINE.test(rest)) return true; // it ends Markdown's paragraph, so no tag holds one
  return closing ? afterGap(rest, 0) !== rest.length : !isTagRest(rest);
}

const BLANK_LINE = /\n[ \t]*\n/;
/** What CommonMark's tags allow between their parts: a space, a tab or a line ending. (A form feed
 *  ends a name in HTML, but not in CommonMark.) */
const isGapChar = (c: string) => c === " " || c === "\t" || c === "\n";

/** Past the spaces, tabs and line endings from `from`. */
function afterGap(s: string, from: number): number {
  let i = from;
  while (i < s.length && isGapChar(s[i])) i++;
  return i;
}

const isAttrStart = (c: string) => isAlpha(c) || c === "_" || c === ":";
const isAttrChar = (c: string) => isAttrStart(c) || isDigit(c) || c === "." || c === "-";

/** Whether what follows a tag's name, up to its ">", is CommonMark's: attributes each after a gap, a
 *  name `[A-Za-z_:][A-Za-z0-9:._-]*` with perhaps a value (quoted, or with none of whitespace, quotes,
 *  "=", "<", ">" or "`"), then perhaps a "/" right before the ">". In one pass, over text no other tag
 *  shares. */
function isTagRest(rest: string): boolean {
  let i = 0;
  for (;;) {
    const at = afterGap(rest, i);
    if (at === rest.length) return true;
    if (rest[at] === "/") return at + 1 === rest.length;
    if (at === i || !isAttrStart(rest[at])) return false;
    i = at + 1;
    while (i < rest.length && isAttrChar(rest[i])) i++;
    const equals = afterGap(rest, i);
    if (rest[equals] !== "=") continue;
    const value = afterGap(rest, equals + 1);
    const quote = rest[value];
    if (quote === '"' || quote === "'") {
      const close = rest.indexOf(quote, value + 1);
      if (close < 0) return false;
      i = close + 1;
    } else {
      i = value;
      while (i < rest.length && !isGapChar(rest[i]) && !"\"'=<>`".includes(rest[i])) i++;
      if (i === value) return false;
    }
  }
}

/** The first index from `from` whose character passes `test`, or the length. */
function next(s: string, from: number, test: (c: string) => boolean): number {
  let i = from;
  while (i < s.length && !test(s[i])) i++;
  return i;
}

/** A tag's attributes, read from just after its name as the HTML tokenizer reads them, and where the
 *  tag ends (just past its ">"); null if the text ends first. A quote opens a value only after "=",
 *  and hides any ">" inside it. */
function readTag(
  s: string,
  from: number,
  note: Note,
): { attrs: Attr[]; selfClosing: boolean; end: number } | null {
  const n = s.length;
  const attrs: Attr[] = [];
  const seen = new Set<string>();
  let slash = false; // the last character skipped was a "/", so a ">" now closes the element too
  let i = from;
  for (;;) {
    while (i < n && (isSpace(s[i]) || s[i] === "/")) {
      slash = s[i] === "/";
      i++;
    }
    if (i >= n) return null;
    if (s[i] === ">") return { attrs, selfClosing: slash, end: i + 1 };
    slash = false;
    // A name runs to whitespace, "/", ">" or "="; one that starts with "=" keeps it (HTML).
    let j = s[i] === "=" ? i + 1 : i;
    while (j < n && !isSpace(s[j]) && s[j] !== "/" && s[j] !== ">" && s[j] !== "=") j++;
    const name = asciiLower(s.slice(i, j));
    i = j;
    let k = i;
    while (k < n && isSpace(s[k])) k++;
    let value = "";
    if (s[k] === "=") {
      k++;
      while (k < n && isSpace(s[k])) k++;
      if (k >= n) return null;
      // Only what the reader keeps of a tag can change with a reference a browser reads otherwise:
      // an address or a style, its first writing. The rest is dropped, or refused anyway.
      const kept = (name === "href" || name === "style") && !seen.has(name);
      const heard = kept ? note : UNHEARD;
      const quote = s[k];
      if (quote === '"' || quote === "'") {
        const close = s.indexOf(quote, k + 1);
        if (close < 0) return null;
        value = decoded(s.slice(k + 1, close), heard, mayBeDrawnInValue);
        i = close + 1;
      } else {
        let e = k;
        while (e < n && !isSpace(s[e]) && s[e] !== ">") e++;
        value = decoded(s.slice(k, e), heard, mayBeDrawnInValue);
        i = e;
      }
    }
    // A repeated name keeps its first value, as browsers keep it.
    if (!seen.has(name)) {
      seen.add(name);
      attrs.push({ name, value });
    }
  }
}

/** `s` with its character references decoded once. */
function decoded(s: string, note: Note, drawn: (s: string, at: number) => boolean): string {
  let out = "";
  let i = 0;
  while (i < s.length) {
    const amp = s.indexOf("&", i);
    if (amp < 0) return out + s.slice(i);
    out += s.slice(i, amp);
    const [text, next] = reference(s, amp, note, drawn);
    out += text;
    i = next;
  }
  return out;
}

/** Just past the next `ch` from `from`, or the end. */
function afterChar(s: string, from: number, ch: string): number {
  const at = s.indexOf(ch, from);
  return at < 0 ? s.length : at + 1;
}

/** Past a comment or any other "<!…>" declaration, from its "<". A comment ends at "-->", at the
 *  tokenizer's "--!>", or abruptly at "<!-->" or "<!--->"; an unterminated one runs to the end. */
function afterDeclaration(s: string, from: number): number {
  if (!s.startsWith("<!--", from)) return afterChar(s, from, ">");
  if (s.startsWith("<!-->", from)) return from + 5;
  if (s.startsWith("<!--->", from)) return from + 6;
  let dashes = s.indexOf("--", from + 4);
  while (dashes >= 0) {
    if (s[dashes + 2] === ">") return dashes + 3;
    if (s[dashes + 2] === "!" && s[dashes + 3] === ">") return dashes + 4;
    dashes = s.indexOf("--", dashes + 1);
  }
  return s.length;
}

/** Past the end tag that closes a never-shown element, or the end if none does. */
function afterEndTag(s: string, from: number, name: string): number {
  let at = s.indexOf("</", from);
  while (at >= 0) {
    const after = at + 2 + name.length;
    const next = s[after] ?? "";
    if (
      asciiLower(s.slice(at + 2, after)) === name &&
      (next === "" || next === ">" || next === "/" || isSpace(next))
    ) {
      const tag = readTag(s, after, { unsure: false });
      return tag === null ? s.length : tag.end;
    }
    at = s.indexOf("</", at + 2);
  }
  return s.length;
}

/** A character reference at `from` (its "&"), decoded, and where the text goes on. Anything that
 *  isn't one is a plain "&"; `note` hears of one left as written that `drawn` says a browser may draw
 *  as a character. */
function reference(
  s: string,
  from: number,
  note: Note,
  drawn: (s: string, at: number) => boolean,
): [string, number] {
  if (s[from + 1] === "#") {
    const hex = s[from + 2] === "x" || s[from + 2] === "X";
    const digitsAt = from + (hex ? 3 : 2);
    let j = digitsAt;
    while (j < s.length && (hex ? isHex(s[j]) : isDigit(s[j]))) j++;
    if (j === digitsAt) return ["&", from + 1];
    // Leading zeros don't count; more significant digits than any code point needs is out of range.
    let k = digitsAt;
    while (k < j && s[k] === "0") k++;
    const code = j === k ? 0 : j - k > 8 ? -1 : parseInt(s.slice(k, j), hex ? 16 : 10);
    return [codePoint(code), s[j] === ";" ? j + 1 : j];
  }
  let j = from + 1;
  while (j < s.length && j - from <= MAX_NAME && (isAlpha(s[j]) || isDigit(s[j]))) j++;
  const named = s[j] === ";" ? NAMED.get(s.slice(from + 1, j)) : undefined;
  if (named !== undefined) return [named, j + 1];
  if (isAlpha(s[from + 1] ?? "") && drawn(s, from + 1)) note.unsure = true;
  return ["&", from + 1];
}

/** Whether a name PM leaves as written may be one a browser draws: a name with its ";" that isn't
 *  HTML 4's ("&check;", "&foo;"; HTML 5 knows over two thousand), or a run starting with a legacy
 *  name, which needs no ";" ("&copy 2026", "&notes"). Looks at most MAX_ANY_NAME characters ahead. */
function mayBeDrawn(s: string, at: number): boolean {
  for (let len = LEGACY_MIN; len <= LEGACY_MAX; len++) {
    if (LEGACY.has(s.slice(at, at + len))) return true;
  }
  return endsAsAName(s, at);
}

/** `mayBeDrawn` inside an attribute's value (`s`), under HTML's rule for one: a legacy name written
 *  without its ";" stays as written when "=" or a letter or digit follows it, so a link's
 *  "&region=eu", "&section=2" or "&copy=3" is what PM reads too; at the value's end, or before anything
 *  else, a browser draws it ("&copy", "&amp"). */
function mayBeDrawnInValue(s: string, at: number): boolean {
  for (let len = LEGACY_MAX; len >= LEGACY_MIN; len--) {
    if (LEGACY.has(s.slice(at, at + len))) {
      const after = s[at + len] ?? "";
      if (after !== "=" && !isAlpha(after) && !isDigit(after)) return true;
      break; // the longest name is the one a browser takes
    }
  }
  return endsAsAName(s, at);
}

/** A run of letters and digits from `at` with a ";" after it. Looks at most MAX_ANY_NAME ahead. */
function endsAsAName(s: string, at: number): boolean {
  let j = at;
  while (j < s.length && j - at < MAX_ANY_NAME && (isAlpha(s[j]) || isDigit(s[j]))) j++;
  return s[j] === ";";
}

/** A numeric reference's character, as a browser reads it: Windows-1252's for 0x80-0x9F, and U+FFFD
 *  for nothing, a surrogate or past Unicode's end. */
function codePoint(code: number): string {
  const valid = code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
  return String.fromCodePoint(valid ? (C1.get(code) ?? code) : 0xfffd);
}
