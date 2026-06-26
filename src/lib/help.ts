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
  "nav-teach": {
    title: "Teach",
    body: "Where you teach PM how your projects are named. Merge a name variant into the real project so it stops reappearing, rename a project everywhere at once, or add a name you know means the same thing — the same thing correcting a project in Review does, just directly. On for the Standard and Power presets; toggle it under Settings → Appearance.",
  },
  "nav-graph": {
    title: "Map",
    body: "A visual graph of your documents grouped by project. Each project is a hub; the documents around it are its files. Hover any document node to see its details.",
  },
  "nav-pinboard": {
    title: "Pinboard",
    body: "A free-form planning board: pin sticky notes and simple timelines, drag them around, and resize them. It's a scratch space for thinking — saved on this device, separate from your documents.",
  },
  "nav-dev": {
    title: "Dev",
    body: "Developer mode's inspection tab: read-only views of PM's internal state — raw tables, row counts, the corrections log, and system & build info. For debugging and watching how PM works; nothing here changes your data. Turn it on or off under Settings → Developer.",
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

  // Teach (entity resolution)
  "teach-intro": {
    title: "Teach",
    body: "PM keeps one canonical name per project, and remembers the other names you've filed things under as aliases. Here you manage them directly — merge, rename, and add names — so the same project never shows up two ways again.",
  },
  "teach-entity": {
    title: "A project",
    body: "One project PM knows, with the doc count and every other name that resolves to it. Rename it (changes it on every document at once), merge it into another project, or add another name that means the same thing.",
  },
  "teach-suggestions": {
    title: "Look-alikes",
    body: "Projects whose names are identical once spacing, punctuation, and capitalisation are ignored — likely the same thing filed two ways. Merging folds one into the other so the duplicate stops appearing.",
  },
  "teach-preferences": {
    title: "Preferences",
    body: "Your typed preferences — how you like things filed, named, and answered. PM brings just the relevant ones into chat, sorting, and your briefing, instead of one long note. The “Suggested” ones were carried over from your old Learning You profile; keep, edit, or remove them.",
  },
  "teach-pref": {
    title: "A preference",
    body: "One rule, with where it applies — everywhere, one project, or a situation. Edit it, delete it, or “Keep” a suggested one to vouch for it.",
  },
  "teach-pref-nl": {
    title: "In your own words",
    body: "Type a preference as a plain sentence and PM fills in the fields for you. Check them before saving — nothing is stored until you do.",
  },
  "settings-teach-tab": {
    title: "Teach tab",
    body: "Show or hide the Teach tab in the sidebar. It's on for the Standard and Power presets and off for Minimal — but you can override that here. Hiding it only hides the editor; PM still applies the naming rules you've already taught it.",
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
    body: "Two live suggestions from OpenRouter's catalogue. Day-to-day is the cheapest model that still handles tool-calling and a decent context — ideal for high-volume sorting and everyday chat. Advanced is the highest-capability, most faithful model for high-stakes, citation-critical chat. Each shows why it was picked and its effective (cache-weighted) price. PM enforces zero-data-retention on every request, so providers can't store or train on your prompts — that's the 'ZDR' marker. Apply either to your chat or background slot; nothing changes until you Save.",
  },
  "settings-usage-cost": {
    title: "Usage & cost",
    body: "What you've spent on model calls, priced from OpenRouter's public rates (refreshed about once a day). Shows last-30-days and all-time totals; expand 'How this is calculated' for the method and a per-model spend ranking. 'Refresh prices' re-pulls the latest rates. Costs are computed from token counts at read time, so a price change re-prices your history.",
  },
  "settings-help-mode": {
    title: "Help mode",
    body: "The switch you're using right now. While it's on, hovering any highlighted section shows an explanation like this one.",
  },
  "settings-app-lock": {
    title: "App lock",
    body: "Optionally require an OS check — Windows Hello (face, fingerprint, or PIN) — before PM opens. It's a convenience lock for the window only: your store is always encrypted at rest, so this isn't a second password on your data. It's off until you turn it on, only available where your device can actually verify, and takes effect the next time you open PM. If your device can't verify, you can still get in — you're never locked out of your own app.",
  },
  "settings-connectors": {
    title: "Connectors",
    body: "Connect external accounts so PM can find and use what's in them, grouped by what they do — Calendar, Drive, and email. Each provider keeps its own sign-in; every connection is independently opt-in and removable, and nothing cascades or auto-enables. Credentials and tokens live only in your keychain.",
  },
  "connectors-google-client": {
    title: "Google sign-in (one-time setup)",
    body: "One Google Cloud 'Desktop app' OAuth client — your own, pasted once and shared by every Google service (Calendar, Drive). PM ships no Google secret, so you supply your own; it stays in your keychain. Setting it up connects nothing on its own. If your account uses Advanced Protection, Google blocks this — use a calendar subscription (iCal) instead.",
  },
  "connectors-ics": {
    title: "Calendar subscription (iCal)",
    body: "Paste a calendar's private 'secret address in iCal format' — no sign-in, no Google Cloud project, and it works even with Advanced Protection. Read-only: it powers your agenda, schedule questions in chat, and the 'Due soon' status when an event names a project. The feed URL is a secret link and lives only in your keychain.",
  },
  "settings-calendar": {
    title: "Google Calendar",
    body: "Read-only Google sign-in with your own OAuth client. Once connected, pick which calendars to mirror so PM can show your agenda, answer schedule questions in chat, and flip a project to 'Due soon' when an event names it. For a no-sign-in option, use a calendar subscription (iCal) instead. Tokens live only in your keychain.",
  },
  "settings-drive": {
    title: "Google Drive",
    body: "Index your Google Drive (read-only) so PM can find and answer from your files. Everything is 'index-only': PM stores a searchable pointer and a short summary, never the file itself — the full file stays in Drive and is fetched when you open it. Connect more than one account; each syncs independently. Tokens live only in your keychain.",
  },
  "settings-drive-firstsync": {
    title: "First sync",
    body: "The first sync walks your whole Drive and indexes every file it can read, so it can take a while and use bandwidth on a large Drive. After that, syncing only fetches what changed since last time. Files deleted in Drive are kept findable but marked 'source missing' — never silently dropped.",
  },
  "settings-drive-shared": {
    title: "Shared drives & scope",
    body: "Choose what each account indexes. Your personal My Drive is indexed whole by default. Shared drives (Team Drives) are opt-in and folder-scoped by default — pick the folders you want (everything inside is indexed) or switch to the entire drive. Saving re-syncs: newly-in-scope files get indexed, and files that fall out of scope are kept findable but marked 'source missing'. Still index-only — the files stay in Drive.",
  },
  "settings-data": {
    title: "Data",
    body: "Everything you keep in PM lives in one folder named 'Personal Manager' — the Markdown vault of your documents plus the encrypted store (settings, pinboard, and the search index). Your documents in the vault are stored unencrypted so any tool can read them; their at-rest protection relies on your OS full-disk encryption (BitLocker on Windows, FileVault on macOS), so turn that on. 'Open data folder' reveals it in your file manager so you can copy or back it up by hand. 'Export all data' bundles the vault and the store into a single .zip you choose where to save; the regenerable runtime (the local model environment) is left out, and the store stays encrypted inside the archive.",
  },
  "settings-license": {
    title: "License",
    body: "PM is free and open-source software under the GNU AGPL v3 — you're free to use, study, share, and modify it, and any networked version must offer its source. The link opens the full project source on GitHub.",
  },
  "settings-developer": {
    title: "Developer mode",
    body: "A plainly-labelled switch for technical and curious users. When on, it adds a read-only Dev tab and shows PM's internals in place — raw rows, ids, and confidence values — so a problem is diagnosable where it happens. It's strictly read-only (it never changes your data), independent of the density preset, and off by default. 'Build' shows whether this is a dev or release build; 'runtime' is your toggle.",
  },

  // Developer mode (issue #78) — read-only inspection
  "dev-system": {
    title: "System & build",
    body: "The running app version, this store's schema migration level, the embedder and its vector dimension, whether reranking is on, the splitter version, and the document engine's status. A quick health read of the running vault.",
  },
  "dev-counts": {
    title: "Table counts",
    body: "Row counts across the store — documents, chunks, the vector and keyword indexes, entities, preferences, and more. The fastest way to confirm the index is populated and nothing is empty that shouldn't be.",
  },
  "dev-tables": {
    title: "Raw table browser",
    body: "Browse the store's tables, newest rows first. Only an allow-listed set of columns is shown; personal or large fields (chat bodies, document text) are truncated or shown as a length, and the settings grab-bag hides all but a few operational values — so the browser stays a safe, read-only window.",
  },
  "dev-corrections": {
    title: "Corrections log",
    body: "Every change you've made to a proposed project, tag, or importance — the raw signal PM learns your filing habits from. Shown as before → after with the document and when.",
  },
  "dev-retrieval": {
    title: "Retrieval explain",
    body: "Type a query and PM runs it through the same hybrid retriever that grounds chat, then shows why each chunk ranked: its vector distance, keyword rank, fused score, recency decay, and reranker score. Read-only — chunk text is a short truncated preview and nothing is changed. Use it to confirm the index returns the right passages and to see what reranking does.",
  },
  "dev-calendar": {
    title: "Calendar sync state",
    body: "Developer-mode read-out of the calendar connector's state: how many feeds/calendars are connected, whether Google OAuth is configured and connected, the sync window, and when it last synced. Tokens and feed URLs live in your keychain and are never shown here.",
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
