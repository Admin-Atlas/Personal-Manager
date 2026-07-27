// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The `@tag` grammar, which exists in two places: here and in `tags::parse_mentions` (Rust). The
// backend decides the retrieval scope; this side decides what to suggest and what to highlight. If
// the two disagree, the app shows a pin it did not apply — or applies one it did not show. Each case
// below is a rule the Rust tests assert too.

import { describe, expect, it } from "vitest";
import { completeMention, mentionAtCaret, parseMentions, splitMentions } from "./mentions";

describe("parseMentions", () => {
  it("finds a mention that begins a word", () => {
    expect(parseMentions("ask @Marketing about it")).toEqual(["Marketing"]);
    expect(parseMentions("@Ops first")).toEqual(["Ops"]);
    expect(parseMentions("(@Ops) handled it")).toEqual(["Ops"]);
  });

  it("does not read an email address as a mention", () => {
    expect(parseMentions("mail bob@example.com")).toEqual([]);
  });

  it("drops trailing sentence punctuation but keeps the name", () => {
    expect(parseMentions("see @Atlas, then go")).toEqual(["Atlas"]);
    expect(parseMentions("done with @Ops.")).toEqual(["Ops"]);
  });

  // Without the quoted form, the project names #275 went out of its way to allow — the ones with a
  // comma and a space — would be exactly the ones that could never be pinned.
  it("reads a quoted name as one mention, spaces and all", () => {
    expect(parseMentions('see @"Atlas, Inc." later')).toEqual(["Atlas, Inc."]);
  });

  it("ignores a quote the user is still typing", () => {
    expect(parseMentions('see @"Atlas, Inc')).toEqual([]);
  });

  it("deduplicates case-insensitively and keeps the first spelling", () => {
    expect(parseMentions("@ops and @OPS")).toEqual(["ops"]);
  });
});

describe("mentionAtCaret", () => {
  it("finds the token being typed, and nothing once it is closed", () => {
    const text = "ask @mark";
    expect(mentionAtCaret(text, text.length)).toEqual({
      start: 4,
      end: 9,
      query: "mark",
      quoted: false,
    });
    // A space ends the token, so the suggestion list closes rather than following the caret on.
    expect(mentionAtCaret("ask @marketing now", 18)).toBeNull();
  });

  it("keeps following the token inside an open quote, where a space is part of the name", () => {
    const text = 'ask @"Atlas, In';
    const at = mentionAtCaret(text, text.length);
    expect(at?.quoted).toBe(true);
    expect(at?.query).toBe("Atlas, In");
  });

  it("offers nothing for an address", () => {
    expect(mentionAtCaret("bob@exa", 7)).toBeNull();
  });
});

describe("completeMention", () => {
  it("inserts the tag and leaves the caret past a closing space", () => {
    const text = "ask @mark";
    const at = mentionAtCaret(text, text.length)!;
    expect(completeMention(text, at, "Marketing")).toEqual({
      text: "ask @Marketing ",
      caret: 15,
    });
  });

  // A bare insert here would be parsed as `@Atlas,` — the completion has to produce something the
  // parser reads back as the tag it just offered.
  it("quotes a name containing a space", () => {
    const text = "ask @atl";
    const at = mentionAtCaret(text, text.length)!;
    expect(completeMention(text, at, "Atlas, Inc.").text).toBe('ask @"Atlas, Inc." ');
  });
});

describe("splitMentions", () => {
  it("marks only the mentions that match a real tag", () => {
    const segs = splitMentions("@Marketing and @markting", ["Marketing"]);
    expect(segs.filter((s) => s.tag)).toEqual([{ text: "@Marketing", tag: "Marketing" }]);
    expect(segs.map((s) => s.text).join("")).toBe("@Marketing and @markting");
  });

  it("matches case-insensitively but reports the canonical name", () => {
    const segs = splitMentions("ask @marketing", ["Marketing"]);
    expect(segs.find((s) => s.tag)?.tag).toBe("Marketing");
  });

  it("leaves an ordinary message as a single run", () => {
    expect(splitMentions("no mentions here", ["Marketing"])).toEqual([
      { text: "no mentions here" },
    ]);
  });

  it("never loses or duplicates a character", () => {
    const text = 'start @Ops middle @"Atlas, Inc." end@notamention';
    const segs = splitMentions(text, ["Ops", "Atlas, Inc."]);
    expect(segs.map((s) => s.text).join("")).toBe(text);
  });
});
