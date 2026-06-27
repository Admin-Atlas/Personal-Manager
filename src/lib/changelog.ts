// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// In-app changelog shown in the "What's New" view. The app auto-opens it once
// after updating to a version the user hasn't seen yet (see App.tsx), and it's
// always reachable from the sidebar.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ RELEASE CHECKLIST: add a new entry at the TOP for every release, with the │
// │ version matching package.json / tauri.conf.json / Cargo.toml. Newest      │
// │ first. See docs/RELEASING.md.                                             │
// └─────────────────────────────────────────────────────────────────────────┘

export interface ChangelogEntry {
  version: string; // matches the released app version, no leading "v"
  date: string; // YYYY-MM-DD
  highlights: string[]; // short user-facing bullet points
}

export const CHANGELOG: ChangelogEntry[] = [
  {
    version: "2.26.0-alpha",
    date: "2026-06-27",
    highlights: [
      "New connector: Microsoft OneDrive. Under Connectors → Drive, set it up once (a free Microsoft “Mobile & desktop” app registration — just paste the client ID, there’s no secret to copy), then connect one or more OneDrive accounts. Works with both personal Microsoft accounts and work/school accounts.",
      "Everything stays index-only, exactly like Google Drive: PM stores a searchable pointer and a short summary, never the file itself — the full file stays in OneDrive and is fetched on demand. The first sync indexes your whole OneDrive; later syncs only fetch what changed (via OneDrive’s delta query).",
      "Index your whole OneDrive, or expand an account and switch to “Choose folders” to index just the folders you pick (subfolders included). Files that fall out of scope stay findable but stop syncing new changes.",
      "Same robust syncing as Drive: it runs in the background (leave the page and come back), can be stopped (keeping everything indexed so far), resumes automatically after a crash, and shows a results summary — including any files it couldn’t read.",
      "Disconnecting an account keeps its indexed items findable (marked “source unreachable”) and never deletes them; reconnect to resume.",
    ],
  },
  {
    version: "2.25.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Set a document's importance right from a project's Files panel — the same High / Medium / Low / Archive toggle as the Review tab, so triaging happens where you're already looking. On Power density the panel also gives you the full Review-style controls (re-file the project, edit tags); Standard keeps just the importance toggle.",
      "Those focus-panel controls follow your 'Review & Teach tabs' setting: if you've decided PM's auto-filing is good enough and hidden those tabs, the manual controls in Focus hide with them and the panel goes back to a clean read-only list.",
      "New Archive importance level (it replaces 'None'). Archiving shelves a document: it drops off the Map and sinks to the bottom of the Documents list, but stays fully searchable — including by exact keyword. Brand-new, un-triaged documents still show on the Map as before, so archiving is now a deliberate choice rather than just 'not set yet'.",
      "A project's Files panel can now be sorted by name or importance — click a heading to sort, click it again to reverse.",
      "The Review queue now orders itself by importance: the AI's most-important suggestions rise to the top, and it re-sorts live as proposals come in, so you triage what matters first.",
      "If Google Calendar sync fails only because the Calendar API isn't switched on in your Google Cloud project, PM now shows a one-click 'Enable the Google Calendar API' link (pointed at your project) instead of a raw error — connect, enable, sync.",
    ],
  },
  {
    version: "2.24.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connected a personal Google account? You can now index just the folders you choose from My Drive instead of the whole thing. Your whole drive stays the default — but under Connectors → Drive, switch My Drive to 'Choose folders' and pick the ones you want (subfolders included), the same way shared drives already work.",
      "Everything stays index-only (a pointer and summary, never the file bytes), and the change applies on your next Sync now: newly-in-scope files get indexed, and files that fall outside the folders you picked stay findable but stop syncing new changes.",
    ],
  },
  {
    version: "2.23.0-alpha",
    date: "2026-06-27",
    highlights: [
      "New Settings → Storage tab: see what PM has downloaded to this device — the document engine, the enhanced map layout, the speech model, and your search model — with sizes, and remove what you don't need to free space. Everything here re-downloads on demand if you want it back.",
      "Removals are safe by design: the heavy shared libraries behind the enhanced map layout can only be removed once nothing still uses them — a greyed Remove button points you to what to turn off first — and removing them asks an extra confirm that recommends keeping them. The libraries PM relies on elsewhere are never offered for removal.",
      "The enhanced (t-SNE) layout's Remove moved to the new Storage tab; its on/off switch stays under Settings → Memory map.",
      "The Map can now write each document's file name inside its node — turn 'Names' on in the Map's top bar (next to the arrangement toggle) to read nodes at a glance instead of hovering. The text is sized to the node and a long name is shortened to fit, so zoom into a bigger node to read more of it.",
    ],
  },
  {
    version: "2.22.1-alpha",
    date: "2026-06-27",
    highlights: [
      "Behind the scenes: routine maintenance updates to a few bundled dependencies, keeping things current. No change to the app itself.",
    ],
  },
  {
    version: "2.22.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Map can now arrange your documents by meaning: similar ones sit close together, worked out from their content on your device. Switch between Semantic and By-project in the Map header (or set a default under Settings → Memory map). It's computed quietly in the background and cached, so it's ready when you open it.",
      "Semantic proximity uses a fast built-in layout by default; for sharper, tighter clusters you can download an optional enhanced (t-SNE) component — a one-time download that then runs fully on your device — from the Map or Settings → Memory map.",
      "Settings → Memory map also lets you cap how many documents are plotted (200–5,000; default 1,000) so the Map stays comfortable on large libraries — beyond the cap, the rest are gathered at their project's spot rather than dropped.",
      "The Map is now fully navigable: scroll to zoom (scroll sideways to pan left/right), drag to pan, and double-click (or the Fit button) to reset — in every arrangement. Large project clusters (e.g. right after a big import) now pack together more tightly instead of spreading out.",
      "An optional Project cohesion control (Off by default) gently pulls same-project documents together in the semantic view, if you'd like projects to read as looser clusters while meaning still drives the layout — it's in the Map's top bar and Settings → Memory map.",
      "Downloading the enhanced (t-SNE) layout shows a progress bar, and once it's installed you can switch it on or off — or remove it to free space — from Settings → Memory map.",
    ],
  },
  {
    version: "2.21.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Map is faster and smoother with large libraries: it's now drawn on a single canvas instead of one shape per node, so panning and hovering stay responsive even with thousands of documents (a big Drive sync adds a lot). It also loads quietly in the background when the app starts — off to the side so it never stutters the rest of the app — so opening the Map tab is instant.",
      "Click any document node on the Map to jump straight to its project. A dashed ring now marks documents still awaiting review or held index-only (in a connected source like Drive, not stored on this device); hover a node — with help mode on — to see what its size and colour mean.",
    ],
  },
  {
    version: "2.20.1-alpha",
    date: "2026-06-27",
    highlights: [
      "Behind the scenes: PM's design-system primitives — buttons, cards, list rows, badges, dialogs and the rest — are now mirrored to Claude Design, so on-brand mockups can be built from the app's real components. No change to the app itself.",
    ],
  },
  {
    version: "2.20.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Indexing speed setting (Fast / Gentle) moved to the top of Settings → Connectors — that's where it matters most, since a big Drive sync is the longest index. Switching it now takes effect immediately, even partway through a sync; before, a change only applied to the next run.",
      "Gentle indexing is now easier on memory, not just the processor: it embeds in smaller batches, so a low-memory machine stays usable while a large index runs. Each mode is spelled out — Fast uses as much CPU and memory as it needs; Gentle paces the work and uses less of both.",
      "Settings → Usage & cost no longer reads blank for a model you've used both before and after the real-cost update. It now adds the real per-call cost OpenRouter reports and only estimates the older calls that predate it — so your real spend always shows, instead of one older call hiding the whole model's cost.",
    ],
  },
  {
    version: "2.19.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Google Drive now indexes shared drives (Team Drives), not just your personal My Drive. Expand a connected account under Settings → Connectors → Drive to pick which shared drives to add. Shared drives are folder-scoped by default — choose the folders you want (everything inside is indexed) or switch a drive to index entirely. Still index-only: the files stay in Drive.",
      "The Connectors sections (Calendar, Drive, Email) are now collapsible so the tab stays tidy — expanded by default on Standard and Power density, collapsed by default on Minimal, and you can fold either way.",
      "External links throughout the app — the Google setup guide, OpenRouter, release notes — now open in your default browser when clicked (previously they did nothing in the app window).",
      "Drive indexing now runs in the background: start a sync and you can leave the Settings page — it keeps working, and the progress reappears when you come back. One “Sync now” per account covers My Drive and your chosen shared drives.",
      "You can Stop a Drive sync mid-index if it's bigger than you expected — everything indexed so far is kept and stays searchable; only the rest is left for next time. You can also add another Google account while one is still indexing, and if PM is closed or crashes during a large index, it picks up where it left off on the next launch (already-indexed files are never lost).",
      "After a sync, Drive shows a summary: how many files were indexed, updated, or removed, plus an expandable list of any it couldn't read (e.g. an unsupported file type) with the reason — so nothing is silently dropped.",
      "The Review tab no longer re-runs (and re-bills) its AI suggestions every time you leave and come back — proposals are remembered until you commit them or press Re-propose. Helpful while a large Drive index is filling the queue and you want to peek at other tabs.",
      "Settings → Usage & cost now shows the real spend OpenRouter reports for each call — including prompt-cache savings — instead of a local estimate that could read blank when a model wasn't in the cached price list. (Costs from before this update still use the estimate.)",
      "Cheaper sorting: when the Review tab proposes filing for a batch of documents it now reuses a cached prompt prefix across the run, so models that support prompt caching bill the shared instructions once instead of per document — noticeably less for a large import.",
      "Wide tables (the developer Raw-table browser and retrieval explainer) now pan sideways with a plain mouse wheel; the Documents list is sortable by any visible column; and the Review cards explain — in help mode — what Project, Importance, and Tags each mean.",
      "New Indexing speed setting (Settings → Connectors): Gentle paces indexing so a low-end computer stays usable while a large Drive sync or import runs in the background; Fast (default) indexes at full speed.",
    ],
  },
  {
    version: "2.18.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Google Drive sync (Settings → Connectors → Drive): connect your Google account and PM indexes your Drive so you can search it and ground answers in it. It's index-only — PM keeps a searchable pointer and a short summary, never a copy; the file stays in Drive and is fetched on demand. Connect more than one account; each syncs on its own.",
      "The first sync walks your whole Drive (it warns you it can take a while); after that only changes are fetched. Files removed in Drive stay findable but are flagged 'source missing' rather than dropped. From Documents you can open an indexed file in Drive or pull its full text on demand.",
    ],
  },
  {
    version: "2.17.0-alpha",
    date: "2026-06-26",
    highlights: [
      "New Connectors tab in Settings, grouped by what each account does — Calendar, Drive, and email. The read-only calendar moved here from its own tab: connect Google Calendar with your own sign-in, or paste a no-sign-in calendar subscription (iCal) link.",
      "Each connection is independently opt-in and removable, and your Google sign-in is set up once and shared across Google services. Google Drive (index-only) arrives next; Microsoft and Apple show as coming soon.",
    ],
  },
  {
    version: "2.16.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Developer mode gains a “Retrieval explain” tool (Dev tab): type a query and see exactly why the assistant's search ranks the chunks it does — vector distance, keyword rank, fused score, recency decay, and reranker score — side by side. Handy for confirming the index returns the right material and seeing what reranking changes.",
      "It runs the same retriever chat uses but changes nothing: it's read-only, chunk text is shown only as a short truncated preview, and no document bodies or secrets are surfaced.",
    ],
  },
  {
    version: "2.15.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Developer mode now reveals internals right where each feature lives (turn it on under Settings → Developer): the Teach tab shows each project's and preference's raw id, confidence, and confirmed state; Documents lets you click a chunk count to inspect that file's chunk breakdown; and Calendar shows a read-only sync-state read-out.",
      "Same safety rules as the Dev tab — everything is read-only and personal or large fields (chat text, document bodies) and any secrets are never shown.",
    ],
  },
  {
    version: "2.14.0-alpha",
    date: "2026-06-26",
    highlights: [
      "New Developer mode for the technically curious: a plainly-labelled switch under Settings → Developer adds a read-only “Dev” tab that shows PM's internals — system & build info, row counts across the store, a raw table browser, and your corrections log. It's strictly read-only (it never changes your data) and independent of the density preset.",
      "Built so it's safe to share a screen: personal and large fields (chat text, document bodies) are truncated or shown only as a length, and secrets are never surfaced. More features will add their own read-only diagnostics here over time.",
    ],
  },
  {
    version: "2.13.0-alpha",
    date: "2026-06-26",
    highlights: [
      "The Teach tab now has a Preferences section where you can see and shape how PM works for you. Add a preference in plain words (“keep replies short during work hours”) and PM fills in the details, or set it out yourself — scoped to everywhere, one project, or a situation.",
      "Preferences carried over from your old “Learning You” profile show up here marked “Suggested”, ready for you to keep, edit, or remove — so PM only acts on the ones you've vouched for. The old read-only profile box in Settings is gone; it now points you here.",
    ],
  },
  {
    version: "2.12.0-alpha",
    date: "2026-06-26",
    highlights: [
      "PM now keeps a structured memory of how you like things done. Instead of one ever-growing note, your preferences are kept as separate rules — each tied to where it applies (everywhere, one project, or a particular situation) — so chat, sorting suggestions, and your daily briefing draw on just the ones that fit the moment, rather than everything at once.",
      "Your existing “Learning You” profile is carried over into these structured preferences automatically, so nothing you've taught PM is lost. Viewing and editing them lands in the Teach tab in the next update.",
    ],
  },
  {
    version: "2.11.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Projects you've deliberately set up — by renaming, merging, adding a name, or correcting one in Review — now show a “✓ Confirmed” mark in the Teach tab, so you can tell at a glance which ones you've vouched for versus which were filed automatically.",
      "That confirmed status travels with your vault: it's saved alongside your project names in the same encrypted rules file, so copying your vault to another device — or rebuilding the search index — keeps it intact.",
    ],
  },
  {
    version: "2.10.0-alpha",
    date: "2026-06-26",
    highlights: [
      'Index-only items now react to changes at their source: an edit re-indexes automatically, and a file that\'s deleted or a source that goes offline keeps the item findable with a clear badge ("Source missing" / "Source unreachable") instead of silently vanishing. This is the second half of the index-only foundation; the connectors that detect those changes come next.',
      "A renamed or moved item keeps its place — its project, tags, and search position ride a stable id, so reorganising a folder at the source never wipes how you'd filed it.",
    ],
  },
  {
    version: "2.9.0-alpha",
    date: "2026-06-26",
    highlights: [
      'Groundwork for a new "index-only" mode: PM will be able to make cloud files and watched folders searchable by keeping a short summary and a pointer to the original, instead of importing a full copy. This release lands the storage + search foundation; the connectors that add real sources come next.',
      'Documents that work this way are clearly badged "Indexed-only" — they stay findable and their summary reads offline, while the full document is fetched from its source when you open it.',
    ],
  },
  {
    version: "2.8.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Setting up the document engine, importing files, and re-indexing your library now show real progress instead of just a busy shimmer — so you can see how far along things are and roughly how much is left.",
      "It follows your Depth setting: Minimal keeps the calm shimmer, Standard adds a filling bar with an 'X of Y' document count, and Power adds a live percentage. The one-time engine setup and model download still shimmer, since there's no total to count there.",
    ],
  },
  {
    version: "2.7.1-alpha",
    date: "2026-06-25",
    highlights: [
      "Search quality fix: PM had been breaking your documents into far more — and far smaller — fragments than intended, which watered down both document search and chat answers. Each passage is back to the right size, so results are more coherent and to the point.",
      "Because this changes how your library is indexed, PM will suggest a one-time Rebuild for existing vaults. It re-reads your own Markdown files and changes nothing in your documents or notes.",
    ],
  },
  {
    version: "2.7.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Settings is tidier. Everything now lives in five tabs down the side — General, AI & Models, Search, Calendar, and Data & Security — so related options sit together instead of in one long scroll. Nothing was taken away; it's just easier to find.",
      "Settings also follows your Depth setting now. On the Power layout each section opens with its full detail (your spend breakdown, model recommendations, the profile PM has learned about you); on Standard and Minimal that detail starts neatly collapsed and is one click away — so the page stays calm without ever hiding a setting. The first-run welcome is unchanged.",
    ],
  },
  {
    version: "2.6.1-alpha",
    date: "2026-06-25",
    highlights: [
      "Fixed a Windows issue where the document engine could fail to set up the first time you used it — the bundled Python was packaged with its folders flattened, so it couldn't start and showed a confusing error. A clean install now sets up correctly.",
      "If document-engine setup ever fails like that again, PM now recognises it as a PM packaging problem (not something wrong with your computer) and gives you a one-click, pre-filled way to report it.",
    ],
  },
  {
    version: "2.6.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Meet Teach — a new tab for tidying how your projects are named. If the same project ever shows up under two names, merge one into the other in a click and the duplicate stops coming back; rename a project and it updates on every document at once; or add a name you know means the same thing. PM even flags obvious look-alikes for you. It does the same thing as correcting a project in Review — just somewhere you can do it deliberately.",
      "Teach is on for the Standard and Power layouts and tucked away under Minimal — show or hide it anytime under Settings → Appearance. Hiding it only hides the tab; the naming rules you've already taught PM keep working underneath.",
    ],
  },
  {
    version: "2.5.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Corrections stick now. When you fix which project a document belongs to in the sorting review, PM remembers it as a lasting rule rather than a one-off edit — so if it had been proposing the same project under slightly different names (say “PM”, “Personal Manager” and “Atlas - PM”), correcting it once teaches PM they’re the same, and that variant stops reappearing every time you sort new files.",
      "Behind the scenes your projects now have a stable identity, kept in a small encrypted file alongside your library so the rules travel with your vault and survive a re-index. Nothing in your documents or notes is changed — this just records how you like things organised. A dedicated place to rename and merge project names is coming next.",
    ],
  },
  {
    version: "2.4.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Switch your search language anytime — now for any vault, not just new ones. Under Settings → Search you can move a vault between English and Multilingual whenever you like. Multilingual understands 100+ languages and finds meaning across them (not just matching words), so it's the one to pick if your library isn't only in English; the first time you turn it on it downloads a larger language model (about 1 GB, once). Switching re-indexes your library from your own Markdown files — your documents are never changed, and if you're offline it simply leaves things as they were until you're back online.",
      "Good to know if you ever go back: a vault you've switched to Multilingual is built for this version, so opening it in an older copy of PM will make search misbehave until you update again. Your files and notes are always safe either way.",
    ],
  },
  {
    version: "2.3.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Sharper search results: PM now re-ranks what it finds with a second, more precise pass, so chat answers and document search surface the most relevant passages first. It's on by default — the first search downloads a small model — and there's a new “Re-rank search results” switch under Settings → Search if you ever want the fastest, lightest results instead.",
      "Multilingual search for new vaults: when you set up a fresh vault you can now choose a Multilingual search language (great if your notes aren't only in English) instead of the default English. Existing vaults keep working unchanged; switching an existing vault's language (which re-indexes it) is coming in a later update.",
    ],
  },
  {
    version: "2.2.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Better search foundation: PM now splits your documents into smarter, structure-aware pieces — it keeps headings, code blocks, and tables intact and carries each section's heading into what gets searched — so answers draw on more relevant passages. The search models are now swappable behind the scenes too, setting up multilingual support next. After updating, the Documents view offers a one-time “Rebuild” to re-index your existing files with the improvement; your documents and notes aren't changed, just re-indexed.",
    ],
  },
  {
    version: "2.1.5-alpha",
    date: "2026-06-24",
    highlights: [
      "Linking a second Windows account to a shared vault is clearer: Settings → Vault now shows you exactly what to enter (an account name or SID, not a folder path), the PowerShell command to look it up, and the difference between the two — with the stable SID recommended.",
    ],
  },
  {
    version: "2.1.4-alpha",
    date: "2026-06-24",
    highlights: [
      "Document engine setup is more reliable on macOS: PM now finds a modern Python automatically (including Homebrew and python.org installs) and rebuilds its engine after a Python upgrade — so setup just works, with no Terminal steps. If it still can't finish, a new troubleshooting popup explains exactly what to do.",
    ],
  },
  {
    version: "2.1.3-alpha",
    date: "2026-06-24",
    highlights: [
      "Documentation refresh: the README is now up to date with everything PM does today and written to stay current as features land, and the project ships public Contributing and Releasing guides covering how changes are reviewed, gated, and shipped. No change to the app or your data.",
    ],
  },
  {
    version: "2.1.2-alpha",
    date: "2026-06-24",
    highlights: [
      "Clearer downloads: the release page now leads with a simple, one-file install guide for Windows and macOS — it's obvious which single file to grab and how to open it. No change to how you use PM.",
    ],
  },
  {
    version: "2.1.0-alpha",
    date: "2026-06-24",
    highlights: [
      "Shared & portable vaults: you can now protect your vault with a passphrase instead of tying it to this one Windows account — so the same vault can be opened from another profile on this machine. Choose it during setup, or switch anytime in Settings → Vault.",
      "Your files stay yours. A passphrase-protected vault keeps its Markdown encrypted at rest, and you can export everything to plain Markdown at any time with your passphrase — encryption protects your notes, it never locks you in.",
      "Safe to share: only one profile writes at a time. If PM is already open in another profile, you'll get a calm “Continue here?” hand-off instead of two copies racing over your data.",
      "Zero-friction by default: if you don't opt in, your vault stays private to this device exactly as before, with its key held in your OS keychain — nothing new to remember.",
    ],
  },
  {
    version: "2.0.1-alpha",
    date: "2026-06-23",
    highlights: [
      "Behind-the-scenes fixes to how PM is packaged and shipped, so the Windows installer builds correctly and the macOS app bundles cleanly — no change to how you use PM. (This is the build that delivers the first public release.)",
    ],
  },
  {
    version: "2.0.0-alpha",
    date: "2026-06-23",
    highlights: [
      "Welcome to the first public release of PM — your private, local-first personal manager that keeps your documents, your focus, and the moving parts of your day in one calm place.",
      "Everything stays on your machine. Your encrypted store stays encrypted, and PM never sends your data anywhere on its own.",
      "A polished, fully themeable interface — light and dark, accent colours, and depth — plus an optional app lock so a quick glance can't open your things (Windows Hello).",
      "On Windows, PM is completely self-contained: there's nothing extra to install, and from here on it quietly keeps itself up to date.",
    ],
  },
  {
    version: "1.3.5-alpha",
    date: "2026-06-23",
    highlights: [
      "Security hardening (nothing changes in how you use PM): your sensitive values — the database key, your OpenRouter keys, and your Google Calendar tokens — are now held in a wrapper that can never be accidentally written to a log or error message, so they stay protected even as the app keeps growing.",
    ],
  },
  {
    version: "1.3.4-alpha",
    date: "2026-06-23",
    highlights: [
      "Clearer wording about what’s protected at rest: your encrypted store stays encrypted, but the documents in your Markdown vault are stored in the open — so Settings → Data now reminds you to turn on full-disk encryption (BitLocker / FileVault) to protect them.",
    ],
  },
  {
    version: "1.3.3-alpha",
    date: "2026-06-23",
    highlights: [
      "Developer tooling only (no change to the app or your data): PM’s code-quality checks now skip the bundled Python runtime on Windows, so local builds stay clean.",
    ],
  },
  {
    version: "1.3.2-alpha",
    date: "2026-06-23",
    highlights: [
      "The “Day-to-day” model suggestion now skips free models — they’re often rate-limited and unreliable, which caused chat to silently fall back to your second-choice model. It now recommends the cheapest dependable model instead.",
    ],
  },
  {
    version: "1.3.1-alpha",
    date: "2026-06-23",
    highlights: [
      "Fixed the Focus tab getting stuck on its loading placeholder instead of showing your projects.",
      "Focus now opens instantly when you switch back to it from another tab, instead of reloading each time.",
    ],
  },
  {
    version: "1.3.0-alpha",
    date: "2026-06-22",
    highlights: [
      "On Windows, PM is now fully self-contained — you no longer need to install Python yourself before using the document features. Everything PM needs to run ships inside the app.",
      "The document engine still does its one-time setup the first time you add a document (it downloads the tools and models it needs); after that it's ready and offline-capable.",
    ],
  },
  {
    version: "1.2.1-alpha",
    date: "2026-06-22",
    highlights: [
      "Added a security policy (SECURITY.md): how to privately report a security issue, which versions get fixes, and what's in and out of scope. No change to your data or how you use PM.",
    ],
  },
  {
    version: "1.2.0-alpha",
    date: "2026-06-22",
    highlights: [
      "Behind-the-scenes groundwork: PM now runs an automated quality and safety net on every change — formatting, type, security, dependency, and version checks — so updates stay consistent and dependable. No change to your data or how you use PM.",
    ],
  },
  {
    version: "1.1.2-alpha",
    date: "2026-06-22",
    highlights: [
      "Your data now lives in one clearly-named folder — “Personal Manager” — that's easier to find and back up, in the right machine-local spot for your OS.",
      "Your appearance settings and your Pinboard now travel with your data: they're kept inside your encrypted store rather than the browser layer, so backing up or moving your data folder carries them along.",
      "New in Settings → Data: “Open data folder” reveals everything in your file manager, and “Export all data” bundles your documents and store into a single .zip you choose where to save (the store stays encrypted in the archive).",
    ],
  },
  {
    version: "1.1.1-alpha",
    date: "2026-06-22",
    highlights: [
      "Polish: the document Map no longer flashes “No documents yet” while it's loading — it shows a gentle placeholder until your map is ready.",
    ],
  },
  {
    version: "1.1.0-alpha",
    date: "2026-06-22",
    highlights: [
      "Optional app lock: you can now ask PM to check it's you — with Windows Hello (your face, fingerprint, or PIN) — before it opens. Turn it on under Settings → App lock; it stays off until you do, and it only appears where your device can actually perform the check.",
      "It's a convenience lock for the window, not a second password on your data — your store is always encrypted at rest regardless. If your device can't verify for some reason, you can still get in, so you're never locked out of your own app.",
    ],
  },
  {
    version: "1.0.0-alpha",
    date: "2026-06-21",
    highlights: [
      "A whole new look. PM has been rebuilt on a proper design system, so every screen now shares one calm, consistent visual language — the same features you already use, freshly dressed.",
      "Make it yours: a new Appearance section in Settings lets you switch between three visual styles, light or dark mode, an accent colour, and a density from minimal to power-user — and PM remembers your choice on this device.",
      "A custom window frame replaces the stock title bar for a more polished, app-like feel, and dates now read consistently as day-month.",
      "Dates now follow your time zone. PM detects your device's zone automatically — or you can pin one in Settings — so “today”, what's “due soon”, and your calendar agenda all land on the right local day.",
      "Cleaner interactions: long results and source lists collapse into tidy cards you can expand, clicking a citation in an answer highlights the source it came from, and the actions you can't undo — like rebuilding the index or disconnecting a calendar — now ask you to confirm first.",
      "Smoother waits and more room: imports and indexing show a real progress bar, screens fill in with placeholder shapes instead of a blank pause while they load, and you can go full-screen with F11 or the title-bar button.",
      "See what you're spending: a new Usage & cost panel in Settings tracks your model costs — priced from OpenRouter's public rates, refreshed daily — with 30-day and all-time totals, a per-model breakdown, and a nudge toward a cheaper equivalent model when one would do.",
      "Help choosing a model: PM now suggests two — a low-cost, reliable Day-to-day pick and a higher-powered Advanced one for high-stakes chat — each with a short reason and its real price, ready to apply in a click. And every request now enforces zero-data-retention, so providers can't keep or train on your prompts.",
      "New Pinboard: a bounded board for thinking things through — drag, snap, and resize sticky notes and simple timelines. It's a private scratch space; nothing on it is filed or searched.",
      "If an automatic update ever can't install itself, PM now offers a direct download link instead of getting stuck — so you can always reach the latest version.",
    ],
  },
  {
    version: "0.8.6-alpha",
    date: "2026-06-20",
    highlights: [
      "Housekeeping ahead of the move to a single public home — the docs now live in one place alongside the code and releases. No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.5-alpha",
    date: "2026-06-20",
    highlights: [
      "Reliability pass from a full code review: switching chats while a reply is still streaming no longer shows that reply under the wrong conversation, and confirming a sorting review is now all-or-nothing — an interrupted confirm can’t leave some documents half-filed.",
      "Sturdier importing and calendar handling — a huge or unusual document, a long voice clip, or an oddly-scheduled calendar event can no longer balloon memory or stall the app — plus clearer messaging when your store is briefly locked (e.g. by antivirus) instead of it looking like corruption.",
      "Under-the-hood tidying ahead of going open-source. No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.4-alpha",
    date: "2026-06-20",
    highlights: [
      "PM now wears an “Alpha” label — in the app, in its window title, and in its version number — to make its stage clear as it heads toward a public release. It’s feature-complete for v1 and usable day to day, but still under active development, so expect the occasional rough edge.",
      "No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.3",
    date: "2026-06-20",
    highlights: [
      "Defence-in-depth hardening ahead of going open-source: sensible limits everywhere so a hostile file, calendar feed, or huge pasted message can’t hog memory or run up your model bill, and PM now asks your system for only the permissions it actually uses.",
      "Your encryption key is now wiped from memory right after the store is unlocked, the store’s encryption settings are pinned so future updates can always reopen your data, and each update step is all-or-nothing so an interrupted update can’t leave it half-changed.",
      "Housekeeping for the move to a public home — no change to how you use PM.",
    ],
  },
  {
    version: "0.8.2",
    date: "2026-06-20",
    highlights: [
      "Security hardening from a full security review: calendar feeds now must use a secure https link and can’t point at a private or local address — so a feed link can’t be turned into a way to probe your own network, and it’s never sent in the clear.",
      "Plus under-the-hood robustness — a large or malformed document can no longer balloon memory while it’s being imported — with no change to how you use PM.",
    ],
  },
  {
    version: "0.8.1",
    date: "2026-06-19",
    highlights: [
      "Reliability polish: replies that include emoji, accents, or other languages now come through cleanly as they stream; the microphone always switches off the moment you stop recording or leave the chat; and your edits in Review are no longer overwritten by an AI suggestion that lands a split second later.",
      "A downloaded update now stays one click away after you choose “Later”, chat answers come back a little quicker, and a calendar that fails to sync no longer pretends it succeeded — plus a batch of smaller fixes from a full code review.",
    ],
  },
  {
    version: "0.8.0",
    date: "2026-06-19",
    highlights: [
      "Voice input: there’s now a microphone button in the chat box — click it, speak, and PM turns your words into text you can edit before sending. It works in both your main chat and a project’s chat.",
      "Private by design: your voice is transcribed entirely on your device — no audio ever leaves it. The first time you use it, PM downloads a small speech model once (about 145 MB), then it works fully offline.",
    ],
  },
  {
    version: "0.7.0",
    date: "2026-06-19",
    highlights: [
      "Daily briefing: your Focus screen now opens with a short “here’s your picture today” summary — what’s due soon, quick wins you could knock out, anything that’s gone quiet, and what’s on your calendar — written for you and grounded in your real projects.",
      "It refreshes itself about once a day when you open Focus, and there’s a Refresh button to rebuild it from your current state any time. It learns your voice from the same profile that powers PM’s suggestions.",
      "Everything stays on your device — the briefing is built from data PM already has, and only the model call leaves.",
    ],
  },
  {
    version: "0.6.0",
    date: "2026-06-19",
    highlights: [
      "Calendars (read-only): connect a calendar to see an Upcoming agenda on your Focus screen and ask chat things like “what’s on this afternoon?”. Everything stays on your device — only the fetch to your calendar leaves.",
      "Two easy ways to connect: paste a calendar’s private iCal feed URL (simplest — no sign-in, and it works even with Google Advanced Protection), or use full Google sign-in with your own credentials under Settings → Advanced.",
      "Due soon, automatically: when an upcoming event’s title names one of your projects, that project flips to “Due soon” and shows the event — no need to set a deadline by hand.",
      "You stay in control: feed URLs and any tokens live only in your OS keychain, and removing a feed or disconnecting wipes its synced events.",
    ],
  },
  {
    version: "0.5.0",
    date: "2026-06-19",
    highlights: [
      "Command palette: press Ctrl+K (⌘K on a Mac) — or click Search in the sidebar — to jump straight to any project, file, or past conversation. Just start typing.",
      "Pick a file and PM opens its project with that file highlighted; pick a project, a chat, or a 'Go to' destination and it takes you there — no clicking through the sidebar.",
    ],
  },
  {
    version: "0.4.0",
    date: "2026-06-18",
    highlights: [
      "Focus view: your new home screen shows every project on one page, each with a single honest status — Due soon, Quick win, Take a look, Blocked, Part of, or On track — so you can pick the one right thing to look at.",
      "Triage your projects: give a project a size, a deadline, a blocker, or a parent — by hand, or let the AI suggest them and just confirm. The status updates to match.",
      "Per-project view: click a project to narrow everything to just it — its files alongside a chat that answers only from that project's documents.",
    ],
  },
  {
    version: "0.3.0",
    date: "2026-06-18",
    highlights: [
      "Pick any model: the model picker now lists every model on OpenRouter — search by name, see each one's input/output price, sort by price, and read at-a-glance tags (Free, Reasoning, Vision, Coding, Long context…).",
      "Separate chat and background models: choose one model for your chats and a different one for behind-the-scenes work (sorting and learning) — handy for pairing a strong chat model with cheap or free background models.",
      "Auto-switch on limits: give each a ranked list of models and PM falls through to the next when one hits its daily limit, so you're never stuck mid-task.",
      "A models tag in the sidebar shows which chat and background models you're using right now, without opening Settings.",
      "Organise & Review: PM proposes a project, tags, and an importance level for each new document; your corrections are remembered.",
      "Learning You: a short, readable profile distilled from those corrections, fed back into PM's suggestions and chat so it gets more like you.",
      "Document map: a visual graph of your documents grouped by project, plus a Help mode that explains any part of the app on hover.",
    ],
  },
  {
    version: "0.2.0",
    date: "2026-06-17",
    highlights: [
      "Automatic updates: PM now updates itself in the background and offers a one-click restart when a new version is ready — no manual reinstalls.",
      "This “What's New” view, so you can always see what changed since your last version.",
    ],
  },
  {
    version: "0.1.0",
    date: "2026-06-15",
    highlights: [
      "Encrypted local store and streaming chat through OpenRouter.",
      "The Archivist: drag files or folders into Documents to convert, embed, and index them locally, with a rebuildable Markdown vault as the source of truth.",
    ],
  },
];
