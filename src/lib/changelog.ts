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
    version: "2.62.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Filing suggestions now take a hint from your Drive folders. When PM proposes a project for a file synced from Google Drive, it quietly considers the folder the file was found in — so a document sitting in your “Taxes 2025” folder is more likely to be suggested under the right project. It’s only ever a hint: every document still passes through your review before anything is filed, and the folder name never changes how the file is searched.",
    ],
  },
  {
    version: "2.61.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Calendar polish and a thorough reliability pass. The Day scale now opens on your daytime hours and fills the pane (and still scrolls to the small hours), the 24-hour scale no longer leaves a sea of empty space, and the Year view scales to fill the window in every look. Multi-day events read as a proper bar, and the settings pop-up no longer shows the “now” line through it.",
      "Fewer events quietly go missing. Daily repeating events from an iCal subscription now show for the whole year (not just the first ~12 months), an event that’s still running from earlier stays on today’s agenda, a busy month shows a “+N more” instead of dropping overlapping multi-day events, and the same event synced from two calendars appears once. Outlook repeating events now show every occurrence, and all-day events no longer disappear halfway through their own day.",
      "Steadier under the hood. Recurring events land on the right time across daylight-saving changes and whatever machine synced them, a calendar that partly fails to sync is flagged rather than shown as fine, and disconnecting an account reliably clears its saved sign-in. The clock line and “today” now advance on their own.",
    ],
  },
  {
    version: "2.60.0-alpha",
    date: "2026-07-02",
    highlights: [
      "The Calendar now speaks Terminal. Switch PM to the Terminal look and every view — Month, Week, Day, Year, and Agenda — turns into a crisp monospace layout: a “~/pm ❯ cal” status line, square day markers, and green kept strictly for today. (Week and Day read as a tidy day-by-day agenda rather than a pixel grid, which suits the mono style.)",
      "Small touches everywhere else, too. Page through time from the keyboard — ← and → step by period and “t” jumps back to today — each view fades in gently as you switch (and holds still if you’ve asked for reduced motion), and PM now tells you when you’ve paged past the stretch of time it keeps synced.",
    ],
  },
  {
    version: "2.59.0-alpha",
    date: "2026-07-02",
    highlights: [
      "The Calendar grows into a real calendar. Alongside Agenda, there are now Month, Week, Day, and Year views — switch between them from the header, and page through time with the arrows or by picking a date straight off the pop-up mini-calendar.",
      "Week and Day show a proper time grid: events sit at their real hours, side by side when they overlap, with a live “now” line on today and an all-day strip up top. Choose how tall the grid runs — business hours, the whole day, or a compact 24-hour view.",
      "Month lays out your weeks with multi-day events flowing across as bands and the rest as tidy per-day chips (they shrink to coloured dots when you keep things minimal). Year gives you all twelve months at a glance — click any day to dive in. Every view stays colour-coded by source and themed to match the rest of PM.",
    ],
  },
  {
    version: "2.58.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Your calendars, together at last. A new Calendar tab gathers every calendar you’ve connected — Google, Outlook, and iCal — into one read-only agenda, colour-coded by source. Show or hide any calendar on the fly (it’s instant, nothing re-syncs), step through by month, and hit Refresh for the latest; PM also keeps the view quietly up to date in the background.",
      "Under the hood, PM now mirrors a full year ahead (and the month behind), not just the next three weeks — so the calendar has the room to grow. Month, week, day, and year views are coming next.",
    ],
  },
  {
    version: "2.57.1-alpha",
    date: "2026-07-01",
    highlights: [
      "Chat polish, under the hood. Long general chats now genuinely stay cheap as they grow (the running summary is cached properly again), and a chat that runs on never balloons its cost even if the summary falls behind.",
      "Your conversations survive a re-index intact. Rebuilding search now keeps each chat’s identity — answers still jump to the exact turn they drew from, and a chat you filed into a project (or archived) stays exactly where you put it.",
      "A preference PM notices in chat truly waits for you. It stays a suggestion in Teach and never steers a reply until you’ve kept it. Plus smaller fixes: the context meter reads true even when a backup model answers, deleting a conversation leaves nothing behind, and auto-naming an old chat no longer bumps it to the top of the list.",
    ],
  },
  {
    version: "2.57.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Chat can now show its work. Open “Explain retrieval” under any conversation to see exactly which of your notes it pulled and how each one scored. Drag one dial — depth — to widen the pool the ranker gets to weigh (not just how many results show), then hit “Use this depth” to make it stick.",
      "Not sure why it missed something? Describe it in plain words and PM will tell you what to try and why — it only ever suggests, and never changes a setting on its own; the dial stays in your hands. (Shows up on the fuller layouts; hide it any time from Appearance.)",
    ],
  },
  {
    version: "2.56.0-alpha",
    date: "2026-07-01",
    highlights: [
      "You can now delete a past conversation: hover it in the sidebar, hit the trash, confirm — and it’s gone for good, along with its messages and everything it had added to search. Nothing’s left dangling behind the scenes.",
    ],
  },
  {
    version: "2.55.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Chats now wear their colours everywhere they show up: a little 💬 marks them in your review inbox and in a project’s file list, so at a glance you can tell a conversation apart from a document you saved.",
      "When PM suggests a preference it picked up from something you said in chat, Teach now says “Suggested from chat” — so you know where the idea came from before you decide whether to keep it.",
    ],
  },
  {
    version: "2.54.0-alpha",
    date: "2026-07-01",
    highlights: [
      "PM now notices when you state a preference in a chat — say “I always want dates as DD-MM-YYYY” or “keep replies short for the Atlas project” and it quietly turns up in Teach as a suggestion you can keep with one click (or ignore). It only ever picks up things you actually said, never guesses, and it never applies one until you’ve kept it.",
    ],
  },
  {
    version: "2.53.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Your conversations now sort themselves like everything else. A chat you start inside a project files itself there straightaway (marked important, kept close to hand), while a general chat drops into your review inbox for you to place — the same approve-or-correct step your documents already get.",
      "Nothing stays wrongly buried: pick a chat back up and let it turn into a real discussion, and PM re-evaluates it — a throwaway you'd archived comes back out of the archive, and an already-filed chat returns to review so it can be re-placed.",
      "Archiving now truly tidies up — an archived chat or document drops off the semantic Map, so the picture stays about what matters (it's still there in search whenever you go looking).",
    ],
  },
  {
    version: "2.52.0-alpha",
    date: "2026-07-01",
    highlights: [
      "When an answer draws on one of your past chats, it now says so — the source reads “from [the chat’s name], [date]” instead of looking like a plain file. Click it and PM jumps you straight back to that conversation, scrolled to the exact turn it drew from, so you can see the full context behind the answer.",
    ],
  },
  {
    version: "2.51.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Each project now keeps its own chat history, right where you'd expect it — in the left sidebar under Conversations, just like the main chat. Open a project and its past conversations are listed there; click any one to pick it back up.",
      "The project panel on the right now gives Milestones and Files their own space, split by a divider you can drag to suit each project. Where you set it is remembered everywhere, even on another computer.",
      "Starting fresh is one click (or one word): the “+ New” button — or just typing /new (or /done) — tucks the current conversation into history and opens a clean one, without losing anything. And reopen a project chat that's been quiet for more than a day and PM gently offers to start a new one, so a fresh train of thought doesn't get tacked onto yesterday's.",
    ],
  },
  {
    version: "2.50.0-alpha",
    date: "2026-06-29",
    highlights: [
      "Your chats name themselves. After a few exchanges, PM gives a conversation a short, fitting title (using the background model, so it never slows down the chat you're having) — and you can rename any conversation just by clicking its title.",
      "Reopen an old conversation and PM marks it as resumed, with the date it was last active, so you always know whether you're picking up an older thread or carrying on a current one.",
    ],
  },
  {
    version: "2.49.0-alpha",
    date: "2026-06-29",
    highlights: [
      "Chat now shows how full the current model's context is — a quiet meter by the message box, tuned to whichever model you're using. As a long conversation approaches that model's limit, PM offers to compress it (folding the older turns into a short summary to free up room, and showing you exactly what was condensed so you can undo if needed), switch to a model with a bigger context, or simply carry on.",
    ],
  },
  {
    version: "2.48.0-alpha",
    date: "2026-06-28",
    highlights: [
      "PM now shows how full the current model's context is. Each model has its own size limit, so the meter is tuned to whichever model you've picked — giving you a quiet heads-up as a long conversation starts to fill it up. (The on-screen meter itself arrives in the next update; this release adds the model-aware measurement and the controls behind it.)",
    ],
  },
  {
    version: "2.47.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Long conversations are now cheaper and sharper. Instead of resending your whole chat history every message, PM sends the recent turns word-for-word plus the running summary of everything earlier — and keeps that stable part cached, so each new message costs far less as a thread grows.",
      "PM no longer talks over itself: when it looks things up to answer you, it skips anything that's already visible in the recent conversation, so it pulls in genuinely new context from your other notes and chats rather than echoing what's on screen.",
    ],
  },
  {
    version: "2.46.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Long chats now stay affordable and stay on-point. PM keeps a short, running summary of the earlier part of each conversation, updated quietly in the background as you talk — so it can remember the gist of where you've been without re-reading the whole thread every message. The full conversation is always kept word-for-word in your vault; the summary is just a lightweight memory aid PM can rebuild any time.",
    ],
  },
  {
    version: "2.45.0-alpha",
    date: "2026-06-28",
    highlights: [
      "PM now keeps up with your conversations as you go. While you're not actively chatting, it quietly indexes new back-and-forths in the background — so a long session becomes searchable bit by bit, and you no longer have to reopen the app for it to catch up. It always waits for a lull, so it never gets in the way of what you're doing.",
      'Small talk stays out of the way: a quick "thanks" or "ok" no longer clutters what PM can recall — only the parts of a conversation that actually carry something (a decision, a fact, an answer) become searchable. Your full chat is always kept either way.',
    ],
  },
  {
    version: "2.44.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Your conversations with PM start becoming searchable. PM now quietly turns each completed back-and-forth into part of its memory — so a decision, fact, or preference you mentioned in chat can resurface later when it's relevant, the same way your documents do.",
      "It happens on its own and stays out of your way: when you reopen PM, it catches up on any chat that hasn't been indexed yet, in the background. (The ongoing while-you-work indexing and the skip-the-small-talk filter arrive in the very next update.)",
      "Each piece of a chat keeps its own date, so an old conversation with one fresh decision isn't treated as entirely stale — the fresh part can still surface when it matters.",
    ],
  },
  {
    version: "2.43.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for a big one: your conversations with PM are on their way to becoming a first-class, searchable source — so a decision or preference you mention in chat can be recalled later, just like a document. This update lays the quiet foundation for how a chat is saved and tracked; nothing changes in how chat looks or works yet.",
      "Under the hood, each completed back-and-forth is now kept in your vault as it happens — the same durable, rebuildable place your documents live — so once indexing arrives in the next updates, your past chats become part of what PM can draw on.",
    ],
  },
  {
    version: "2.42.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Drop in photos and screenshots — the feature is now fully wired up. The first time you add a photo, PM offers to install on-device text recognition (a one-time ~70–100 MB download). You can skip it and still add the photo (indexed by its date and location), then turn it on later — your choice, no pressure.",
      "New option when adding photos: “Save a copy in the vault.” It's off by default — PM just references your photos where they are — but flip it on to keep a copy inside PM (handy for screenshots you delete after), and it follows your vault's encryption. Re-dropping a photo you already added, with this turned on, now saves the copy too.",
      "Manage text recognition in one place under Settings → Storage: it sits with the other on-device components, where you install it or remove it (and its image libraries) to reclaim space, with clear confirmations. Removing it just means new photos are indexed by date and location only.",
    ],
  },
  {
    version: "2.41.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Photos and screenshots can now be ingested (engine side): drop in a JPG/PNG/WebP/HEIC and PM reads the text inside it (when text recognition is enabled), pulls the capture date and location from the photo, and makes it searchable alongside your documents — including a clean “that screenshot from March” style of recall.",
      "Your originals stay where they are by default; nothing is copied unless you ask. (The drag-and-drop with the “save a copy into my vault” option arrives in the next update.)",
      "Photos rebuild from your vault like any document, and the recognized text is never re-run on a rebuild — so re-indexing stays fast.",
    ],
  },
  {
    version: "2.40.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for a new feature arriving shortly: dropping in photos and screenshots so the text inside them becomes searchable, on-device. This update lays the storage and text-recognition plumbing; the drag-and-drop and the “read the text in my screenshots” experience land in the next couple of updates.",
      "Text recognition (OCR) is an optional add-on you choose to install — it won't bloat PM if you never use it, and you'll be able to remove it again any time from Settings → Storage.",
    ],
  },
  {
    version: "2.39.1-alpha",
    date: "2026-06-28",
    highlights: [
      "Fixed a bug where a document with a very long unbroken run of text (a dense table, a code dump, a no-spaces blob) could fail to be indexed — the embedder choked on the over-long piece and skipped it silently. Such pieces are now sized correctly and, as a safety net, always trimmed to fit, so the whole document gets indexed.",
      "Because that changes how a few documents are broken into pieces, PM will re-index your library once on the next launch — no action needed; search just works once it finishes.",
      "Quieted a stream of harmless “FontBBox” warnings some PDFs printed to the log during import.",
    ],
  },
  {
    version: "2.39.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Set a project's priority yourself. Triage now has a Priority picker (High / Medium / Low, or Auto for no tag) — the honest replacement for the old guessed tag. It shows on the project's card and you can sort by it.",
      "Sort your focus list. A new Sort control reorders your projects by Deadline, Priority, Size, or most-recent activity — and the ↑/↓ button flips the direction. “Smart” (the default) keeps the most pressing first, just as before; your choice is remembered.",
    ],
  },
  {
    version: "2.38.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Chatting in a project or editing its milestones now counts as activity. A project's “active” date — and whether it's gone quiet enough to read “Take a look” — used to move only when its documents changed; now engaging with the project itself keeps it current.",
      "A project's priority is now something you set, not a guess. The old “high/medium” tag was inferred from your most important document in the project, which didn't really say whether the project mattered — so it's gone. You'll set priority yourself in Triage in the next update; until then a project simply shows no priority tag.",
    ],
  },
  {
    version: "2.37.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Milestones got tidier. The name and date fields now fit whatever width you give them, so the side panel never has to scroll sideways — and the date box no longer gets clipped when you triage a project on the Focus view.",
      "Marking a milestone done is now obvious: tick its checkbox and it gets a “Done” tag with a line through it. The nearest one you still haven't ticked keeps driving the project's “Due soon” status, as before.",
      "You can resize a project's side panel — drag its left edge to make it wider or narrower. The width is kept in proportion as you resize the app and remembered for next time.",
    ],
  },
  {
    version: "2.36.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Projects can now have more than one deadline. A project usually has several — a pitch, a presentation, an internal due date — so the single deadline box is replaced by a list of milestones, each with its own label and date. Add them when you triage a project on the Focus view, or in the project's own page. Your existing deadline is carried over as a milestone automatically.",
      "Your project's status stays a single, honest signal: “Due soon” now follows the nearest milestone you haven't ticked off yet — finish your pitch, tick it, and the next deadline quietly takes over.",
      "Link a milestone to a calendar event and its date keeps itself in sync with your calendar (which stays the source of truth) — handy for the deadlines that are really meetings. Tap 📅 next to a milestone's date to pick the event.",
    ],
  },
  {
    version: "2.35.0-alpha",
    date: "2026-06-28",
    highlights: [
      "More groundwork for project milestones: PM can now track several deadlines per project under the hood, and a project's focus status (“Due soon”, etc.) is worked out from whichever milestone is nearest and still unmet — so finishing your pitch automatically lets the next deadline take over. The list editor to add and tick off these milestones arrives in the next release; nothing changes in what you see today.",
    ],
  },
  {
    version: "2.34.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for project milestones: a project will soon be able to carry several dated deadlines — a pitch, a presentation, an internal due date — instead of just one. This release lays the storage for that; your current single deadline is carried over automatically, so nothing changes in how you use PM yet. The new milestone list arrives in an upcoming release.",
    ],
  },
  {
    version: "2.33.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Shared (Team) drives are now indexed just once across your Google accounts. If two connected accounts can both see the same shared drive, PM no longer indexes it twice — whichever account syncs it first “owns” it, and that drive shows up greyed out (“Already synced by …”) when you expand your other accounts. Disconnect the owning account and another account with access automatically takes it over on its next sync; a shared drive's files only become “source unreachable” once no connected account can reach it.",
      "Upgrade note: your existing shared-drive items are rebuilt under the new shared index on the next sync (they're index-only pointers and summaries, so nothing real is lost). Press “Sync now” to refresh them.",
    ],
  },
  {
    version: "2.32.0-alpha",
    date: "2026-06-27",
    highlights: [
      "You can now connect a Google account that signs in with its own Cloud project, separate from the shared one. This is what makes Google Advanced Protection accounts work — Google blocks those from using any shared third-party project. Under the Drive or Calendar connect button, open “Advanced Protection account? Use its own project”, paste that project's Client ID + secret, and sign in; PM remembers it for that account and uses it for every later refresh.",
      "Because two Advanced-Protection accounts can't share a project, each can carry its own — so you can connect several of them, one project apiece. A project entered for an account's Drive also covers its Calendar (same Google account).",
    ],
  },
  {
    version: "2.31.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connecting a Google Drive or OneDrive account no longer starts indexing straight away. The account now lands ready-but-unsynced with its scope chooser already open, so you can pick what to index first — your whole drive, just certain folders, or (for Google) which shared drives — and then press “Sync now” to start. Nothing is fetched until you do.",
      "Choosing what to sync now saves as you go — the Save button is gone. Tick My Drive, switch a drive to “Choose folders”, or add a shared drive and it's stored immediately; it takes effect the next time you press “Sync now”.",
    ],
  },
  {
    version: "2.30.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Multi-calendar is here. You can now connect several Google Calendar accounts — each shows under Google with its own list of calendars to sync.",
      "Outlook / Microsoft 365 calendars now connect with a read-only sign-in, right under Microsoft — pick which calendars to sync, just like Google.",
      "Apple iCloud calendars have their own spot under Apple — add one by its public iCal link (no Apple sign-in needed).",
      "Everything you connect — Google, Outlook, Apple, and any iCal link — flows into one read-only calendar that powers your agenda, “what's on tomorrow” in chat, and the “Due soon” status. Read-only for now; nothing is written back to any provider.",
    ],
  },
  {
    version: "2.29.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Groundwork for multi-calendar support: behind the scenes, your calendar now records each connected account and individual calendar in a cleaner structure — the foundation for bringing several Google accounts, Outlook (Microsoft 365), and Apple calendars together in one place. How you use it doesn't change yet; the new controls arrive next.",
      "Each event now keeps its stable calendar ID, so a future release can link an event to the project or deadline it belongs to.",
      "Your existing Google Calendar connection carries over automatically — no need to reconnect.",
    ],
  },
  {
    version: "2.28.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Connectors tab got a visual tidy-up — far fewer nested boxes — so each provider's sign-in, accounts, and options are easier to follow at a glance.",
      "Indexing speed (Fast / Gentle) now tucks its explanation behind a short “What do Fast and Gentle do?” toggle, open by default only on Power density. The summary also makes clear it paces Drive and file indexing (email later) — calendars are tiny and always sync at full speed.",
      "The “Using more than one Google account?” help now links straight to the right Google Cloud page (Audience → Test users); the old link landed on a dead overview screen. It also explains that an app left in Testing mode signs accounts out after 7 days, and recommends publishing it to Production for a connection that lasts.",
      "The “first sync indexes your entire Drive” note now appears only while that first indexing is actually running — right by the progress bar — and tucks itself away when it finishes, reappearing when you add a new account. Same for OneDrive.",
      "You can now start a sync for another connected account while one is already indexing: its “Sync now” stays enabled and the account joins the queue (it shows “Queued” until its turn). Same for OneDrive.",
      "Google Calendar now shows which account you're signed in as, above the list of calendars to sync — handy when you have several Google accounts.",
      "The calendar-subscription guide now covers Outlook and Apple iCloud as well as Google, with step-by-step instructions for finding each one's private iCal/ICS link.",
    ],
  },
  {
    version: "2.27.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connecting a second Google (or Microsoft) account now works properly: when you click “Add another account”, the sign-in always shows the account chooser, so you can pick a different account instead of it silently re-using the one you're already signed in with.",
      "The Connectors tab is now grouped by provider — Google, Microsoft, Apple — instead of by Calendar / Drive / Email. Each provider's sign-in is set up once at the top of its group and shared across all of that provider's services (Google: Calendar + Drive; Microsoft: OneDrive), so a calendar-only user no longer has to hunt around the Drive section to find it.",
      "New “Using more than one Google account?” help sits right under the Google sign-in: reuse the one project, add each account under Test users, then pick it from the chooser — no second project needed.",
      "When you connect your first Google or Microsoft account, a short note reminds you that the account you connect first heads the list, so you can connect your main one first.",
      "Calendar subscriptions (iCal, no sign-in) now have their own clearly-labelled section, since they don't belong to any one provider.",
    ],
  },
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
