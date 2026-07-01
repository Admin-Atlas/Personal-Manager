// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Conversation } from "../lib/types";
import { formatDate } from "../lib/format";
import { NavItem } from "./ui";

interface Props {
  /** This project's conversations, newest-first (already filtered + ordered upstream). */
  conversations: Conversation[];
  /** The conversation on screen, or null on a fresh unsent chat. */
  activeId: number | null;
  onSelect: (id: number) => void;
  onNew: () => void;
}

/** The top panel of a project's sidebar (board card 7E): its past chats, newest-first, with a
 *  "+ New chat" action. Clicking a row resumes that conversation in the main pane (a clean swap —
 *  the current chat is already persisted and shows here). Mirrors the global sidebar's row idiom. */
export function ChatHistoryList({ conversations, activeId, onSelect, onNew }: Props) {
  return (
    <div className="flex h-full flex-col" data-help="project-chat-history">
      <div className="flex items-center justify-between px-4 pb-1 pt-3">
        <span className="font-mono text-xs uppercase tracking-wide text-ink4">Chats</span>
        <button
          type="button"
          onClick={onNew}
          title="Start a new chat (also: type /new)"
          className="rounded-[var(--radius-sm)] px-2 py-0.5 text-xs text-ink3 hover:bg-surface hover:text-ink"
        >
          + New chat
        </button>
      </div>
      {conversations.length === 0 ? (
        <p className="px-4 py-2 text-xs text-ink4">No chats in this project yet.</p>
      ) : (
        <ul className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-2">
          {conversations.map((c) => (
            <li key={c.id}>
              <NavItem
                active={c.id === activeId}
                onClick={() => onSelect(c.id)}
                trailing={
                  <span className="font-mono text-[10px] text-faint">
                    {formatDate(c.updated_at)}
                  </span>
                }
              >
                <span title={c.title}>{c.title}</span>
              </NavItem>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
