// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small "this came from a conversation" marker for documents with source_type === "chat" (epic 7).
// A chat reads differently from a filed file, so it's labelled wherever documents are listed — the
// Review queue and a project's Files. `compact` renders just the glyph (for tight list rows); the
// default renders a pill, matching the SourceBadge idiom in DocumentsView.

const TITLE = "This is a conversation — it flows into sorting like any other source";

export function ChatBadge({ compact = false }: { compact?: boolean }) {
  if (compact) {
    return (
      <span className="shrink-0 text-ink4" title={TITLE} aria-label="From a conversation">
        💬
      </span>
    );
  }
  return (
    <span
      className="inline-flex shrink-0 items-center gap-1 rounded-full bg-accent-soft px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-accent-text"
      title={TITLE}
    >
      💬 Chat
    </span>
  );
}
