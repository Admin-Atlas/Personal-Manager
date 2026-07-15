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
  "nav-calendar": {
    title: "Calendar",
    body: "One place to see everything from the calendars you've connected — Google, Outlook and iCal subscriptions — laid over each other, read-only. Switch between Agenda, Month, Week and Year, jump to today, and choose which calendars show. PM also reads these events to answer schedule questions in chat and to flag a project 'Due soon' when an event names it.",
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
  "settings-pinboard-confirm-delete": {
    title: "Confirm before deleting a pinboard card",
    body: "Ask before a note or timeline is deleted from the Pinboard, so an important one can't go with a stray click. The pop-up tells you what actually goes with it — entries that would leave your calendar, or a document already saved to your vault that would stay behind. If you ticked 'Don't ask again' in that pop-up, this is where you turn it back on. It's per-device, and it doesn't apply to a folder's ✕, which only ungroups it and spills the cards back onto the board. Either way, Ctrl+Z (⌘Z on a Mac) undoes a deletion.",
  },
  "settings-teach-tab": {
    title: "Review & Teach tabs",
    body: "Show or hide the two 'learning tools' — Review (approve the assistant's filing of new items) and Teach (correct its naming rules and preferences) — together in the sidebar. They're on for the Standard and Power presets and off for Minimal, but you can override that here. Turn them off once the assistant files things well enough on its own that you no longer want to curate; nothing is lost — new items still index and auto-file, and PM keeps applying what you've already taught it. Turn them back on any time.",
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
    body: "Rebuilds PM's search index from scratch. It re-reads every document, note and chat from your vault and re-splits them with the current settings, then re-reads your connected files (Google Drive, OneDrive, watched folders) from their source so each is indexed on its full contents — not just its saved summary. Your documents, projects, tags and importance are never changed. Safe to run anytime; large connected sources make it take longer, and anything offline is skipped and caught up on the next sync.",
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
    body: "Set the project (start typing to reuse an existing one), pick an importance, and add short tags. The grey line is the AI's reasoning for its suggestion. Hover the Project, Importance, and Tags fields for what each one does.",
  },
  "review-project": {
    title: "Project",
    body: "Which body of work this document belongs to — the bucket it's filed and found under (one document, one project). Start typing to reuse an existing project; a new name creates one. Projects are what the Focus and Map views group by, so consistent names matter more than perfect ones.",
  },
  "review-importance": {
    title: "Importance",
    body: "How much this document matters to you — NOT how new it is or how often you open it. High: a core reference you'd be set back without; it can surface in Focus and nudge a project toward 'due soon'. Medium: useful, you'll want it sometimes. Low: keep-but-peripheral — it's auto-filed out of the review queue so it never becomes a chore, and stays fully searchable. Archive: deliberately shelve it — hidden from the Map and sorted to the bottom of lists, but still findable by search (especially exact keywords); pick it for things you want out of the way without deleting. Rules of thumb for the tricky cases: an active project but an outdated/unused file → Low (the project being live doesn't make the file significant); something you open often but that isn't significant → Medium is fine; something you rarely touch but would be stuck without → High. When unsure, Medium is the safe middle — you can change it anytime.",
  },
  "review-tags": {
    title: "Tags",
    body: "Short, free-form labels for finding and grouping documents later — topics, people, or types like 'invoice', 'tax', 'spec', 'meeting'. They're extra handles for search and don't change a document's project or importance. Keep them lowercase and short; press Enter or comma to add, × to remove. A document can have many; none is fine too.",
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
  "context-meter": {
    title: "Context usage",
    body: "How full the selected model's context window is for this conversation — the button sits on the message row so it's out of the way until you need it. It stays quiet on the calm preset and turns red near the limit; open it to Compress (fold older turns into a running summary, with an Undo), switch to a larger-context model, or continue anyway. Your full conversation is always kept word-for-word in your vault.",
  },
  "chat-retrieval-explain": {
    title: "Explain retrieval",
    body: "Opens from the button on the message row. Shows exactly which chunks of your documents a query pulls in and how each one scored, and lets you tune the one lever that matters — the retrieval depth (the size of the candidate pool the reranker weighs, not how many results are shown). Dragging the depth slider previews live; a separate button commits it. Describe what it's missing and PM suggests what to change — it never changes anything on its own.",
  },
  "chat-idle-prompt": {
    title: "Idle conversation",
    body: "This chat has been quiet for a while. Starting a new one keeps each topic separate and gives the assistant a clean slate, rather than carrying a stale thread forward. Dismiss to keep writing in this one — nothing is deleted either way.",
  },
  "chat-resumed": {
    title: "Where you left off",
    body: "Marks where an earlier conversation picks back up, with when it was last active — so a long-running chat you return to reads in order instead of looking brand-new.",
  },

  // Calendar (unified read-only view, card 8)
  "calendar-view": {
    title: "Your calendar",
    body: "A single read-only view of every calendar you've connected, merged together and colour-coded by source. PM never writes to your calendars here — it mirrors them so you can see your whole schedule in one place, and it uses the same events for your agenda, chat answers and 'Due soon' flags. Connect or choose calendars in Settings → Connectors.",
  },
  "calendar-header": {
    title: "Move around & choose a view",
    body: "‹ Today › steps through time and jumps back to now; the buttons beside it switch between Agenda, Month, Week and Year, and the label shows the range you're looking at. Everything here only changes what you see — it never changes your actual calendars.",
  },
  "calendar-filter": {
    title: "Which calendars show",
    body: "Tick the connected calendars you want laid over each other; untick one to hide it here without disconnecting it. Each keeps its own colour so you can tell sources apart. This only changes what's shown — it doesn't stop PM syncing that calendar.",
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
  "settings-indexing-speed": {
    title: "Indexing speed",
    body: "How hard PM works your machine when it indexes documents — a Drive sync, or files you add (email later). Fast indexes at full speed, using as much CPU and memory as it needs. Gentle paces the work — it pauses briefly between files and embeds in smaller batches — so it uses less CPU and less memory and your computer stays usable while a big index runs in the background. Calendars aren't affected: they're tiny and only fetch events (no indexing), so they always sync at full speed. The setting only changes how fast indexing goes, never what gets indexed, and a change applies right away — even partway through a sync.",
  },
  "settings-learning": {
    title: "Learning You",
    body: "A short, readable profile of how you organise — distilled from the corrections you make in Review. PM injects it into its suggestions and chat so it gets more like you over time. 'Refresh now' rebuilds it from your latest corrections.",
  },
  "settings-appearance": {
    title: "Appearance",
    body: "Switch the visual System (Editorial / Slate / Terminal), Light or Dark mode, the accent colour, and Depth (how much detail is shown). Changes apply instantly and are remembered on this device.",
  },
  "settings-storage": {
    title: "On-device components",
    body: "Everything PM has downloaded to this device, with sizes. The document engine and the active search model are always needed. The enhanced map layout (t-SNE) and the speech model can be removed to free space — they re-download when you need them again. The heavy shared libraries (scikit-learn, scipy) can only be removed once nothing still uses them: a greyed Remove button shows a pill pointing to what to remove first. numpy is never offered because the search model shares it.",
  },
  "settings-memory-map": {
    title: "Memory map",
    body: "Settings for the Map tab. Default grouping picks how the Map opens — Semantic proximity (similar documents sit together) or By project (each project a hub with its documents around it). Project cohesion (Off by default) gently pulls same-project documents together in the semantic view without abandoning the meaning-driven layout. Maximum nodes caps how many documents are individually plotted; above it, the rest are gathered at their project's spot — raise it for a fuller picture, lower it on a slower machine. Enhanced layout (t-SNE) is a one-time, on-device download that gives the semantic view tighter, clearer clusters than the built-in basic layout; once installed you can switch it on or off, or remove it to free space.",
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
    body: "Optionally require an OS check before PM opens — Windows Hello (face, fingerprint, or PIN) on Windows; not available on macOS or Linux yet. It's a convenience lock for the window only: your store is always encrypted at rest, so this isn't a second password on your data. It's off until you turn it on, only available where your device can actually verify, and takes effect the next time you open PM. If your device can't verify, you can still get in — you're never locked out of your own app.",
  },
  "settings-connectors": {
    title: "Connectors",
    body: "Connect external accounts so PM can find and use what's in them, grouped by provider — Google, Microsoft, Apple. A provider's sign-in is set up once at the top of its group and shared across all of that provider's services (Google: Calendar + Drive; Microsoft: OneDrive). Calendar subscriptions (iCal) need no sign-in and sit in their own section. Every connection is independently opt-in and removable, and nothing cascades or auto-enables. Credentials and tokens live only in your keychain.",
  },
  "connectors-google-multiaccount": {
    title: "More than one Google account",
    body: "How to connect a second (or third) Google account. You don't need a new Google Cloud project or new credentials — every account reuses the one client you saved. If your OAuth app is still in 'Testing' mode, add each account's email under Audience → Test users in the Google Cloud Console first; a published app skips that. Then use 'Add another account' on Drive and pick the different account from Google's chooser. A separate project is only needed if you'd rather keep accounts fully isolated.",
  },
  "connectors-google-client": {
    title: "Google sign-in (one-time setup)",
    body: "One Google Cloud 'Desktop app' OAuth client — your own, pasted once and shared by every Google service (Calendar, Drive). PM ships no Google secret, so you supply your own; it stays in your keychain. Setting it up connects nothing on its own. If your account uses Advanced Protection, Google blocks this — use a calendar subscription (iCal) instead.",
  },
  "connectors-microsoft-client": {
    title: "Microsoft sign-in (one-time setup)",
    body: "One Microsoft (Azure) app registration — your own client id, pasted once and shared by every Microsoft service (OneDrive). PM ships no Microsoft secret and stores none: it's a public client, so you supply only the id and it lives in your keychain. Setting it up connects nothing on its own — you then add each account separately.",
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
  "settings-drive-report": {
    title: "Sync results",
    body: "A summary of the last sync: how many files were indexed, updated, or removed, plus any that couldn't be indexed and why (an unsupported file type, or a fetch error). Files that couldn't be read are simply skipped — nothing is lost, and they don't block the rest. Indexed files become searchable and show up in Documents. If you stopped the sync early, everything indexed so far is kept; sync again to finish the rest.",
  },
  "settings-onedrive": {
    title: "OneDrive",
    body: "Index your OneDrive (read-only) so PM can find and answer from those files. It's index-only: PM stores a searchable pointer and a short summary, never the file itself — the full file stays in OneDrive and is fetched when you open it. Connect more than one account; each syncs on its own. Tokens live only in your keychain.",
  },
  "settings-onedrive-firstsync": {
    title: "First sync",
    body: "The first sync walks your whole OneDrive and indexes every file it can read, so on a large drive it takes a while and uses some bandwidth. After that it only fetches what changed. Files deleted in OneDrive are kept findable but marked 'source missing' — never silently dropped.",
  },
  "settings-onedrive-scope": {
    title: "What OneDrive indexes",
    body: "Choose which folders an account indexes. By default the whole drive is in scope; narrow it to specific folders (everything inside a chosen folder is indexed). Saving re-syncs — newly in-scope files get indexed, and files that fall out of scope are kept findable but marked 'source missing'. Still index-only: the files stay in OneDrive.",
  },
  "settings-onedrive-report": {
    title: "Sync results",
    body: "A summary of the last OneDrive sync — indexed, updated and removed counts, plus anything that couldn't be indexed and why. Unreadable files are skipped, not lost, and don't hold up the rest. Stop early and everything indexed so far is kept; sync again to finish.",
  },
  "settings-local-folders": {
    title: "Folders on this device",
    body: "Point PM at folders on your own computer and it indexes the documents inside them, so their contents turn up in search alongside your cloud sources. Nothing is copied — PM only reads each file to index it, and watches the folder to stay current as files change. Each folder shows what it's indexed and when; removing a folder just stops the watching, and what's already indexed stays findable.",
  },
  "settings-local-report": {
    title: "Folder sync results",
    body: "A summary of the last folder scan — how many files were indexed, updated, or dropped, and any that couldn't be read (an unsupported type, or a permission error). Skipped files don't block the rest; sync again to retry. Files removed on disk are kept findable but marked 'source missing'.",
  },
  "settings-data": {
    title: "Data",
    body: "Everything you keep in PM lives in one folder named 'Personal Manager' — the Markdown vault of your documents plus the encrypted store (settings, pinboard, and the search index). Your documents in the vault are stored unencrypted so any tool can read them; their at-rest protection relies on your OS full-disk encryption (BitLocker on Windows, FileVault on macOS), so turn that on. 'Open data folder' reveals it in your file manager so you can copy or back it up by hand. 'Export all data' bundles the vault and the store into a single .zip you choose where to save; the regenerable runtime (the local model environment) is left out, and the store stays encrypted inside the archive.",
  },
  "settings-vault": {
    title: "Vault mode",
    body: "Whether this vault is private to this device or shared. A private vault is tied to this device's keychain — zero friction, nothing to remember. Sharing protects it with a passphrase instead and moves it where other Windows accounts on this PC can reach it, so the same documents, chats, and projects open from each account (one writes at a time; PM hands over cleanly). The passphrase is the only way in and can't be recovered, so keep it safe. A shared vault also encrypts your Markdown at rest, and you can copy it to another machine and open it there with the same passphrase.",
  },
  "settings-vault-share": {
    title: "Share with other accounts",
    body: "A short guided flow: pick a passphrase, PM moves the vault to a spot every account on this PC can reach, and you choose which accounts may open it. When they next launch PM on their account, it offers this vault by name — they join with just the passphrase. Their AI key and cloud sign-ins stay their own; each person reconnects those as themselves.",
  },
  "settings-vault-join": {
    title: "Open an existing shared vault",
    body: "Point PM at a shared vault folder on this PC (or a vault copied from another machine) and open it with its passphrase. PM switches to that vault; whatever you were using stays on disk, set aside, and you can go back to it any time. If PM can't reach the folder, the vault's owner needs to add your account first under Manage sharing.",
  },
  "settings-backup": {
    title: "Encrypted backup",
    body: "Beyond a one-off backup, PM can run them on a schedule and keep the last few for you. Scheduled backups only fire when it's safe and unobtrusive — PM unlocked, the machine idle and online, and a passphrase set. Point it at more than one place (say a folder plus Google Drive) and each backup is packed once and copied to all of them. Older backups beyond the number you keep are pruned automatically.",
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
    body: "Each node is a document — its size is how much content it holds (its number of chunks) and its colour is its project, matching the legend. A dashed ring means a document is still awaiting review, or is index-only (it lives in a connected source like Drive rather than on this device). There are two arrangements: in By project (the default), each project becomes a hub with its documents gathered around it and linked to it, so you see how your library is organised; in Semantic proximity, documents that sit close together are about similar things — their position comes from their meaning, not their project. Scroll to zoom (or sideways to pan left/right), drag to pan, and double-click (or the Fit button) to reset the view. Hover a node for its details, or click it to open its project.",
  },
  "map-layout-toggle": {
    title: "How the map is arranged",
    body: "Switch how documents are laid out. By project (the default) groups documents by where you've filed them: each project is a hub with its documents gathered around it and linked to it, so the map shows how your library is organised. Semantic proximity instead places documents so that similar ones sit close together, worked out from their meaning (a basic layout, computed on your device) — here projects only colour the nodes, they don't decide where a node goes. An optional enhanced layout — t-SNE, which finds tighter, clearer clusters of related documents — can be downloaded from the bar below the header (or Settings → Memory map); it's a one-time download that then runs fully on your device.",
  },
  "map-layout-toggle-tsne": {
    title: "How the map is arranged",
    body: "Switch how documents are laid out. By project (the default) groups documents by where you've filed them: each project is a hub with its documents gathered around it and linked to it, so the map shows how your library is organised. Semantic proximity places similar documents close together — it's using t-SNE, the enhanced layout you installed, which arranges documents by learned neighbourhoods so related ones form tighter, clearer clusters (projects only colour the nodes here). You can turn t-SNE off, or remove it, in Settings → Memory map. The semantic layout is computed in the background and cached, so the first run after big changes can take a moment.",
  },
  "map-cohesion": {
    title: "Project cohesion",
    body: "How strongly the semantic layout pulls documents in the same project together. Off (the default) lays everything out purely by meaning. Low, Medium, or High nudge same-project documents a little closer, so each project reads as a looser cluster while meaning still drives the overall shape — a middle ground between Semantic proximity and By project. It applies instantly and changes nothing about how the layout is computed or stored.",
  },
  "map-navigate": {
    title: "Move around the map",
    body: "Zoom in or out, or fit the whole map back into view. You can also scroll to zoom (scroll sideways to pan left/right), drag anywhere to pan, and double-click to reset the framing.",
  },
  "map-labels": {
    title: "Show file names",
    body: "Write each document's file name inside its node, so you can tell nodes apart without hovering. The text is sized to the node — small when you're zoomed out, larger as you zoom in — and a long name is shortened to fit, showing the start; zoom into a bigger node to read more of it. Off by default to keep a busy map uncluttered; your choice is remembered on this device.",
  },

  // Focus view & projects (Step 5)
  "focus-header": {
    title: "Focus",
    body: "Your home screen. It gathers every active project onto one page so you can see the whole picture at a glance and pick the one thing to do next, instead of holding it all in your head.",
  },
  "focus-cards": {
    title: "Your projects",
    body: "Every active project. By default they're sorted with the most pressing first; use the Sort control to reorder by deadline, priority, size, or most-recent activity, and the ↑/↓ button to reverse it. Each row carries one status that answers 'should I look at this now?'. Hover a project, its status, or its Triage panel for more.",
  },
  "focus-sort": {
    title: "Sort your projects",
    body: "Reorder the list. Smart (the default) keeps the most pressing first — due-soon, then blocked, quick wins, gone-quiet, and so on. Or sort by Deadline (the nearest milestone), Priority (the level you set in Triage), Size, or Recent active (when you last touched the project — including chatting in it or editing its milestones). The ↑/↓ button flips between ascending and descending; your choice is remembered on this device.",
  },
  "focus-briefing": {
    title: "Today's briefing",
    body: "A short, AI-written summary of where to focus today — drawn from your due-soon projects, upcoming calendar, quick wins, and anything that's gone quiet. It refreshes itself about once a day; hit Refresh to rebuild it now from your current state.",
  },
  "focus-suggest": {
    title: "Suggest attributes",
    body: "Asks the AI to propose a size, a parent project, a blocker, and (if your documents mention one) a deadline for each project. Nothing is applied until you confirm it in a project's Triage panel — AI proposes, you decide.",
  },
  "focus-box": {
    title: "Say what you mean",
    body: "One box that reads your plain words and does the right thing: tell it something's handled ('the deck is done') and it ticks that item off; say how you want to be nudged ('stop reminding me so early') and it saves that as a lasting preference; or ask a question and it opens a chat to answer. No menus, no picking the right item first. Anything that would cross something off asks you to confirm, so nothing changes by accident.",
  },
  "focus-box-input": {
    title: "Type here",
    body: "Write in plain language and press Enter. PM works out whether you're marking something done, setting a preference, or asking — you don't choose a mode. A suggestion you haven't confirmed waits for you, and survives switching tabs and back.",
  },
  "focus-agenda": {
    title: "Upcoming",
    body: "Your next events from the calendars you connected, read-only. PM also uses these to answer schedule questions in chat and to flag a project 'Due soon' when an event's title names it. Connect or sync in Settings → Google Calendar.",
  },
  "focus-card": {
    title: "Project",
    body: "One project and its current status. The line beneath shows what drives that status — document count, priority, size, the nearest milestone, blocker, last activity. Click the name to open it; click Triage to set its attributes.",
  },
  "focus-status-badge": {
    title: "Status",
    body: "The one thing this project is telling you. Due soon = a deadline is near. Quick win = small enough to finish fast. Take a look = it's gone quiet. Blocked = waiting on another project. Part of = a piece of a bigger project. On track = nothing needed now.",
  },
  "focus-triage": {
    title: "Triage a project",
    body: "Set a size (a 'quick' project becomes a Quick win), a priority (High/Medium/Low — or Auto, which shows no tag), a blocker (drives Blocked), or a parent (drives Part of), and add milestones — the project's deadlines. Priority is yours to set: it's no longer guessed from your documents, and you can sort the focus list by it. If the AI suggested values, 'Use it' fills the form so you can confirm or tweak before saving.",
  },
  "project-milestones": {
    title: "Milestones",
    body: "A project's deadlines — a pitch, a presentation, an internal due date — each with a name and a date. Tick the checkbox on the left to mark one done: it gets a 'Done' tag and a line through it. The nearest one you haven't ticked off drives the 'Due soon' status; tick it and the next takes over. Reorder with the arrows, remove with ×. Link a milestone to a calendar event (📅) and its date stays in sync with your calendar automatically.",
  },
  "project-resize": {
    title: "Resize the panel",
    body: "Drag this edge to make the side panel wider or narrower. The width is a share of the window, so it stays in proportion as you resize the app, and it's remembered on this device.",
  },
  "project-sidebar": {
    title: "Project panel",
    body: "Everything about the open project, on the right: its milestones on top and its filed documents below, over a chat that draws on just this project. Drag the left edge to widen it, and the divider between milestones and files to change their split.",
  },
  "project-milestones-panel": {
    title: "This project's milestones",
    body: "The project's deadlines, editable in place — add one, date it, tick it off, reorder, or link it to a calendar event. The nearest one you haven't ticked drives the project's 'Due soon' status. Drag the divider below to give this panel more or less room against the files list.",
  },
  "project-split": {
    title: "Resize milestones vs files",
    body: "Drag to change how the panel is split between the milestones (top) and the files (bottom). The ratio is remembered on this device, so the panel opens the way you left it.",
  },
  "project-chat": {
    title: "Project chat",
    body: "A chat that only draws on this project's documents, so the rest of your knowledge falls away while you focus. Ask anything about just this project.",
  },
  "project-files": {
    title: "Project files",
    body: "Every document filed under this project. This is the grounding the scoped chat answers from.",
  },

  // Pinboard (spec §4) — a free-form planning board
  "pinboard-board": {
    title: "Your planning board",
    body: "A free space to think. Add notes, timelines and folders, then drag them anywhere and resize from the bottom-right corner — they snap to a grid and stay where you put them between visits. Cards are free to overlap; to file one into a folder, drop it with your mouse pointer over the folder. Ctrl+Z (⌘Z on a Mac) undoes what you last did here — a deletion, a colour, a move, or the last few seconds of typing — and Ctrl+Y (or Ctrl/⌘+Shift+Z) puts it back; the trail is kept for this visit, not saved between them. The board is saved on this device, encrypted with the rest of your data, and is separate from your indexed documents.",
  },
  "pinboard-add-note": {
    title: "Add a note",
    body: "Drops a sticky note on the board to jot anything down. Notes are free text; tint one with a colour to group or flag it. Available at every density.",
  },
  "pinboard-add-timeline": {
    title: "Add a timeline",
    body: "Drops a timeline card for laying out dated milestones in order — handy for sketching a plan at a glance.",
  },
  "pinboard-add-folder": {
    title: "Add a folder",
    body: "Drops an empty folder on the board to tidy things into. Drag a note or timeline so your mouse pointer is over the folder and let go to file it inside — the folder lights up when it's going to catch what you're holding. (Dropping a card so it merely overlaps a folder just leaves it lying on top, the same as notes overlap each other.) You can also make a folder without this button, by dropping one card exactly on top of another the same size. A folder stays until you ungroup it with its ✕, however few cards are left in it — so an empty one will happily sit and wait.",
  },
  "pinboard-folder": {
    title: "A folder",
    body: "A group of notes and timelines, kept in one tile so the board stays tidy. It shows how many cards are inside; click it to open them, and give it a title like any other card. Drag a card onto it — pointer over the folder — to file that card away. Folders don't go inside folders: drag one onto another and they simply stack, each keeping its own cards. The ✕ ungroups the folder, spilling its cards back onto the board rather than deleting them. Once open, 'In place' shows the cards as a compact grid beside the folder, and 'Overlay' opens the folder's own pinboard over the middle of the screen, where the cards keep their real shape and size.",
  },
  "pinboard-folder-board": {
    title: "The folder's pinboard",
    body: "Inside a folder you get a board of its own, at 80% of your main one. The cards keep the shape and size you gave them, and you drag, resize and overlap them in here exactly as you do outside — a folder is a place to put things, not a different kind of thing. Nothing files into anything in here: folders don't nest, so a card dropped on another simply overlaps it. Use ⤴ on a card to move it back out to the main board.",
  },
  "pinboard-timeline-view": {
    title: "List or line",
    body: "Two ways to show this timeline's entries: as a stacked list of rows, or laid out left to right along a line in date order. Purely how it looks — the entries and their dates are the same either way — so pick whichever reads better at the size you've made the card.",
  },
  "pinboard-note": {
    title: "A note",
    body: "A sticky note — type anything into it and it saves as you go. Notes are Markdown: start a line with . or - for a bullet, 1. for a numbered list, i. for roman numerals, > for an arrow/quote, or [] for a checkbox, and pressing Enter continues the list for you. The note renders itself when you're not editing — click it to write again. Drag the header to move it, resize from the bottom-right corner, and use the ✕ to remove it. The dots along the bottom tint it a colour so related notes read together.",
  },
  "pinboard-note-format": {
    title: "Format the note",
    body: "Quick formatting for the note: bold, italic, heading, and bullet / numbered / checklist lists. Each button applies to the selected line(s) — press it again to turn it off. Every button also has a keyboard shortcut (shown when you hover) so you can format without leaving the keyboard.",
  },
  "pinboard-note-ingest": {
    title: "Ingest as a document",
    body: "Save this note into your vault as a real Markdown document, the way any document goes in: it's written to your vault, chunked, indexed, and lands in the Review queue for you to file to a project and set its importance — after which it turns up in Documents, Focus and search like anything else. It shows 'In review' until you file it, then 'Filed · project'. Edit the note and a 'Re-ingest' button updates the same document, keeping its filing. The document is its own copy in your vault: deleting the note never removes it.",
  },
  "pinboard-note-tint": {
    title: "Tint this note",
    body: "Colour the note to group or flag it — the colours are the same status hues PM uses elsewhere (due, quick win, and so on), so a tint can carry a light meaning. It's purely visual and changes nothing else.",
  },
  "pinboard-timeline": {
    title: "A timeline",
    body: "A card for dated milestones. Give it a title and add rows with a date and a short label; they sort themselves earliest to latest. Or link it to a real project (below) to show and edit that project's actual milestones. Drag the header to move it and resize from the corner.",
  },
  "pinboard-timeline-project": {
    title: "Link a project",
    body: "Bind this timeline to one of your projects — pick an existing one or type a new name. Once linked, the card shows that project's real milestones on a line, and adding or editing one here writes straight to the project, so it also shows in your daily briefing and the project's Focus panel. A brand-new name becomes a real project when you add its first milestone (it appears on your Focus page once it also has a document).",
  },
  "pinboard-timeline-line": {
    title: "Milestones on a line",
    body: "This project's milestones, laid out earliest to latest. Each is a dot on the line with its date above and label below — edit either in place, click the dot to mark it done, or ✕ to remove it, and every change syncs with the project everywhere else. A 📅 date is synced from a linked calendar event and is read-only here.",
  },
  "pinboard-timeline-unlink": {
    title: "Unlink the project",
    body: "Detach this card from its project and return it to a freeform timeline. The project and its milestones are untouched — only this card stops tracking them.",
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
