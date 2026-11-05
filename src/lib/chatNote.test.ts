// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { chatNoteId } from "./chatNote";

// The backend's note-id rule (`ingest_note_document`): non-empty, ASCII letters, digits, `-`, `_`.
const BACKEND_NOTE_ID = /^[A-Za-z0-9_-]+$/;

describe("chatNoteId", () => {
  it("passes the backend's note-id check", () => {
    // `chat:<id>` failed this check, so every "Save as note" errored.
    for (const id of [1, 42, 9_007_199_254_740_991]) {
      expect(chatNoteId(id)).toMatch(BACKEND_NOTE_ID);
    }
  });

  it("is stable per message and never collides with a pinboard note id", () => {
    expect(chatNoteId(7)).toBe("chat-7");
    expect(chatNoteId(7)).toBe(chatNoteId(7));
    expect(chatNoteId(7)).not.toBe(chatNoteId(8));
    // Pinboard ids are UUIDs or `w-…`; neither starts with `chat-`.
    expect(chatNoteId(7).startsWith("chat-")).toBe(true);
  });
});
