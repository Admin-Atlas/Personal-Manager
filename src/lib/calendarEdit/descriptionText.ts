// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The text of an event description Google holds as HTML, for the editor to show read-only (#884).
//
// Read by descriptionTokens.ts's scanner, not the browser's parser, and only ever shown as a React text
// child, so no markup in a description can run, load or render. Tags and comments vanish, the content
// of <script>, <style> and the other never-shown elements doesn't show, and a character reference is
// decoded once ("&lt;b&gt;" shows as "<b>"). An address in angle brackets ("<https://…>") is words.
//
// Lines break where a browser would draw them: at <br>, and at the edges of paragraphs, blocks,
// headings, lists and table rows (once, however many edges meet); table cells get a space between
// them.

import { BLOCKS, isSpace, tokens } from "./descriptionTokens";

/** The readable text of an HTML description. */
export function descriptionText(description: string): string {
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
  for (const token of tokens(description)) {
    if (token.kind === "cut") break; // the text ends inside a tag: it and the rest are dropped
    if (token.kind === "text" || token.kind === "literal") {
      put(token.text);
    } else if (token.kind === "start" || token.kind === "end") {
      if (token.name === "br") {
        // Browsers read "</br>" as "<br>".
        cellGap = false;
        out += "\n";
      } else if (BLOCKS.has(token.name)) {
        newLine();
      } else if (token.kind === "end" && (token.name === "td" || token.name === "th")) {
        cellGap = out !== "" && !isSpace(out.slice(-1));
      }
      // Anything else, a never-shown element's start and end included, shows nothing itself.
    }
  }
  return out.replace(/\n{3,}/g, "\n\n").trim();
}
