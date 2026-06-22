// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createContext, useContext } from "react";

/** One section's plain-language explanation, shown by the HelpOverlay (Step 4b). */
export interface HelpEntry {
  title: string;
  body: string;
}

/**
 * The help registry: every `data-help="<id>"` in the UI maps to an entry here.
 * Keep these short and jargon-free — the point is to let the user learn what each
 * part of PM does just by hovering it with help mode on.
 */
export const HELP: Record<string, HelpEntry> = {
  // Sidebar / navigation
  "nav-focus": {
    title: "Focus",
    body: "Your home screen: every active project on one page, each with a single status that answers 'should I look at this now?' — Due soon, Quick win, Take a look, Blocked, Part of, or On track. Click a project to narrow everything to just it.",
  },
  "nav-chat": {
    title: "Chat",
    body: "Ask questions in plain language. PM answers using your own documents and shows which files it drew from, so you can trust and trace every answer.",
  },
  "nav-documents": {
    title: "Documents",
    body: "Your ingested files. Drag files or a folder in and PM converts them to Markdown, splits them into chunks, and indexes them so they become searchable.",
  },
  "nav-review": {
    title: "Review",
    body: "The sorting queue. For each new document PM proposes a project, tags, and an importance level — you approve or correct them. Your corrections teach PM how you organise (see Learning You in Settings).",
  },
  "nav-graph": {
    title: "Map",
    body: "A visual graph of your documents grouped by project. Each project is a hub; the documents around it are its files. Hover any document node to see its details.",
  },
  "conversations-list": {
    title: "Conversations",
    body: "Your past chats. Click one to reopen it; '+ New' starts a fresh conversation.",
  },
  "sidebar-models": {
    title: "Models in use",
    body: "The models PM is currently using — 'Chat' for your conversations and 'Tasks' for background work (sorting and learning). A '+N' badge means auto-switch is on with N fallback models behind it. Click to change them in Settings.",
  },
  "sidebar-search": {
    title: "Search",
    body: "Opens the command palette — a quick-jump to any project, file, or past conversation. You can also open it anytime with Ctrl+K (⌘K on a Mac).",
  },

  // Command palette (Step 5b)
  "command-palette": {
    title: "Command palette",
    body: "Type to jump anywhere: a project opens its focus view, a file opens its project with the file highlighted, a conversation reopens it, and 'Go to' jumps to a section. Use ↑↓ to move, Enter to open, Esc to close.",
  },

  // Documents view
  "documents-dropzone": {
    title: "Add documents",
    body: "Drop files or a folder here (or use the buttons). PM converts each to Markdown and saves it in your local vault — so once ingested, you can safely delete the original file.",
  },
  "documents-rebuild": {
    title: "Rebuild",
    body: "Deletes the search index and rebuilds it from the Markdown vault. Proves your data is reconstructable from disk; safe to run anytime.",
  },
  "documents-review-banner": {
    title: "Documents to review",
    body: "How many ingested documents are still waiting for you to confirm their project, tags, and importance. Click to jump to the Review queue.",
  },

  // Review view
  "review-repropose": {
    title: "Re-propose",
    body: "Re-runs the AI's suggestions for everything in the queue — handy after you've created new projects you'd like it to consider.",
  },
  "review-approve-all": {
    title: "Approve all",
    body: "Commits every item in the queue with its current values. Anything you changed from the AI's suggestion is logged to teach PM your preferences.",
  },
  "review-autofiled": {
    title: "Auto-filed",
    body: "Low-importance items are collapsed here so they don't pile up as a chore. They're still committed when you approve, and stay fully searchable.",
  },
  "review-row": {
    title: "Reviewing a document",
    body: "Set the project (start typing to reuse an existing one), pick an importance, and add short tags. The grey line is the AI's reasoning for its suggestion.",
  },

  // Chat
  "chat-composer": {
    title: "Message box",
    body: "Type a question and press Enter. PM retrieves the most relevant chunks of your documents and answers from them.",
  },
  "composer-mic": {
    title: "Voice input",
    body: "Click to dictate instead of type. PM records a short clip, transcribes it right here on your device (nothing is sent to the cloud), and drops the text into the box so you can edit it before sending. The first use downloads the voice model once.",
  },
  "chat-sources": {
    title: "Sources",
    body: "The documents this answer was grounded in. They make the answer verifiable — open the file to check.",
  },

  // Settings
  "settings-api-key": {
    title: "OpenRouter API key",
    body: "Powers chat. Stored only in your operating system's keychain — never on disk or in the code. Get one at openrouter.ai/keys.",
  },
  "settings-background-key": {
    title: "Background API key",
    body: "An optional second key used for behind-the-scenes work (sorting suggestions and learning), so you can track that spend separately. Falls back to your main key if left blank.",
  },
  "settings-chat-models": {
    title: "Chat model",
    body: "Which model answers your chats. Add a model from the picker — search every model on OpenRouter, compare the input → output price per million tokens, sort by price, and use the tags (Free, Reasoning, Vision, Coding…). Add more than one and turn on auto-switch to fall through to the next when one hits its daily limit. PM is never locked to one model.",
  },
  "settings-background-models": {
    title: "Background model",
    body: "Which model does PM's behind-the-scenes work — the sorting proposals in Review and the Learning You profile. It can differ from your chat model; free models work well here. Chain several with auto-switch so background work keeps going when a free model's daily cap is reached.",
  },
  "settings-learning": {
    title: "Learning You",
    body: "A short, readable profile of how you organise — distilled from the corrections you make in Review. PM injects it into its suggestions and chat so it gets more like you over time. 'Refresh now' rebuilds it from your latest corrections.",
  },
  "settings-appearance": {
    title: "Appearance",
    body: "Switch the visual System (Editorial / Slate / Terminal), Light or Dark mode, the accent colour, and Depth (how much detail is shown). Changes apply instantly and are remembered on this device.",
  },
  "settings-timezone": {
    title: "Time zone",
    body: "The zone PM reasons about dates in — which day is 'today', when something is 'due soon', and the times in your calendar agenda and briefing. Auto follows this device; switch to Manual to pin a specific zone (useful when you travel and don't want the boundaries to shift).",
  },
  "settings-recommended-models": {
    title: "Recommended models",
    body: "PM's sensible defaults — a capable model for chat, and cheaper/faster models for background work (sorting and learning). Click to fill the lists above with the recommendation; nothing changes until you Save, and you can still edit them.",
  },
  "settings-usage-cost": {
    title: "Usage & cost",
    body: "What you've spent on model calls, priced from OpenRouter's public rates (refreshed about once a day). Shows last-30-days and all-time totals; expand 'How this is calculated' for the method and a per-model spend ranking. 'Refresh prices' re-pulls the latest rates. Costs are computed from token counts at read time, so a price change re-prices your history.",
  },
  "settings-help-mode": {
    title: "Help mode",
    body: "The switch you're using right now. While it's on, hovering any highlighted section shows an explanation like this one.",
  },
  "settings-calendar": {
    title: "Calendar",
    body: "Connect a calendar (read-only) so PM can show your agenda, answer schedule questions in chat, and flip a project to 'Due soon' when an event names it. Two ways: paste a calendar's private iCal feed URL (simplest — no sign-in, works with Advanced Protection), or use Google sign-in with your own OAuth credentials (advanced). Feed URLs and tokens live only in your keychain.",
  },
  "settings-license": {
    title: "License",
    body: "PM is free and open-source software under the GNU AGPL v3 — you're free to use, study, share, and modify it, and any networked version must offer its source. The link opens the full project source on GitHub.",
  },

  // Graph
  "graph-canvas": {
    title: "Document map",
    body: "Each large labelled node is a project; the smaller nodes linked to it are its documents. Bigger document nodes have more content. Hover a document to see its full details.",
  },

  // Focus view & projects (Step 5)
  "focus-header": {
    title: "Focus",
    body: "Your home screen. It gathers every active project onto one page so you can see the whole picture at a glance and pick the one thing to do next, instead of holding it all in your head.",
  },
  "focus-cards": {
    title: "Your projects",
    body: "Every active project, sorted with the most pressing first. Each row carries one status that answers 'should I look at this now?'. Hover a project, its status, or its Triage panel for more.",
  },
  "focus-briefing": {
    title: "Today's briefing",
    body: "A short, AI-written summary of where to focus today — drawn from your due-soon projects, upcoming calendar, quick wins, and anything that's gone quiet. It refreshes itself about once a day; hit Refresh to rebuild it now from your current state.",
  },
  "focus-suggest": {
    title: "Suggest attributes",
    body: "Asks the AI to propose a size, a parent project, a blocker, and (if your documents mention one) a deadline for each project. Nothing is applied until you confirm it in a project's Triage panel — AI proposes, you decide.",
  },
  "focus-agenda": {
    title: "Upcoming",
    body: "Your next events from the calendars you connected, read-only. PM also uses these to answer schedule questions in chat and to flag a project 'Due soon' when an event's title names it. Connect or sync in Settings → Google Calendar.",
  },
  "focus-card": {
    title: "Project",
    body: "One project and its current status. The line beneath shows what drives that status — document count, importance, size, deadline, blocker, last activity. Click the name to open it; click Triage to set its attributes.",
  },
  "focus-status-badge": {
    title: "Status",
    body: "The one thing this project is telling you. Due soon = a deadline is near. Quick win = small enough to finish fast. Take a look = it's gone quiet. Blocked = waiting on another project. Part of = a piece of a bigger project. On track = nothing needed now.",
  },
  "focus-triage": {
    title: "Triage a project",
    body: "Set a size (a 'quick' project becomes a Quick win), a deadline (drives Due soon), a blocker (drives Blocked), or a parent (drives Part of). If the AI suggested values, 'Use it' fills the form so you can confirm or tweak before saving.",
  },
  "project-chat": {
    title: "Project chat",
    body: "A chat that only draws on this project's documents, so the rest of your knowledge falls away while you focus. Ask anything about just this project.",
  },
  "project-files": {
    title: "Project files",
    body: "Every document filed under this project. This is the grounding the scoped chat answers from.",
  },
};

/** Provided by App; toggled from Settings. */
export interface HelpState {
  enabled: boolean;
  setEnabled: (enabled: boolean) => void;
}

export const HelpContext = createContext<HelpState>({
  enabled: false,
  setEnabled: () => {},
});

export const useHelp = () => useContext(HelpContext);
