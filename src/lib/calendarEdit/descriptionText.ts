// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The text of an event description Google holds as HTML, for the editor to show read-only (#884).
//
// A character scanner, not the browser's parser: nothing here hands the description to DOMParser,
// innerHTML or a <template>, and the result is only ever shown as a React text child, so no markup in
// a description can run, load or render. (CodeQL rightly treats any HTML parse of text that can come
// from a text field as a sink; an inert parse was still a parse.)
//
// It reads markup as the HTML tokenizer does wherever that changes what shows: line endings are
// normalised, tags and comments vanish, a ">" inside a quoted attribute doesn't end its tag, a tag or
// comment the text ends inside takes the rest with it, and a "<" that can't open a tag is text. The
// content of <script>, <style> and the other never-shown elements doesn't show. A character reference
// is decoded once, onto the output and never back into the scanner, so "&lt;b&gt;" shows as "<b>" and
// is never read as a tag.
//
// Lines break where a browser would draw them: at <br>, and at the edges of paragraphs, blocks,
// headings, lists and table rows (once, however many edges meet); table cells get a space between
// them. One pass that never steps back, so a hostile description costs linear time.

/** Elements whose content never shows: skipped to their end tag, or to the end if nothing ends them. */
const HIDDEN = new Set([
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
const BLOCKS = new Set([
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
 *  HTML 4's names with the ";", Google included. */
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

const isSpace = (c: string) => c === " " || c === "\n" || c === "\t" || c === "\f" || c === "\r";
const isAlpha = (c: string) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
const isDigit = (c: string) => c >= "0" && c <= "9";
const isHex = (c: string) => isDigit(c) || (c >= "a" && c <= "f") || (c >= "A" && c <= "F");

/** The readable text of an HTML description. */
export function descriptionText(description: string): string {
  // The tokenizer reads CR LF and a lone CR as LF before anything else.
  const html = description.replace(/\r\n?/g, "\n");
  const n = html.length;
  let out = "";
  // The space owed after a table cell: written only when more text follows on the same line.
  let cellGap = false;
  const put = (text: string) => {
    if (cellGap && !isSpace(text.slice(0, 1))) out += " "; // whitespace in the source is gap enough
    cellGap = false;
    out += text;
  };
  const newLine = () => {
    cellGap = false;
    if (out !== "" && !out.endsWith("\n")) out += "\n";
  };
  let i = 0;
  while (i < n) {
    const c = html[i];
    if (c === "&") {
      const [text, next] = reference(html, i);
      put(text);
      i = next;
      continue;
    }
    if (c !== "<") {
      put(c);
      i++;
      continue;
    }
    const after = html[i + 1] ?? "";
    if (after === "!") {
      i = afterDeclaration(html, i);
      continue;
    }
    if (after === "?") {
      i = afterChar(html, i, ">");
      continue;
    }
    const closing = after === "/";
    const nameAt = closing ? i + 2 : i + 1;
    if (!isAlpha(html[nameAt] ?? "")) {
      if (!closing) {
        put("<"); // opens nothing: it's text
        i++;
      } else {
        i = afterChar(html, i, ">"); // "</>" or "</ x>": dropped
      }
      continue;
    }
    let j = nameAt;
    while (j < n && !isSpace(html[j]) && html[j] !== "/" && html[j] !== ">") j++;
    const name = html.slice(nameAt, j).toLowerCase();
    const end = tagEnd(html, j);
    if (end < 0) break; // the text ends inside the tag: it and the rest are dropped
    i = end;
    if (!closing && HIDDEN.has(name)) {
      i = afterEndTag(html, i, name);
    } else if (name === "br") {
      cellGap = false;
      out += "\n";
    } else if (BLOCKS.has(name)) {
      newLine();
    } else if (closing && (name === "td" || name === "th")) {
      cellGap = out !== "" && !isSpace(out.slice(-1));
    }
  }
  return out.replace(/\n{3,}/g, "\n\n").trim();
}

/** Where a tag ends (just past its ">"), from just after its name, or -1 if the text ends first. A
 *  quote opens an attribute value only after "=", as in the tokenizer, and hides any ">" inside it. */
function tagEnd(s: string, from: number): number {
  let afterEquals = false;
  let i = from;
  while (i < s.length) {
    const c = s[i];
    if (c === ">") return i + 1;
    if ((c === '"' || c === "'") && afterEquals) {
      const close = s.indexOf(c, i + 1);
      if (close < 0) return -1;
      i = close + 1;
      afterEquals = false;
      continue;
    }
    if (c === "=") afterEquals = true;
    else if (!isSpace(c)) afterEquals = false;
    i++;
  }
  return -1;
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
      s.slice(at + 2, after).toLowerCase() === name &&
      (next === "" || next === ">" || next === "/" || isSpace(next))
    ) {
      const end = tagEnd(s, after);
      return end < 0 ? s.length : end;
    }
    at = s.indexOf("</", at + 2);
  }
  return s.length;
}

/** A character reference at `from` (its "&"), decoded, and where the text goes on. Anything that
 *  isn't one is a plain "&". */
function reference(s: string, from: number): [string, number] {
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
  return named === undefined ? ["&", from + 1] : [named, j + 1];
}

/** A numeric reference's character, as a browser reads it: Windows-1252's for 0x80-0x9F, and U+FFFD
 *  for nothing, a surrogate or past Unicode's end. */
function codePoint(code: number): string {
  const valid = code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
  return String.fromCodePoint(valid ? (C1.get(code) ?? code) : 0xfffd);
}
