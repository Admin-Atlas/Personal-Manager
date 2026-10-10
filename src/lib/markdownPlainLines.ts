// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Plain text typed line by line (an event's description, #884), read so its lines mean what they look
// like. Once a description keeps its line breaks, two Markdown constructs start to misread ordinary
// typing: a line of only "-" or "=" under some text is a setext underline that turns the lines above
// into a heading (a pasted invite's "---" under its dial-in details becomes an H2), and a line
// indented four spaces becomes a code block. Both are switched off in the parser for this surface
// only; everything else still reads as Markdown.
//
// A parser setting (micromark constructs turned off), so like every remark plugin it runs upstream of
// the whole rehype chain and of the sanitizer, and can't put anything past it.

/** The remark plugin: turns off micromark's setext underline and indented code. */
export function remarkPlainLines(this: unknown) {
  const data = (this as { data(): Record<string, unknown> }).data() as {
    micromarkExtensions?: unknown[];
  };
  (data.micromarkExtensions ??= []).push({
    disable: { null: ["setextUnderline", "codeIndented"] },
  });
}
