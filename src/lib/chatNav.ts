// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Splitting the sidebar's flat conversation roster into the Chats tab's two sections.
//
// A conversation's scope is one nullable field (`Conversation.project`) and `list_conversations`
// returns every conversation with no WHERE clause, so both sections come out of the array the
// sidebar already has — no new backend command, no second round trip.
//
// Kept pure and in its own module (like `chatSession.ts`) so the ordering and counting rules are
// unit-testable without standing up the sidebar.

import type { Conversation } from "./types";

/** One row of the Projects section: a project and the chats scoped to it, newest activity first. */
export interface ProjectChats {
  project: string;
  chats: Conversation[];
}

/** Conversations with no project — the "Global chats" section, in the order the backend returned. */
export function globalChats(conversations: Conversation[]): Conversation[] {
  return conversations.filter((c) => c.project == null || c.project === "");
}

/**
 * Group scoped conversations by project, unioned with `known` — the project list from the backend,
 * which is the only way a project with no chats yet appears at all.
 *
 * The reverse union matters too: `list_projects` is `SELECT DISTINCT project FROM documents`, so a
 * project that has chats but no ingested document is missing from it. The sidebar's move dialog
 * already compensates for that same gap the same way. Taking the union in both directions is what
 * makes this list complete without a new query.
 *
 * Sorted by project name (case-insensitive) so the section is stable as chats come and go, rather
 * than reshuffling on every send.
 */
export function projectChats(conversations: Conversation[], known: string[]): ProjectChats[] {
  const byProject = new Map<string, Conversation[]>();
  for (const name of known) {
    if (name) byProject.set(name, []);
  }
  for (const c of conversations) {
    const p = c.project;
    if (p == null || p === "") continue;
    const existing = byProject.get(p);
    if (existing) existing.push(c);
    else byProject.set(p, [c]);
  }
  return [...byProject.entries()]
    .map(([project, chats]) => ({ project, chats }))
    .sort((a, b) => a.project.localeCompare(b.project, undefined, { sensitivity: "base" }));
}
