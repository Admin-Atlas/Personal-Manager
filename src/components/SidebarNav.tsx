// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's tab list, drawn from lib/nav: the entries, their order and which ones show all come
// from there, so this file only says how a tab LOOKS. Two tabs carry something extra: Chats has its
// "+ New" beside it, and Review its count of documents waiting.
//
// Right-click a tab for Pin / Unpin, Hide, and a Show for each tab that isn't there; Focus can't be
// hidden, so there is always a tab to right-click. One handler on the list, not one per tab: the tab
// under the pointer is found from `data-nav-id`, which also covers Chats' "+ New", a button of its
// own beside the tab.

import type { MouseEvent as ReactMouseEvent } from "react";
import { openContextMenu, wantsAppMenu } from "../lib/contextMenu";
import { isActiveEntry, navMenuItems, useNav, writeNavPlacement, type View } from "../lib/nav";
import { NavItem } from "./ui";

interface Props {
  view: View;
  onNavigate: (view: View) => void;
  reviewCount: number;
  /** Start a new GLOBAL conversation (see Sidebar's `onNew`). */
  onNew: () => void;
}

export function SidebarNav({ view, onNavigate, reviewCount, onNew }: Props) {
  const { entries, placement, ctx, resolved } = useNav();

  function onMenu(e: ReactMouseEvent) {
    if (!wantsAppMenu(e)) return;
    const row = e.target instanceof Element ? e.target.closest<HTMLElement>("[data-nav-id]") : null;
    const id = row?.dataset.navId ?? null;
    const entry = resolved.shown.find((x) => x.id === id);
    openContextMenu(
      e,
      entry ? `${entry.label} tab` : "Sidebar tabs",
      navMenuItems(id, entries, placement, ctx, writeNavPlacement),
    );
  }

  return (
    <nav className="flex flex-col gap-1 px-2 pb-2" onContextMenu={onMenu}>
      {resolved.shown.map((entry) => {
        const item = (
          <NavItem
            active={isActiveEntry(entry, view)}
            onClick={() => onNavigate(entry.id)}
            helpId={entry.helpId}
            trailing={entry.id === "review" ? <CountBadge count={reviewCount} /> : undefined}
            className={entry.id === "chat" ? "min-w-0 flex-1" : undefined}
          >
            {entry.label}
          </NavItem>
        );
        if (entry.id !== "chat") {
          return (
            <div key={entry.id} data-nav-id={entry.id}>
              {item}
            </div>
          );
        }
        // New sits on the Chats row because that is what it makes — not in the chrome above,
        // where it had no subject and cost the whole sidebar a line. A SIBLING of NavItem, never
        // its `trailing` slot: NavItem is itself a <button>, so a button inside it would be
        // invalid markup and every New click would also fire the navigate underneath.
        //
        // It makes a GLOBAL chat from every tab, including while a project is open. A button on
        // the Chats row reads as "new chat in Chats" no matter what is on screen behind it, so
        // scoping it to the open project silently — as its predecessor did — meant the same
        // control made two different things with nothing to tell them apart. A project's own
        // New sits over that project's conversation list below.
        return (
          <div key={entry.id} data-nav-id={entry.id} className="flex items-center gap-1">
            {item}
            <button
              onClick={onNew}
              title="New conversation"
              aria-label="New conversation"
              className="shrink-0 rounded-[var(--radius-sm)] px-1.5 py-1.5 text-xs text-ink4 transition hover:bg-surface hover:text-ink"
            >
              + New
            </button>
          </div>
        );
      })}
    </nav>
  );
}

/** The pill count shown on a nav row (e.g. pending reviews); renders nothing at zero. */
function CountBadge({ count }: { count?: number }) {
  if (count == null || count <= 0) return null;
  return (
    <span className="ml-2 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 font-mono text-xs font-medium text-accent-text">
      {count}
    </span>
  );
}
