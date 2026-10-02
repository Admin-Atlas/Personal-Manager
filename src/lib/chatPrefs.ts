// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device view prefs for chat: which of the Chats tab sidebar's two sections ("Projects" and
// "Global chats") the user has folded away, and the composer's Thinking toggle. The folds are
// display-only with no backend consumer, so localStorage, never a backend Setting (mirrors
// focusPrefs / mapPrefs).
//
// The chat Thinking toggle is the one pref here with a backend reader. Like Developer mode's
// `capturePrompt`, it travels with each send (`sendMessage(..., showThinking)`). The backend never
// stores it, so it stays per-device view state in localStorage, not a backend Setting.
//
// The stored value is deliberately TRI-STATE: absent means "never chosen", not "open". The two
// sections seed their initial state from density (`!minimal` for Projects), so a plain boolean
// default would freeze the Depth preset out on first run. An explicit click wins from then on —
// that is the "explicit beats derived" reading, and it does mean switching to Minimal later no
// longer re-collapses a section the user has opened by hand.
//
// Writers announce on the app-wide `pm:settings-changed` signal, for the same reason focusPrefs
// does: Settings renders as an overlay over a still-mounted Sidebar, so a read taken at mount would
// go stale rather than follow the other side's writes.

export type ChatSectionId = "projects" | "global";

const SECTIONS_KEY = "pm.chats.sections";
const CHANGED_EVENT = "pm:settings-changed";

function announce(): void {
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

/** The whole stored record, or `{}` when absent/corrupt. */
function readAll(): Partial<Record<ChatSectionId, boolean>> {
  try {
    const raw = localStorage.getItem(SECTIONS_KEY);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      // Arrays are objects too, hence the explicit reject — a stale value from another key shape
      // must degrade to "never chosen" rather than throwing on the read.
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        const out: Partial<Record<ChatSectionId, boolean>> = {};
        for (const id of ["projects", "global"] as const) {
          const v = (parsed as Record<string, unknown>)[id];
          if (typeof v === "boolean") out[id] = v;
        }
        return out;
      }
    }
  } catch {
    /* never chosen */
  }
  return {};
}

/** The user's explicit fold choice, or `null` when they have never made one (caller keeps its
 *  density-derived seed). */
export function readChatSectionOpen(id: ChatSectionId): boolean | null {
  return readAll()[id] ?? null;
}

export function writeChatSectionOpen(id: ChatSectionId, open: boolean): void {
  try {
    localStorage.setItem(SECTIONS_KEY, JSON.stringify({ ...readAll(), [id]: open }));
  } catch {
    /* best-effort */
  }
  announce();
}

/** True when the user has never folded either section by hand. */
export function chatSectionsAreDefault(): boolean {
  return Object.keys(readAll()).length === 0;
}

/** Back to density-derived folding — one key, so this is a single remove. */
export function resetChatSections(): void {
  try {
    localStorage.removeItem(SECTIONS_KEY);
  } catch {
    /* best-effort */
  }
  announce();
}

// The Thinking toggle. Stored as "1" when on; turning it off REMOVES the key, so absent is the
// default and a reset is a single remove. Any other value, or storage that throws, reads as off —
// the off state is #852's fast path, so every doubt lands there.
const SHOW_THINKING_KEY = "pm.chat.showThinking";

/** Whether chat should ask the local model to think and stream its thinking. Off unless chosen. */
export function readShowThinking(): boolean {
  try {
    return localStorage.getItem(SHOW_THINKING_KEY) === "1";
  } catch {
    return false;
  }
}

export function writeShowThinking(on: boolean): void {
  try {
    if (on) localStorage.setItem(SHOW_THINKING_KEY, "1");
    else localStorage.removeItem(SHOW_THINKING_KEY);
  } catch {
    /* best-effort */
  }
  announce();
}

/** True when the Thinking toggle is at its default (off). */
export function showThinkingIsDefault(): boolean {
  return !readShowThinking();
}

/** Back to off — the default is the absent key. */
export function resetShowThinking(): void {
  try {
    localStorage.removeItem(SHOW_THINKING_KEY);
  } catch {
    /* best-effort */
  }
  announce();
}
