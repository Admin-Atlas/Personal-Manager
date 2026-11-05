// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The note id Chat's "Save as note" files a saved answer under.
//
// Saved answers reuse the pinboard's note-ingest path, which names the vault file after the id and
// refuses any id outside `[A-Za-z0-9_-]` (`ingest_note_document` in ingest.rs). The `chat-` prefix
// keeps saved answers apart from pinboard notes, whose ids are UUIDs or `w-…`, and the message id
// makes a second save of the same answer update it in place instead of duplicating it. Message ids
// are AUTOINCREMENT, so a deleted chat's id is never handed to a new answer.

/** The note id for a saved chat answer. */
export function chatNoteId(messageId: number): string {
  return `chat-${messageId}`;
}
