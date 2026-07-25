// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// In-app changelog shown in the "What's New" view. The app auto-opens it once
// after updating to a version the user hasn't seen yet (see App.tsx), and it's
// always reachable from the sidebar.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ RELEASE CHECKLIST: add a new entry at the TOP for every release, with the │
// │ version matching package.json / tauri.conf.json / Cargo.toml. Newest      │
// │ first. See RELEASING.md (repo root).                                      │
// └─────────────────────────────────────────────────────────────────────────┘

export interface ChangelogEntry {
  version: string; // matches the released app version, no leading "v"
  date: string; // YYYY-MM-DD
  highlights: string[]; // short user-facing bullet points
  /** True only on entries that map to an actual tagged GitHub release (RELEASING.md §2), so What's
   *  New can mark the release boundaries among the interim per-PR dev bumps. Omitted (falsy) on a
   *  dev-bump entry — which is what the top entry is on a developer machine between releases. Set
   *  when a release is cut; the `--tag` release gate refuses to tag if the top entry lacks it. */
  release?: boolean;
}

export const CHANGELOG: ChangelogEntry[] = [
  {
    version: "3.66.0-alpha",
    date: "2026-07-25",
    highlights: [
      "PM has a new Accessibility tab in Settings, with its first set of options. You can scale all of PM's text up or down (Small to XL) -- the same “Text size” control also sits under Appearance, since it's handy for everyone. You can turn animations off regardless of your device's motion setting. And you can switch the whole interface to Atkinson Hyperlegible, a typeface designed to be easy to read and to tell letters apart (numbers and code keep their usual font). These are opt-in and travel with your data -- nothing changes until you turn something on, and each has a Reset. More options are on the way to this tab: larger touch targets, colour-blind-friendly palettes, and a high-contrast theme.",
    ],
  },
  {
    version: "3.65.0-alpha",
    date: "2026-07-25",
    highlights: [
      "More of PM now works with the keyboard and with screen readers. The “drop files here” area, the file rows in Documents and in a project's file list, and pinboard notes can all be reached and opened with the keyboard now, not just the mouse. Screen readers announce the assistant's reply once it finishes, read progress bars as “3 of 10” rather than a bare percentage, and speak the voice-recording status. The vault passphrase box and its error messages are now properly labelled and announced. And the quick-jump palette (Ctrl/Cmd+K) describes its results list to assistive tech. It's all under the hood -- nothing looks different on screen.",
    ],
  },
  {
    version: "3.64.0-alpha",
    date: "2026-07-25",
    highlights: [
      "PM is starting to work better for keyboard and screen-reader users, and this is the groundwork. Open a dialog now and your keyboard focus stays inside it -- Tab moves between its own buttons instead of drifting onto the page behind -- and closing it puts focus back where you were. A clear outline follows the keyboard as you Tab around, so you can always see what's selected; it only appears for keyboard use, so nothing looks different when you're clicking with the mouse. Collapsed sections no longer quietly hold onto hidden focus, and screen readers can now announce which section of the sidebar you're on. The options you'll actually see -- text size, higher contrast, colour-blind-friendly palettes and reduced motion, gathered in a new Accessibility tab -- are coming in the next few updates.",
    ],
  },
  {
    version: "3.63.0-alpha",
    date: "2026-07-25",
    highlights: [
      "What's New now marks which versions were actual releases. Between releases there are lots of small interim versions (one per change we make), so it wasn't obvious which entries were the ones that actually shipped to you. Tagged releases now carry a “Release” badge here, so you can see at a glance when the last real release was and read every change that landed in each version along the way. (On a developer's own machine the newest version is usually an interim build with no badge — that's expected.)",
    ],
  },
  {
    version: "3.62.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now undo a project-name decision in Teach, and PM protects your inbox. In the Teach tab, hover over any of a project's alternate names and a small × appears -- click it to remove that name, so it stops being treated as the project from now on. Honest note: this reverses the name and pulls any files still literally saved under that exact name back out into their own project, but it can't retroactively un-merge files an earlier merge already renamed to the project you kept (PM doesn't store that history) -- for those, restore a backup from before the merge. Separately, the special “Unsorted” inbox is now protected: it can't be merged into another project or renamed away, so a stray click can never sweep everything waiting to be sorted into the wrong place.",
    ],
  },
  {
    version: "3.61.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Power users: progress bars now show how long the current task has been running. With the Power display density on, a live timer appears on the right of loading bars -- during ingests, re-indexing, syncs, backups and downloads -- next to the percentage or item count. It even ticks during the early “setting up” phase where there's no count yet, which is exactly when a long download benefits from it. One honest note: for a bar that reappears part-way through a task (say you reopened the Backup panel mid-upload), the timer counts from when it reappeared, not the original start.",
    ],
  },
  {
    version: "3.60.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now bring your preferences over from another AI. If you've built up a “memory” in ChatGPT, Gemini or Claude, go to Settings → AI & Models → Import AI memory: copy the prompt there, run it in that assistant, and paste its reply back. PM turns it into tidy preference records and stages them in Teach → Preferences as suggestions -- nothing changes how PM works for you until you review each one and click Keep. Whatever you paste is treated as data, never as instructions.",
    ],
  },
  {
    version: "3.59.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Click any event on the calendar to see its full details in a pop-up. Until now only your own milestones and pinboard notes responded to a click; now every event does. The pop-up shows everything PM has synced for it -- which calendar it's on, when, whether you're marked busy or free, the location, the guest list and organiser, a video-call link, whether it repeats, and the full description -- plus quick buttons to open it in its source calendar (Google / Outlook), jump to its linked project, or open the Pinboard. To make this useful, PM now pulls in those richer details (busy/free, guests, organiser, meeting links, recurrence) from Google, Outlook and subscribed (Apple / iCal) calendars, so the aggregated view finally carries what you'd see in the original app. Existing calendars fill in the new details on their next sync.",
    ],
  },
  {
    version: "3.58.1-alpha",
    date: "2026-07-24",
    highlights: [
      "The calendar reads more clearly at a glance. Timed events now carry a soft tint of their calendar's colour (the way all-day events already do), instead of a plain grey block that blended into the grid. The current day's column keeps its hour and day lines visible even on the quieter colour themes, where they used to wash out. And events that have already finished are gently greyed, so what's still ahead stands out from what's done.",
    ],
  },
  {
    version: "3.58.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now back up on demand to a connected cloud, and PM helps keep that cloud tidy. On the Backup screen, each connected Proton Drive or Google Drive gets a \"Back up now\" button that uses your remembered passphrase and keeps only your most recent few backups (the keep-last-N you set) -- just like an automatic run, with no need to re-type the passphrase. And if a destination already holds more backups of this vault than your limit (say you had been saving manually for a while), PM shows a one-time note offering to either keep them all (raising the limit to match) or tidy up now by trashing the oldest down to your limit. It only ever counts and trims this vault's own backups, so a shared Drive folder is never touched by mistake, and nothing is hard-deleted -- trimmed backups go to the cloud's trash.",
    ],
  },
  {
    version: "3.57.1-alpha",
    date: "2026-07-24",
    highlights: [
      "Review now remembers the AI's suggestions between sessions. Before, if you closed PM with items still waiting in Review, re-opening it would ask the AI to suggest a project, tags and importance for the whole queue all over again -- quietly spending credits on work it had already done. PM now saves each suggestion as it arrives and reloads it on startup, so it only asks the model about genuinely new, un-suggested items. Re-propose still regenerates everything from scratch whenever you want a fresh take.",
    ],
  },
  {
    version: "3.57.0-alpha",
    date: "2026-07-24",
    highlights: [
      'macOS: PM now asks for your keychain permission once at startup instead of once for every saved secret. If you use PM on a Mac, you may have seen it show several login-password or "Always Allow" prompts in a row while it loaded — macOS asks separately for each stored item (your database key, API key, and calendar or cloud sign-ins), and PM used to keep each in its own keychain entry. It now stores them together in a single entry, so one "Always Allow" covers everything and the repeated prompts stop. Windows and Linux were never affected. One honest note: that single approval is a little broader than before — allowing it lets PM read all of its own saved secrets at once rather than one at a time — and none of them ever leave PM.',
    ],
  },
  {
    version: "3.56.2-alpha",
    date: "2026-07-24",
    highlights: [
      "Security housekeeping: updated a networking library in our dependencies to a patched version as a precaution. PM doesn't use the affected feature, so nothing changes for you — it just keeps our supply chain clear of known advisories.",
    ],
  },
  {
    version: "3.56.1-alpha",
    date: "2026-07-24",
    highlights: [
      "Linux: fixed a harmless-but-noisy crash report that popped up every time you closed PM (a WebKitGTK renderer shutdown quirk on some graphics drivers). Nothing was actually lost — your data is saved well before the app exits — but the app now shuts down cleanly instead of leaving a crash notification behind.",
    ],
  },
  {
    version: "3.56.0-alpha",
    date: "2026-07-24",
    highlights: [
      'Google Drive now indexes the files and folders people share with you. "Shared with me" is a separate place in Drive — apart from your own My Drive and from shared (Team) drives — and until now PM couldn\'t reach it, so anything a colleague shared straight to you (especially the contents of a shared folder) was invisible. Open a Google account under Settings → Connectors → Drive, turn on "Shared with me", and pick the specific files or folders you want: a folder brings its whole contents, shortcuts are followed to the real file, and if you connect two accounts on the same team a file shared with both is indexed just once. Everything stays read-only and index-only, like the rest of Drive.',
      'When you index only some folders of a Drive, you can now also include the loose files sitting in the drive\'s root — the ones not tucked inside any folder — with a checkbox under "Choose folders".',
    ],
  },
  {
    version: "3.55.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Filed a document from a synced folder in Review? You can now file the rest of that folder the same way in one click. When the file came from a folder you're indexing (a Drive/OneDrive folder or a local folder), the row offers to \"apply this filing to the N other files from that folder\" — you pick the project, tick or untick any you don't want, and they're all filed at once. It's a plain, undo-able filing action (no AI needed), and it matches files by their actual folder, so two folders that happen to share a name are never mixed up.",
    ],
  },
  {
    version: "3.54.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Filing documents in Review no longer waits on the AI. You can now approve each item the moment its suggestion is ready — a row's Approve button lights up as soon as that file is sorted, so you can clear the done ones while the rest are still being worked out, instead of waiting for the whole batch.",
      "AI suggestions in Review are now something you turn on, not a requirement. On a fresh install they start off, with a banner offering to switch them on — a real help when you're importing a lot — and you can also toggle them under Settings → AI & Models. When they're on but can't run, Review tells you why (no model linked, no credits, a local endpoint that's unreachable) and lets you set the project, tags and importance yourself and approve. The AI is always a help, never a gate.",
    ],
  },
  {
    version: "3.53.0-alpha",
    date: "2026-07-24",
    highlights: [
      "The Focus tab's “Upcoming” box can now show a short day-by-day calendar instead of a plain list. Switch between List and Days from the toggle at the top of the box (or under Settings → General → Focus). The Days view shows up to four days side by side as an hour grid — with the same Work / Day / 24h hour options as the calendar — and the ‹ › arrows step it a day forward or back, with a Today chip to jump straight back. It opens on today and stays where you leave it while you're on the tab. List stays the default.",
    ],
  },
  {
    version: "3.52.2-alpha",
    date: "2026-07-24",
    highlights: [
      "Milestones on the calendar now show which project they belong to. A milestone appears as an all-day item on its due date; it now reads as “milestone · project”, so two milestones with similar names in different projects are easy to tell apart at a glance.",
    ],
  },
  {
    version: "3.52.1-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now make the left sidebar noticeably thinner. Its minimum width is now half what it was, so you can shrink it down and give the main view more room. As before, dragging the edge all the way to the left tucks the sidebar away behind the little reopen tab.",
    ],
  },
  {
    version: "3.52.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Clicking a document on the memory map now opens it in the reading panel — the same one you get from the Documents tab — so you can read it right there instead of jumping to its whole project. The project name in that panel is now a link, so the project is still one click away. The map also redraws crisply now if you drag the window onto a screen with different scaling.",
    ],
  },
  {
    version: "3.51.0-alpha",
    date: "2026-07-24",
    highlights: [
      "A project's milestones now sort by deadline by default and open on the next one you haven't ticked off, with completed milestones tucked just above — scroll up to see what's done. A new \"Completed\" checkbox by the sort controls hides or shows the finished ones, and your sort choice is now remembered when you leave a project and come back. You can still switch to Manual to drag milestones into your own order.",
    ],
  },
  {
    version: "3.50.0-alpha",
    date: "2026-07-24",
    highlights: [
      "The Focus tab now uses the width of your screen. Your project list sits beside the daily briefing, quick actions and upcoming events, instead of stacked in one narrow column with empty space down both sides. On a narrow window it folds back to a single column. This split view is the new default — you can switch back to a stacked column from the toggle in the Focus header, or under Settings → General → Focus.",
    ],
  },
  {
    version: "3.49.3-alpha",
    date: "2026-07-24",
    highlights: [
      "The Pinboard now fits your app window instead of your whole screen. On Macs and Linux it could open larger than the window with scrollbars down the side and along the bottom, because it was sized to the full display. It's now sized to the window: a board imported from a wider screen tidies itself to fit when it opens, and it only grows downward (never sideways) when it holds more than fits — so it never spills past the edges. Moving notes into a folder shrinks it back. (Windows already looked right.)",
    ],
  },
  {
    version: "3.49.2-alpha",
    date: "2026-07-24",
    highlights: [
      "The Sort dropdown on the Focus tab now shows its text centered and in full on Linux. It was rendering too low, with the bottom of the letters clipped off; the control is now sized so the text sits properly on every system. (Windows and Mac already looked right.)",
    ],
  },
  {
    version: "3.49.1-alpha",
    date: "2026-07-24",
    highlights: [
      "The hour gridlines in the week and day calendar views now render cleanly on Retina Macs. Some of the faint grey hour lines could previously come out uneven or drop out entirely there; PM now draws each as a crisp single-pixel line so they all show up. (Windows and Linux were unaffected.)",
    ],
  },
  {
    version: "3.49.0-alpha",
    date: "2026-07-23",
    highlights: [
      "The Local AI speed estimates now match your actual graphics card. Before, PM assumed the same middle-of-the-road speed for every dedicated GPU, so a fast card and a modest one showed the same tokens-per-second guess. PM now looks up your card's real memory bandwidth — the thing that most governs how fast a local model replies — so the estimate reflects what you actually have (a high-end card reads its memory several times quicker than an entry-level one). Where one card name ships in two memory sizes that run at different speeds, PM uses your card's memory to tell them apart. If your exact card isn't recognised, PM says so and falls back to the old general estimate rather than guess. This only sharpens the speed number on the model cards — it never changes which models fit.",
    ],
  },
  {
    version: "3.48.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI is smarter about fitting bigger models on your machine. A model keeps a running memory of the conversation as it goes (its “KV cache”), and until now PM always sized that at full precision — so a model that was close to fitting had to either drop to a more compressed, lower-quality version or shrink how much text it can consider at once. PM now quietly compresses that running memory to a near-lossless half-size setting when — and only when — doing so lets the model keep its full context or a higher-quality version instead. Cards that use it show a small “q8_0 KV” note so you can see what changed, and anything that already fit is untouched. It helps most on computers with a graphics card, where it can now fit setups that used to spill over.",
    ],
  },
  {
    version: "3.47.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI now reads the real graphics memory on more computers. On Windows, PM couldn't tell how much memory was on a dedicated AMD or Intel graphics card bigger than 4 GB — Windows reports that figure in a field that maxes out at 4 GB — so those cards quietly fell back to sizing models on your system memory and never got the faster “on your GPU” setup. PM now reads the true amount straight from the graphics system, so an Intel Arc or a larger Radeon card gets the same two-way sizing an NVIDIA card already did. NVIDIA cards, smaller cards, integrated graphics and Macs are unaffected. (Reading a dedicated Intel card's memory on Linux is still to come — it needs a different route there.)",
    ],
  },
  {
    version: "3.46.1-alpha",
    date: "2026-07-23",
    highlights: [
      "The new two-way local-model sizing now treats integrated graphics honestly. On a computer whose graphics share system memory - Apple Silicon, or an AMD/Intel integrated GPU - PM no longer offers a lighter “faster on your GPU” setup that wouldn't actually be faster (there's no separate graphics memory to win from). Those machines simply see the single best setup, as they should. Computers with a real dedicated graphics card are unaffected.",
    ],
  },
  {
    version: "3.46.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI now sizes each model two ways on a computer with a graphics card: the highest-quality setup that fits your memory, and a faster, lighter one that fits inside your GPU — usually a smaller, more compressed version at a shorter context, but replies stream far quicker. The Workbench shows both side by side with the speed difference spelled out, so you can choose. Before, PM only ever suggested the highest-quality setup, even when a lighter one would have run many times faster on your card. If a model already fits your graphics card, or your Mac shares one pool of memory, you'll just see the single best setup — and with no dedicated graphics card, nothing changes. PM still never switches for you.",
    ],
  },
  {
    version: "3.45.1-alpha",
    date: "2026-07-23",
    highlights: [
      "Restoring a backup onto a new computer settles in properly now. When you bring a backup to a fresh machine and switch to it, PM moves the restored vault into place as your everyday vault — instead of leaving it parked in a side folder you had to shuffle files out of by hand. And if the backup was a passphrase-protected “shareable” vault, PM asks whether to make it private to this device (the usual choice — sharing is something you set up per computer, and none of that setup travels in a backup) or keep the passphrase, and it no longer carries the original computer's owner tag across. Restoring onto a computer that already has a vault, or restoring to recover the same machine, is unchanged.",
    ],
  },
  {
    version: "3.45.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Linux gets a Debian/Ubuntu installer. Alongside the AppImage and the Fedora .rpm, PM now ships a .deb — so Debian, Ubuntu, Mint and friends can install it with a single `sudo apt install ./pm_*.deb` and get PM in their app menu, no fuss. And if you installed PM from a package (rpm or deb), the update banner now points you to reinstall the new package instead of quietly trying — and failing — to update itself the way only the AppImage can. Not on Linux, or on the AppImage? Nothing changes.",
    ],
  },
  {
    version: "3.44.2-alpha",
    date: "2026-07-23",
    highlights: [
      "Turning a shared vault back into a private one now locks other accounts out of the folder more reliably. When you make a vault you'd shared on this PC private again, PM decrypts your notes back to plain files in place — so any Windows account you'd linked has to lose its folder access at that moment. PM now does that by reading the permissions actually on the folder and clearing every account but you, instead of leaning on a saved list of who to remove that could be out of date or tampered with. Your notes were always safe while shared; this tightens the hand-back to private. (macOS and Linux already worked this way, and nothing changes if you've never shared a vault.)",
    ],
  },
  {
    version: "3.44.1-alpha",
    release: true,
    date: "2026-07-23",
    highlights: [
      "PM 3.44.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "One optional thing after updating: if you chatted with PM before this update, open the Documents tab and click Rebuild once. PM no longer treats its own past answers as source material when it searches your notes, and that single Rebuild clears the older answers out of search so a stale reply can't quietly shape a new one. If you're new to PM, there's nothing you need to do.",
      "Run AI on your own machine — free, and private. A new Settings › Local AI tab scans your computer, recommends on-device models it can actually run (and roughly how fast), connects to a local model server like Ollama or LM Studio, and lets you send your chats — or just the behind-the-scenes work — to it, falling back to the cloud only when the local model isn't reachable. A model on your own machine means nothing leaves your device — and PM now shows you which model answered, and says so honestly when a reply came from the cloud instead. If you use Ollama, PM can download a recommended model straight into it.",
      "You can now start PM without an API key. On first run, choose a cloud provider, a model on your own device, or “set up AI later” — PM works either way, and tells you plainly when a feature needs an AI provider you haven't set up yet. Already using PM? Nothing changes.",
      "PM grounds its answers more carefully. It now measures how strong the best match to your question really is, and when nothing fits well it says so and answers from general knowledge rather than dressing up a weak guess as a fact from your notes. It also weighs the whole shortlist of passages (not just a rough top few) and reads each one's section heading, so the passage you actually meant is likelier to surface — and there's a “Save as note” under each answer to keep a good reply as a real, searchable note in your vault.",
      "The file-reader is now sealed off on every system. The helper that opens and converts your files runs with no way to reach the internet and can only see the file it's working on — not your vault, not the rest of your computer — first on Windows, and now on Linux and macOS too. So even a booby-trapped document can't use it to phone home or snoop around. It's an extra wall, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-3101) you can quote in a bug report.",
      "Settings got a big tidy. Changes now save the moment you make them — the Save button is gone — the tabs are grouped and icon-labelled down the side with their sub-sections listed to jump straight to, and a small “Reset” appears next to anything you've moved off its default (with a “Reset to defaults” on each tab). Your API keys, search language and time zone are deliberately left untouched.",
      "Steadier under the hood. A rebuild now resumes where it left off instead of starting over, and search keeps working the whole way through; your passphrase is taken exactly as you type it; photos saved into a vault survive a passphrase change and come out in a plain-files export; “Remove PM data” now clears every vault key PM had cached, not just the current one; one corrupt photo can no longer freeze the document engine; and a chat reply that fails partway through is reported as a failure instead of being saved as though it were the answer. Your calendars also keep their list in sync on every sync and decide whether something is “today” in your own timezone.",
    ],
  },
  {
    version: "3.44.0-alpha",
    date: "2026-07-23",
    highlights: [
      "You can now start PM without an API key. On first run, choose a cloud provider (OpenRouter), a model running on your own device, or “Set up AI later” — PM works either way, and it tells you plainly when a feature needs an AI provider you haven’t set up yet. Already using PM? Nothing changes.",
    ],
  },
  {
    version: "3.43.0-alpha",
    date: "2026-07-23",
    highlights: [
      "When you run a local model, PM now shows you which one answered — and tells you honestly when a reply came from the cloud instead. A small “Local” line in the sidebar and a pill next to the message box show whether your local model is connected, resting, or unreachable, and a gentle note appears above the chat whenever a reply had to fall back to the cloud (because the local model was busy, still loading, or couldn’t be reached). If you don’t use a local model, nothing changes.",
    ],
  },
  {
    version: "3.42.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Changed a setting and want it back the way it was? A small “Reset” now appears next to anything you’ve moved off its default, and each of the General, AI & Models, and Search tabs has a “Reset to defaults” that puts everything on it back at once — appearance, model choices, the memory map, re-ranking, and more. Your API keys, search language, and time zone are deliberately left untouched.",
    ],
  },
  {
    version: "3.41.1-alpha",
    date: "2026-07-23",
    highlights: [
      "Under-the-hood tidying of the Settings internals, with no change to how anything works: the backend code that stores your preferences moved into its own tidy home, and the on/off settings now share one small helper instead of each repeating the same steps. Purely groundwork — it makes the next batch of settings quicker and safer to add.",
    ],
  },
  {
    version: "3.41.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Run AI on your own machine — free, and private. The new Settings → Local AI tab scans your computer, recommends on-device models it can actually run (and roughly how fast), connects to a local model server like Ollama or LM Studio, and lets you send your chats — or just the behind-the-scenes work — to it, falling back to the cloud only if the local model isn't reachable. A model on your own machine means nothing leaves your device. If you use Ollama, PM can download a recommended model straight into it.",
    ],
  },
  {
    version: "3.40.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Settings is easier to get around: the tabs are now grouped and icon-labelled down the side, and the tab you're on lists its sub-sections right there — click one to jump straight to it, or just scroll and watch it keep pace. Everything's where it was, only quicker to find.",
    ],
  },
  {
    version: "3.39.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Settings now save the moment you change them — the Save button is gone, so there's nothing to remember to click. Your API key shows a green 'Saved' the instant it's stored. (Groundwork, too, for adding new settings cleanly — like the on-device AI setup coming soon.)",
    ],
  },
  {
    version: "3.38.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Under-the-hood groundwork for on-device AI: PM can now read your computer's memory, processor, and graphics card, and size a curated list of local models against it — working out the highest quality each one could run, and roughly how fast. Nothing to switch on yet; the setup screen that shows these recommendations comes next.",
    ],
  },
  {
    version: "3.37.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying, no visible change: when a live chat borrows your local model for a moment, the background jobs it interrupted (like chat summaries and titles) now wait a beat and retry on their own, instead of sitting out until much later. Also added internal notes so a fiddly database-upgrade footgun can't quietly bite us again.",
    ],
  },
  {
    version: "3.37.0-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood: the local-AI engine is now live inside PM. It can talk to a local model server, hand a request to the cloud honestly when the local one is slow or unreachable (and tell you when it did), and count local and cloud usage separately. There's still no switch to flip — the Local AI setup screen that turns it on comes next.",
    ],
  },
  {
    version: "3.36.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying, no visible change: every AI request now flows through a single internal routing point. It behaves exactly as before today, and it's the groundwork that lets a local, on-device model slot in cleanly next.",
    ],
  },
  {
    version: "3.36.0-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood groundwork for an upcoming option: running AI models locally, on your own machine, so your chats can stay entirely on your device. This release lays the plumbing only — there's nothing to switch on yet. The setup screens follow.",
    ],
  },
  {
    version: "3.35.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying. Your Usage & cost view now counts a slice of background AI work — chat summaries, titles, and a couple of others — that a stray database rule had been quietly dropping, so your totals read a little more complete. We also firmed up an internal code boundary and moved a shared building block to its proper home. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.35.0-alpha",
    date: "2026-07-20",
    highlights: [
      "The file-reader's protective sandbox now covers macOS too — completing the set alongside Windows and Linux. The helper that opens your files runs with no way to reach the internet (not even to look up a web address) and can only see the handful of folders it actually needs, never your vault or the rest of your home folder. As always it's an extra layer, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-3101) you can quote in a bug report. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.34.0-alpha",
    date: "2026-07-20",
    highlights: [
      "The file-reader's protective sandbox now covers Linux too: the helper that opens your files runs with no way to reach the internet and can only see the handful of folders it actually needs — not your vault, not the rest of your home directory — the same wall Windows already had. On older Linux kernels that lack the newer 'Landlock' feature, the network is still blocked and PM says so honestly rather than pretending. As always it's an extra layer, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-4105) you can quote in a bug report. Nothing changes in day-to-day use — macOS gets the same treatment next.",
    ],
  },
  {
    version: "3.33.1-alpha",
    date: "2026-07-20",
    highlights: [
      "If PM ever can't start the file-reader's sandbox, it now shows a short code (like SBX-2104) — and there's a plain-English list of what each code means (ERROR_CODES.md in the project, and Developer mode → Sidecar sandbox in the app). Popping that code into a bug report lets us pinpoint the problem fast. Nothing changes in normal use.",
    ],
  },
  {
    version: "3.33.0-alpha",
    date: "2026-07-20",
    highlights: [
      "Under-the-hood groundwork for bringing the file-reader's protective sandbox to Mac and Linux next: the shared machinery now lives in one place, and if the sandbox ever can't start it reports a precise code (like SBX-2104) instead of a vague message — so a one-line note is enough for us to pinpoint and fix it. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.32.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Developer mode now shows, at a glance, whether the file-reading helper's Windows sandbox is actually switched on — and, in development builds, a one-click check that confirms it really can't reach the internet. This is purely a window onto the protection added last update; if you don't use Developer mode, nothing here changes for you.",
    ],
  },
  {
    version: "3.31.0-alpha",
    date: "2026-07-19",
    highlights: [
      "On Windows, the helper that opens and reads your files now runs inside a locked-down sandbox: it has no way to reach the internet, and it can only see the one file it's working on — not your notes, not the rest of your computer. So even a booby-trapped document can't use PM's file reader to phone home or snoop around. Everything works exactly as before; this is simply an extra wall, quietly added around the riskiest part of PM. (The AI models still download normally through a separate, sandbox-free step, and macOS and Linux get the same treatment next.)",
    ],
  },
  {
    version: "3.30.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Under-the-hood tidying that also helps at uninstall time: PM now keeps its downloaded on-device AI models inside its own data folder, so removing PM cleans them up too — previously some were left behind in a temporary system folder. The first time you search or add something after this update, the main model may download once more into its new home; after that, everything runs on your device exactly as before.",
    ],
  },
  {
    version: "3.29.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Under-the-hood security groundwork: the helper that opens and converts your files is being prepared to run completely sealed off from the internet, so that even a malicious file could never use it to reach out. As a first step, downloading PM's on-device AI models is now handled by a separate, short-lived step — leaving the file reader with no reason to touch the network. Day to day nothing changes: the models still download once on first use, then everything runs on your device as before.",
    ],
  },
  {
    version: "3.28.0-alpha",
    date: "2026-07-19",
    highlights: [
      "On a Mac, sharing a vault with other accounts on the same computer now locks the folder down to just the people you choose — the same extra protection PM already gave shared vaults on Windows and Linux. Other accounts signed in to that Mac can no longer reach the raw files; your notes were always encrypted, and this simply adds a lock on the folder on top. If you never share a vault, nothing changes for you.",
    ],
  },
  {
    version: "3.27.1-alpha",
    date: "2026-07-19",
    highlights: [
      "Fixed a Windows-only hiccup where the occasional web page saved in your Google Drive or OneDrive could fail to import. PM briefly collided with antivirus over the temporary copy it was reading, so that one page was skipped. It now gives each file its own name and waits out that momentary lock, so these pages index reliably. Everything else about how your cloud files sync is unchanged.",
    ],
  },
  {
    version: "3.27.0-alpha",
    date: "2026-07-18",
    highlights: [
      "PM now chooses the passages behind an answer more carefully. Until now its smart re-ranker only re-ordered a short list of notes that a first, rougher pass had already picked — so a strongly relevant note that the rough pass ranked just out of reach could never make it in. The re-ranker now weighs the whole shortlist, so those near-misses can surface. You should notice fewer “but I know I wrote that” moments on questions your files genuinely cover.",
    ],
  },
  {
    version: "3.26.1-alpha",
    date: "2026-07-18",
    highlights: [
      "Routine housekeeping: refreshed the behind-the-scenes libraries and build tools PM is built on to their latest releases. Nothing changes in how PM looks or works — this just keeps the foundations current and secure.",
    ],
  },
  {
    version: "3.26.0-alpha",
    date: "2026-07-18",
    highlights: [
      "The safety check from the last update is now switched on. When you ask PM about something your files don't really cover, it now notices that the closest match is weak, says so plainly, and answers from general knowledge instead of dressing up a shaky guess as a fact from your notes. Ask about something your files do cover and nothing changes — it answers and cites its sources exactly as before. (For the curious: turn on Developer mode and each answer shows the confidence score behind it, with a dial to adjust or switch off the check.)",
    ],
  },
  {
    version: "3.25.0-alpha",
    date: "2026-07-18",
    highlights: [
      "Groundwork for a new safety check on how PM grounds its answers. When PM searches your files for an answer, it now measures how strong the best match actually is — so that, once tuned, it can tell the difference between “your notes really cover this” and “nothing here fits,” and stop confidently answering from a weak match. The check is switched off by default while it's being calibrated, so nothing changes in how you chat today.",
    ],
  },
  {
    version: "3.24.0-alpha",
    date: "2026-07-18",
    highlights: [
      "PM no longer treats its own past answers as source material when it's answering you. It still keeps every conversation in full — nothing is deleted, and you can reread any chat — but when it looks for relevant background it now draws only on your own notes, files and messages, not on things it said earlier. That closes a subtle loop where an older, imperfect answer could quietly shape a new one. And since a reply is often worth keeping, there's now a \"Save as note\" button under each answer: one click files it as a real, searchable note in your vault, tagged with a small reminder that it came from a chat. (If you chatted before this update, press Rebuild once in Documents to clear the old answers out of search.)",
    ],
  },
  {
    version: "3.23.1-alpha",
    date: "2026-07-18",
    highlights: [
      "Another improvement to how PM picks which of your notes to read when answering. To rank the passages it finds, PM takes a second, closer look at each one — but that closer read was only seeing the passage text, not the section heading above it. So a section whose heading named exactly what you asked about could be passed over when its text happened to open with routine boilerplate. PM now shows that closer read the heading too, so the right section is more likely to rise to the top. Nothing looks different in everyday use.",
    ],
  },
  {
    version: "3.23.0-alpha",
    date: "2026-07-17",
    highlights: [
      'For anyone who likes to look under the hood: turn on Developer mode and each chat answer now carries a collapsed "Prompt sent to the API" panel — open it to see exactly what PM sent the model for that reply, its own instructions and the notes, calendar and reminders it drew on. The in-chat retrieval "Explain" tool moved into Developer mode alongside it. Leave Developer mode off and nothing about chat changes.',
    ],
  },
  {
    version: "3.22.5-alpha",
    date: "2026-07-17",
    highlights: [
      "Fixed pinboard notes running your lines together. When you write a note across several lines and then click away to read it back, PM now keeps the line breaks exactly where you put them instead of merging everything into one paragraph.",
    ],
  },
  {
    version: "3.22.4-alpha",
    date: "2026-07-17",
    highlights: [
      "A behind-the-scenes tweak to how PM chooses which of your notes to read when answering. When one long document — or a run of very similar passages — dominates the results, PM now makes room for other relevant sources instead of letting a single section crowd everything else out, so answers can draw on a wider slice of what you've saved. Nothing looks different in everyday use.",
    ],
  },
  {
    version: "3.22.3-alpha",
    date: "2026-07-17",
    highlights: [
      "More under-the-hood hardening, this time around chat. When you ask PM something, it draws on your relevant notes, calendar and reminders as background material — which PM now keeps cleanly separated from its own instructions, so text living inside your own documents can never be read as a command to the assistant. It's a safety boundary behind the scenes and doesn't change how you chat.",
    ],
  },
  {
    version: "3.22.2-alpha",
    date: "2026-07-17",
    highlights: [
      "Under-the-hood hardening of how PM handles file locations. When you import files, add a folder, save a backup or export, or move your vault, PM now double-checks the location is a real, sensible path before it touches your disk — an extra safety net you won't notice in everyday use. Choosing where to put a plaintext export, and pointing PM at the Proton Drive program, now open their picker from PM itself for the same reason. Nothing changes in how you use any of these.",
    ],
  },
  {
    version: "3.22.1-alpha",
    date: "2026-07-16",
    highlights: [
      "Renaming a project no longer empties its pinboard timeline. A timeline widget pinned to a project tracked it by name, so renaming (or merging) the project moved its milestones and left the widget pointing at a name that no longer existed — and it silently went blank. Renaming now carries those widgets across too, without touching anything else on your board.",
    ],
  },
  {
    version: "3.22.0-alpha",
    date: "2026-07-16",
    highlights: [
      "In a shared vault, connectors now clearly belong to the vault's owner. When you share a vault with another Windows account on your PC, syncing a connector like Google Drive or OneDrive relies on sign-in details that live in one account's private keychain — so a second account could never actually sync them, and trying just produced a confusing 'connection failed'. Now PM says so plainly: the vault's owner sets up the connectors, and everyone who shares the vault still sees everything that gets indexed. (Existing shared vaults are unaffected — this applies to vaults shared from this version onward.)",
    ],
  },
  {
    version: "3.21.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Rebuilding your index no longer breaks links to your chats. When PM rebuilds its search index it re-processes every chat — and it used to give each rebuilt chat a brand-new internal identity, quietly breaking saved references to it (the 'jump to this chat' links, and the corrections PM had learned from it). Rebuilt chats now keep their identity, so those links survive — the same fix documents got in an earlier release.",
      "Cleaned up rare duplicate entries left over from indexing one shared Google Drive on two accounts. If you'd indexed the same shared drive from two connected accounts before PM learned to de-duplicate them, a leftover duplicate could linger. PM now merges away any duplicate whose filing matches — and where two copies were filed differently, it leaves both rather than guess which to keep and risk discarding your filing.",
    ],
  },
  {
    version: "3.20.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Your calendar list now keeps itself in sync. Before, PM only learned which calendars an account had at the moment you connected it — so a calendar you created later never appeared, and one you deleted upstream kept failing every sync and pinning the whole account as 'unreachable'. Now each sync rechecks the list: new calendars show up on their own (untick any you don't want), and deleted ones are cleaned up — but only when PM has provably seen your whole list, so a dropped connection can never remove a calendar by mistake.",
    ],
  },
  {
    version: "3.19.10-alpha",
    date: "2026-07-16",
    highlights: [
      "PM now describes its cloud indexing accurately. The Google Drive and OneDrive help said PM keeps 'a searchable pointer and a short summary' — which undersold it: PM actually reads each file's full contents to build its search index, it just never keeps a copy of the file. The wording now matches what search can really do.",
      "The scheduled-backup help now describes what actually happens: a backup runs when PM is unlocked and idle with a passphrase set and nothing else syncing, and if a destination can't be reached right then it's simply retried next time — rather than promising an 'online' check only some destinations make.",
    ],
  },
  {
    version: "3.19.9-alpha",
    date: "2026-07-16",
    highlights: [
      "Help mode now answers everywhere it lights up. Four places highlighted as if they had an explanation and then showed nothing when you hovered them — including Remove PM data, the one place in Settings where knowing exactly what a button does matters most. They all have their explanation now, and the model picker's no longer claims you can search every model on OpenRouter (PM only lists the ones it can actually use).",
      "Times on the Focus tab now match the rest of PM instead of quietly using their own format.",
      "Under-the-hood tidying: our own description of what PM sends over the network now mentions syncing the cloud accounts you connect — it listed everything else and somehow not that.",
    ],
  },
  {
    version: "3.19.8-alpha",
    date: "2026-07-16",
    highlights: [
      "One bad photo can no longer freeze everything PM does with your files. A photo whose GPS data is corrupt produced a number that can't be written down — and because PM's document engine handles one job at a time, that single photo jammed the queue for half an hour: no importing, no search, no voice notes, no memory map, with nothing on screen to say why. Corrupt location data is now simply treated as no location, and any impossible number fails just that one job instead of the whole engine.",
      "Your OCR and memory-map add-ons now survive a repair. If PM had to rebuild its document engine — after a torn install, or when it found a newer Python — it deleted the add-ons you'd installed without saying a word: photos silently stopped having their text read, and the memory map quietly dropped to a simpler layout. They're now reinstalled automatically, and if that can't be done you're told rather than left guessing.",
      "Very large text files are now turned away up front instead of after several minutes. A big .txt, .md or .html file was accepted, converted, and only then rejected for being too big to hand back — so you waited, and got an error the whole time was spent earning. The limit for those files now reflects what PM can actually return.",
      "After an update, PM no longer claims its document engine is ready before checking. It could report Ready while still set up for the previous version's requirements, so the first thing you did in that window ran against the wrong setup.",
    ],
  },
  {
    version: "3.19.7-alpha",
    date: "2026-07-16",
    highlights: [
      "A chat reply that fails partway through is no longer saved as if it were the answer. If the model or its provider failed after it had started replying, PM kept the half-finished text, saved it into the conversation and your vault, indexed it, and said nothing — so a broken sentence became something PM could quote back to you later as though it were real. A failure is now reported as a failure. And when a reply is cut short simply because it got long, it's saved with a note saying so rather than pretending it just ended there.",
      "Renaming a chat before you've sent anything now sticks. PM names conversations for you in the background, but a title you chose yourself is meant to win. Renaming a brand-new chat and then sending your first message let the automatic namer overwrite what you'd picked — which is exactly the moment people rename a chat.",
      "Background work now bills the background key, as intended. If you've set a separate key for background jobs, the rolling summaries, chat titles and preference learning were all still charged to your main key — so the split you set up didn't reflect where the spend went, and a background-key-only setup refused to run them at all. Nothing changes if you use a single key.",
      "Setting an empty Microsoft client ID is now refused instead of accepted and then failing later with an unhelpful error.",
    ],
  },
  {
    version: "3.19.6-alpha",
    date: "2026-07-16",
    highlights: [
      "Reminders about your day now use your day. If you're not on UTC, PM worked out whether an event was \"happening today\" or still ahead of you by reading the date off however your calendar provider happened to store it — not by asking what day it is where you are. An evening event could be treated as tomorrow's, so the nudge arrived the morning after it happened. Your timezone now decides, the same way the rest of PM already did.",
      'Subscribed calendar feeds that use lowercase now work. The iCalendar standard says property names can be written in any case, but PM only recognised capitals — so a feed writing "dtstart" instead of "DTSTART" quietly appeared completely empty, and one writing "rrule" lost every repeat of a recurring event. Both now read correctly.',
      'Events from Google and Outlook now sort together properly. Google reports times in the calendar\'s own timezone while Outlook and subscribed feeds report them in UTC, and PM compared them as plain text — so an event could appear in the wrong place in an agenda that mixed the two, and "the next one" could pick the wrong event. All times are now stored the same way. Your calendars will resync once, on their own.',
      "Deleting a milestone now clears its reminders. The milestone went, but any flag it had raised stayed behind — including ones you'd already dismissed, which nothing ever removed.",
    ],
  },
  {
    version: "3.19.5-alpha",
    date: "2026-07-16",
    highlights: [
      "Files from your connected accounts are no longer buried in search results. PM doesn't keep the contents of Drive or OneDrive files on your machine, so it stored a short summary and left a placeholder — \"(body available at the source)\" — where the text would go. Several parts of PM read that placeholder as if it were the file: the step that ranks results by relevance judged every connected file on that one sentence and pushed them all to the bottom, chat answers cited files it couldn't actually read, and the assistant that suggests a project's details saw the same line for every file. They all now read the summary PM does have.",
      "Renaming a note now renames it everywhere. Changing a note's title on the Pinboard and saving it left the old title on the document itself — in search, in citations, and in the file in your vault — because PM decided nothing had changed by looking only at the body. The title counts as a change now, and the note is re-indexed under its new name.",
      "A file that fails to import no longer comes back on its own. When importing a document, photo or spreadsheet failed at the last step, PM had already written the file into your vault and left it there — so the next time you rebuilt the index, the failed import reappeared as a document, unsorted and with no sign anything had gone wrong. The leftover file is now cleaned up.",
      "Under-the-hood tidying: projects created from your connected files now stick properly instead of being forgotten and recreated on every launch, and a connected file that can't be restored during a rebuild is named on screen instead of leaving the progress bar stuck just short of the end.",
    ],
  },
  {
    version: "3.19.4-alpha",
    date: "2026-07-16",
    highlights: [
      "Removing PM's data now clears every vault key it kept, not just the current one. \"Remove PM data\" promises to erase every secret PM has stored on your machine. If you had joined someone else's shared vault and later left it, PM deliberately keeps its key so rejoining is silent — but the erase only ever cleared the vault you were using at the time, so that key stayed on the machine afterwards, for a vault that may still be sitting in a shared folder. Every key PM has cached is now erased, including from vaults you left before this update.",
      "Moving your vault now takes all of it. Two encrypted files — your entity rules and the list of files indexed from cloud accounts — were left behind in the old folder every time the vault moved. Backups always included them, so nothing was ever lost, but making a vault private left readable copies in the folder you were leaving. They now travel with the vault, and deleting a shared vault properly empties the folder instead of leaving those files in it.",
      "Unlocking your vault no longer fails because Windows couldn't remember the key. If PM couldn't save your key for next time — a credential store that's disabled or damaged — it treated that as a failed unlock and refused to open a vault your passphrase had already opened correctly, with no way forward. It now opens as normal and simply mentions you'll be asked for the passphrase again next launch.",
    ],
  },
  {
    version: "3.19.3-alpha",
    date: "2026-07-16",
    highlights: [
      "Your pinboard can no longer be replaced by an empty one. If PM couldn't read your board when you opened the Pinboard — a moment's trouble reading from your data folder was enough — it showed an empty board and then treated that as yours: simply switching to another tab saved the blank one over the real one. PM now says it couldn't open your board, leaves what's saved untouched until it can read it properly, and gives you a Retry.",
      "The Pinboard now tells you when it can't save. It promises your board is \"saved on this device\", but if a save actually failed it said nothing at all and carried on — so you could keep arranging a board that wasn't being kept. It now says so plainly, and your next change tries again by itself.",
    ],
  },
  {
    version: "3.19.2-alpha",
    date: "2026-07-16",
    highlights: [
      "Photos you save into an encrypted vault now survive a passphrase change. When you tick \"save a copy to the vault\" for an image, PM keeps that copy encrypted alongside your notes. Changing your vault's passphrase re-locked the notes but quietly left those images locked to the old one — so the copy you kept, often after deleting the original, could no longer be opened. Photos are now re-locked along with everything else, and making a vault private unlocks them too. If you changed a passphrase before this update, those images can only be recovered from a backup made beforehand — PM will now fall back to showing the original file or the photo's text rather than failing outright.",
      'Saving a copy of an image you\'d already added now sticks. Re-adding a photo with "save a copy to the vault" ticked said it had saved one, but only half the record was written — so the next time you rebuilt the search index, PM forgot the copy existed and left it stranded in your vault. The copy is now recorded properly, and a rebuild repairs any that were stranded this way.',
      "Exporting your vault as plain files now includes your saved photos. The export is there so you're never locked in, but it only ever wrote out your notes — the saved images stayed behind, encrypted. They now come out too, under their real filenames.",
    ],
  },
  {
    version: "3.19.1-alpha",
    date: "2026-07-16",
    highlights: [
      "Fixed a passphrase trap in shared vaults. If you set up or changed a shared vault's passphrase with a space at the start or end, PM quietly dropped those spaces when it locked the vault — but not when it unlocked it — so the passphrase you'd chosen wouldn't open your own vault. Your passphrase is now taken exactly as you type it, spaces and all. Vaults set up before this update still open with the spaces left off: PM now says so if what you're typing starts or ends with a space, and changing the passphrase re-keys the vault to exactly what you type.",
      "A passphrase can no longer start or end with a space. It's almost always a stray one from a copy-and-paste, you can't see it in a password box, and you'd have to type it exactly right forever after — so PM now says so while you're choosing, rather than letting you find out the hard way. Spaces inside are still very much welcome: a passphrase of a few plain words is still the best kind.",
    ],
  },
  {
    version: "3.19.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Rebuilding the search index no longer starts over if it's interrupted. Close PM (or lose power) at 95% through a rebuild and it used to throw all of that away and begin again from nothing — the slowest thing PM does, done twice. Now it picks up exactly where it left off, including the connected files it had already re-read from Google Drive or OneDrive, so nothing gets downloaded or re-read twice.",
      "Search keeps working while a rebuild runs. A rebuild used to clear the whole index first and refill it, so for as long as it took — sometimes hours on a big library — searching and chat found less and less of your library. Now each document is rebuilt in place, so your search stays intact the whole way through. (Switching search language is the one exception: that one still has to clear the index first, because the whole shape of it changes.)",
      "Rebuilding no longer forgets what you'd taught PM. Every rebuild used to quietly cut the link between your filing corrections and the documents they were about, which is the record PM learns your habits from. Those links now survive. Old links to documents in past chats survive too, so citations in your chat history keep working after a rebuild.",
      "PM now stands its ground while it's rebuilding. Adding documents, syncing a connected account, or changing your vault while a rebuild was running could quietly collide with it. Those now wait — and tell you why — instead of racing it.",
    ],
  },
  {
    version: "3.18.2-alpha",
    date: "2026-07-15",
    highlights: [
      "Under-the-hood tidying: PM's own documentation caught up with the last two updates. Nothing in the app changed — the README was still telling you that you could pick any model through OpenRouter, which stopped being true when the model list started filtering out ones that can't answer privately.",
    ],
  },
  {
    version: "3.18.1-alpha",
    date: "2026-07-15",
    highlights: [
      "Rebuilding your documents no longer loses its place when you switch tabs. It never actually stopped — the work carried on the whole time — but the Documents tab forgot it was happening the moment you looked away, came back showing nothing, and left you assuming it had died. Now the progress is yours to leave: switch tabs, go and do something else, and the bar picks up exactly where it is when you return.",
      "It also survives closing the app. If PM is shut mid-rebuild, it starts the rebuild again by itself next time you open it. Being straight with you: a rebuild can't run while the app is closed, and it has no way to pick up mid-file, so this restarts it rather than resuming it. It's still the right thing to do — a rebuild that's been interrupted leaves your search incomplete until it finishes, and previously nothing told you that or fixed it.",
      "Fixed a real one: rebuilding twice at once could destroy the first rebuild's work. Switching away and back reset the button, so a second Rebuild could start while the first was still going — and the second one clears the index out before it begins. PM now refuses the second and tells you the first is still running.",
    ],
  },
  {
    version: "3.18.0-alpha",
    date: "2026-07-15",
    highlights: [
      "The model list in Settings › AI now only offers models PM can actually use. PM sends every request with zero-data-retention enforced, so a provider can't store or train on your prompts — and a model with no provider willing to work that way doesn't quietly become less private, it simply doesn't answer. Those models are now filtered out of the picker rather than left there to fail, which takes the list from around 340 models to around 220. Every one that remains is one your prompts are safe with.",
      "Recommended models is gone. It suggested two models and, in fairness, sometimes suggested one that couldn't answer at all — it ranked on price and capability, and had no way to know whether a provider would agree to zero-data-retention. Rather than teach it, PM now applies that knowledge to the whole list above, where it helps whichever model you pick. The default, Ling-2.6-flash, is doing the job well on its own. Your saved models are untouched.",
      "If you'd set up recommendation exclusions, that list has gone with it. It only ever hid models from those two suggestions and never blocked anything you chose yourself, so nothing you rely on changes.",
      "You can still type a model id by hand, as always — PM isn't locked to the list.",
    ],
  },
  {
    version: "3.17.1-alpha",
    release: true,
    date: "2026-07-15",
    highlights: [
      "PM 3.17.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "One thing to do after updating: open the Documents tab and click Rebuild, once. Files you'd connected from Google Drive, OneDrive or a watched folder were only searchable by a short summary of themselves, so search, chat and the filing suggestions in Review could all miss what was deeper inside them. They're now read and indexed in full — and that one Rebuild is what brings the files you already have up to date. Anything offline at the time stays findable by its summary and catches up on its next sync.",
      "The pinboard is now something you can properly work on. Folders are made on purpose with a + Folder button, they're resizable, and they stay until you ungroup them rather than evaporating when they're down to one card. A card only files into a folder if your mouse is over it when you let go — so a big note can finally sit next to a folder without being swallowed — and folders no longer swallow each other. Opening one as an overlay gives you a real board to drag, resize and overlap on, not a list.",
      "The pinboard has undo, and asks before it deletes. Ctrl+Z (⌘Z on a Mac) takes back your last change, Ctrl+Y puts it back, and deleting a note or timeline now tells you what actually goes with it first — with a “Don't ask again” tick, and a switch in Settings › General to bring the asking back. Two things undo deliberately won't take back, because it can't do so honestly: ingesting a note, and linking a timeline to a project.",
      "Your milestones and timelines now show on the calendar. Any dated milestone appears as an all-day marker in its own colour across Month, Week, Day and Agenda — click it to jump to its project — and every dated entry on a pinboard timeline does the same. You can hide either from the Calendars menu. The Milestones panel also sorts now: by deadline, by name, or by hand, from a Manual / Deadline / Name control in its header.",
      "The calendar scrolls, and its timezones moved somewhere you'll find them. Month and Year views now scroll smoothly through past and future instead of jumping a whole month or year, settling neatly on a week or a row of months. Extra timezones are added from the top-left corner of the Day or Week grid, where the hour column meets the dates, and the list reads as Continent / Country / City so you can search by any of them.",
      "PM now starts you on a far cheaper model — Ling-2.6-flash instead of Claude Sonnet 4.6, a few hundred times less per word — which matters most for the work PM does when you're not watching it. Sonnet is still there in Settings › AI. The honest trade: it reads a little less at once, and it's served by a single provider, so if that provider has a bad day chat waits rather than quietly moving elsewhere; adding a second model and turning on auto-switch gives you that safety net back. If you've already picked your own models, nothing changes.",
      "Settings is much quieter. Every section leads with what you came to change and folds its explanation behind a caret underneath — for everyone, at every density. Nothing was deleted, and warnings you'd regret missing stay out in the open.",
    ],
  },
  {
    version: "3.17.0-alpha",
    date: "2026-07-15",
    highlights: [
      "PM now starts you on a far cheaper model. Out of the box, chat and background work both use Ling-2.6-flash instead of Claude Sonnet 4.6 — a few hundred times less per word, which matters most for the work PM does when you're not watching it: naming your chats, summarising them, and proposing where each new document belongs. Sonnet is still there in Settings › AI whenever you want it. If you've already picked your own models, nothing changes — this only sets the starting point for anyone who hasn't.",
      "The honest trade: the new default reads a little less at once than Sonnet did (262,000 tokens against a million — still far more than a normal chat needs), and it's served by a single provider, so if that provider has a bad day chat waits rather than quietly moving elsewhere. Adding a second model in Settings › AI and turning on auto-switch gives you that safety net back.",
      "Settings is much quieter. Every section now leads with what you came to change — the switches, the pickers, the connect buttons — and folds its explanation away behind a caret underneath. Nothing was deleted; it's one click away, and Help mode still explains anything you hover. What deliberately stays out in the open: warnings you'd regret missing (there's no recovering a backup without its passphrase), and the notes telling you why a button is greyed out.",
      "Those explanations used to unfold themselves for the Power density, on the theory that a power user wants more detail. That had it backwards — the person most likely to already know how backups work was the one being handed the essay. They now start folded for everyone, at every density. Your connected accounts — Google, Microsoft, Apple — are unaffected and still sit open where they always did.",
    ],
  },
  {
    version: "3.16.0-alpha",
    date: "2026-07-15",
    highlights: [
      "The pinboard now has undo. Ctrl+Z (⌘Z on a Mac) takes back what you last did — deleting a card, changing a note's colour, moving something, or the last few seconds of typing — and Ctrl+Y, or Ctrl/⌘+Shift+Z, puts it back. Typing is grouped into short bursts, so one undo takes back a few seconds' worth rather than a single letter, and it leaves your cursor where the change was. The trail lasts for as long as you have PM open; it isn't saved between visits.",
      "Deleting a note or timeline now asks first. The pop-up tells you what actually goes with it — dated entries that would leave your calendar, or a document you'd already saved to your vault that would stay behind. There's a “Don't ask again” tick if you'd rather it didn't, and a switch in Settings › General to bring it back. A folder's ✕ doesn't ask, because it only ungroups the folder and spills the cards back onto the board.",
      "Two things undo deliberately won't take back, because it can't do so honestly: ingesting a note (the document is its own copy in your vault, and stays there), and linking a timeline to a project (its entries become that project's real milestones). Linking is the point where the undo trail starts fresh.",
    ],
  },
  {
    version: "3.15.0-alpha",
    date: "2026-07-15",
    highlights: [
      "Opening a folder as an overlay now gives you a pinboard, not a list. It fills 80% of the board, and the notes and timelines inside keep the shape and size you gave them — you can drag them around, resize them and overlap them in there exactly as you do outside. Cards are laid out clear of each other when they go in, so nothing hides behind anything else. Opening a folder “in place” is unchanged, if you prefer the compact card view.",
      "The board is now a fixed size — your screen — instead of quietly resizing with the window. It used to be as wide as the window and grew downward to wherever your lowest note sat, so the same board changed shape depending on how you'd sized PM. Now it's one canvas that simply scrolls if the window is smaller than it, and a board you made on a bigger monitor still tidies itself in to fit.",
      "Fixed: help mode couldn't be read inside any pop-up window — its labels were painted behind them. They now sit on top.",
      "Fixed: What's New asked to be 80% of the window's height and was silently getting 85%.",
      "Under-the-hood tidying: the board's drag-and-drop, its grid, and its cards are now one shared implementation used by both the main board and the one inside a folder, so the two can't drift apart. Dragging with a folder open is also markedly less work for your machine.",
    ],
  },
  {
    version: "3.14.0-alpha",
    date: "2026-07-15",
    highlights: [
      "Pinboard folders are now something you make on purpose. There's a + Folder button beside + Note and + Timeline that drops an empty folder on the board, ready to tidy things into. Stacking one card exactly on top of another still folds the two together, as before.",
      "Filing a card into a folder now follows your mouse, not the card. Until now, dragging a note so that any part of it merely touched a folder was enough for the folder to swallow it — which made big notes almost impossible to park near a folder. Now a card only goes in if your pointer is over the folder when you let go, and the folder lights up to show it's about to catch it. Anywhere else, the card just lands where you put it and overlaps, the way notes and timelines already do.",
      "Folders no longer swallow each other. Dragging a folder onto another one used to tip its notes into it and destroy the folder you were holding. Now they simply stack, each keeping its own cards — and a folder dropped squarely on another shuffles over a cell so you can still get at the one underneath.",
      "A folder now stays until you ungroup it. It used to vanish the moment it was down to one card, popping that card back onto the board. That made a folder feel like a temporary side-effect rather than a thing you own — and an empty one you'd just made couldn't survive at all. Its ✕ still ungroups it and spills the cards back out, which remains the way to get rid of one.",
      "Fixed: opening a folder in place could squeeze a note's card so narrowly that its delete button was pushed out of sight and its title had no room left. The panel now lays its cards out in two columns whatever the size of your window (it was quietly using three on a wide screen), and each card's title is guaranteed a bit of space of its own.",
    ],
  },
  {
    version: "3.13.0-alpha",
    date: "2026-07-15",
    highlights: [
      "You can now sort a project's milestones. The Milestones panel has a small Manual / Deadline / Name control in its header: sort by deadline (click again for latest-first) or by name, or keep arranging them by hand with the up/down arrows as before. Undated milestones settle at the bottom.",
      "Your milestones now show on the calendar. Any dated milestone that isn't already on one of your calendars appears as an all-day marker in its own colour across Month, Week, Day and Agenda (and the Terminal look) — one central place to see everything. Click one to jump straight to its project. Completed ones show a ✓, and you can hide them from the Calendars menu. (A milestone you'd tied to a real calendar event keeps showing as that event, in its calendar's colour.)",
      "Your pinboard timelines show on the calendar too. Every dated entry on a freeform timeline appears as an all-day marker in its own colour — click one to jump back to the Pinboard. Each freeform timeline has a “Show on calendar” tick if you'd rather keep one off, and you can hide the lot from the Calendars menu. Link a timeline to a project and its entries become that project's milestones, shown in the milestone colour instead.",
      "The little picker for tying a milestone to a calendar event has gone. Now that milestones show on the calendar by themselves, it was more fiddle than help — and each milestone row has more room for its name, with the up/down arrows moved down beside the date. Milestones you had already linked keep their synced date and can still be unlinked.",
      "Linking a pinboard timeline to a project now keeps the entries you typed: they're added to that project's milestones instead of disappearing behind the linked view. Unlinking still leaves them safely in the project. The timeline's + Milestone button now shares a row with the project box, so the card wastes less space.",
      "Ingesting a note now keeps the title you gave it — that title becomes the document's name in Review, with the note's text as its contents. Untitled notes still take their first line, as before.",
      "Fixed: in Week and Day view, the dates along the top could sit slightly out of step with the columns beneath them whenever the grid showed a scrollbar. The header now reserves exactly the same space, so they line up.",
      "Fixed: if your OpenRouter key was ever saved blank, PM would try to use it anyway and the AI would fail with a confusing 401 “Missing Authentication header”. A blank key now counts as no key at all — you'll get the plain “No OpenRouter API key set” prompt instead, and a blank background key quietly falls back to your main one.",
    ],
  },
  {
    version: "3.12.1-alpha",
    date: "2026-07-14",
    highlights: [
      "Files you connect from Google Drive, OneDrive, or a watched folder are now searched on their full contents. Until now, after PM rebuilt its search index these files were only findable by a short (~500-character) summary — so search and chat could miss things deeper in a document, and the filing suggestions in Review only saw the title. They're now indexed in full.",
      "Because of that fix: after updating, open the Documents tab and click Rebuild once. It now also re-reads your connected files from their source, one at a time, so a single Rebuild brings everything fully up to date — you'll see each file processed. Anything temporarily offline stays findable by its summary and is caught up automatically the next time it syncs.",
      "HTML files from Google Drive are now read as clean text — the page's code (scripts, styles, and head) is stripped before indexing, exactly like files in a watched folder — so only the real content is indexed and summarised.",
    ],
  },
  {
    version: "3.12.0-alpha",
    date: "2026-07-14",
    highlights: [
      "The calendar's Month and Year views now scroll. Drag up and down to move smoothly through past and future — no more jumping a whole month or year at a time.",
      "Month view flows as one continuous run of weeks: let go and it settles neatly on a week, never mid-week. Each new month names itself inline and alternates a faint shade so you can see where one ends and the next begins; the header keeps up with whatever's on screen.",
      "Year view scrolls the same way through its little months, settling on a row of months rather than snapping a whole year. Today, the arrows, and the mini-calendar all glide you to the right spot.",
    ],
  },
  {
    version: "3.11.0-alpha",
    date: "2026-07-14",
    highlights: [
      "Adding an extra timezone to the calendar is easier to find. The separate “Zones” button is gone — instead, look to the top-left corner of the Day or Week grid, where the hour column meets the dates: a small “＋ Add” lives there. Add a zone and it shrinks to a “＋” to save space, and you can remove any extra timezone by hovering its column heading and clicking the ✕ that appears.",
      "The timezone list is far easier to search. Every zone now reads as Continent / Country / City with its code or UTC offset alongside — so you can find one by continent, country, city, or code. Can't find your exact city? Search your country and pick the nearest one listed.",
    ],
  },
  {
    version: "3.10.0-alpha",
    date: "2026-07-14",
    highlights: [
      "More pinboard polish, all from living with it. Folders are now resizable like notes and timelines — drag a folder's corner to make it as big or small as you like.",
      "The pinboard is now a fixed-width canvas the size of your window: adding notes fills left-to-right and wraps to a new row instead of stretching the board off the edge and taking the buttons with it. The board only ever grows downward (and scrolls), and a note you add scrolls into view. Notes from before that had drifted off to the right tidy themselves back on-screen.",
      "Note titles can be much longer now, stretching nearly to the Ingest button. The colour dots stay out of the way until you're actually editing a note — and they name themselves by colour on hover: Sage, Coral, Amber, Stone, Teal.",
      "Notes keep their place when you switch to another app and back — a note you're writing stays open for editing until you click somewhere else on the board, rather than snapping shut the moment PM loses focus.",
      "Timelines can switch between a stacked list and a horizontal track from a little toggle in their top bar, and the date column is a touch tidier.",
      "Checklists start flush against the left edge instead of looking pre-indented. Press Tab to nest an item under the one above (new checkboxes keep that level automatically), and Shift+Tab or Backspace to pull it back out.",
    ],
  },
  {
    version: "3.9.1-alpha",
    release: true,
    date: "2026-07-13",
    highlights: [
      "PM 3.9.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "See other timezones on the calendar, and set your own hours. In Day and Week view you can add up to two more timezones down the left beside your local time, frame the view to your own Work or Day hours from the ▾ on those buttons, and events now show their start and end time (e.g. 09:30–10:45) instead of just the start.",
      "A tidier pinboard. Notes and timelines can have a title, the Ingest button moved up into the top bar to give notes their full height back, and dropping a note exactly on top of another the same size folds them into a neat folder tile you can name, open, and drag cards back out of. Every colour also names itself on hover now.",
      "Your vault recovers honestly if Windows ever blocks its folder. Instead of a confusing wall of “the vault is locked” messages, PM now says exactly what's wrong and offers a one-click “Repair access”. Sharing a vault across Windows accounts is safe by construction — PM checks the destination, locks it down and confirms it still works before committing — and you can properly delete a shared vault, with everyone who joined told plainly and moved back to their own. Your data was never at risk; now PM says so.",
      "Windows updates no longer fail silently. If Smart App Control is blocking PM's update installer, PM now spots it and tells you the one thing that fixes it, instead of quietly reopening on the old version every launch.",
      "A cleaner exit. Finishing “Remove PM data” now actually closes the app and a full uninstall leaves no leftover “PM” folder behind, plus a clear reminder before you erase a vault and database that can't be recovered.",
    ],
  },
  {
    version: "3.9.0-alpha",
    date: "2026-07-13",
    highlights: [
      "See other timezones on the calendar. In Day and Week view, add up to two more timezones from the “Zones” button and they show as extra columns down the left beside your local time — handy for a call with another city. Remove them the same way.",
      "Set your own Work and Day hours. The ▾ on the Work and Day buttons (Day/Week view) opens a little editor for the start and end of each. Work now frames 08:30–17:30 by default, so a 9-to-5 day's first and last events sit comfortably inside the view; Day frames your local sunrise-to-sunset, rounded to the hour so it only shifts with the seasons. Change either to whatever suits you, or reset to the default.",
      "Calendar events now show their start and end time (e.g. 09:30–10:45), not just the start.",
      "Under the hood: your timezone and location now come from one shared place, so the calendar, the sunrise/sunset day framing, and the automatic light/dark switch all stay in step.",
    ],
  },
  {
    version: "3.8.0-alpha",
    date: "2026-07-13",
    highlights: [
      "Pinboard notes and timelines can now have a title — click the label at the top of a card (it starts as “Note” or “Timeline”) and type your own. The Ingest button moved up into that top bar too, so notes get their full height back for what you're actually writing.",
      "Stack it to file it: drop a note exactly on top of another the same size and the two fold into a tidy folder tile that shows how many cards are inside. Open it to read and edit them in place (or as a pop-out overlay), give the folder its own name, drag a card back out, and the folder quietly dissolves once it's down to one. Timelines can go in folders too, and a folder's ✕ just spills its cards back onto the board — it never deletes them.",
      "Every colour now tells you its name on hover — the note tint dots on the pinboard and every theme swatch in Settings, not just the monochrome one.",
      "While you're editing a note, hovering the formatting buttons (bold, lists, checklist…) reliably shows what each one does and its keyboard shortcut again.",
    ],
  },
  {
    version: "3.7.2-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now delete a shared vault properly. From Settings → Vault, “Delete shared vault…” removes it from the shared folder for everyone who uses it — with a clear warning first — and switches your account back to a vault of its own. The accounts you shared with are told, plainly, that the vault was deleted the next time they open PM, and are moved back to their own vault instead of hitting a confusing error.",
      "“Remove PM data” from an account that joined someone else's shared vault now only clears your own copy of things and leaves the shared vault untouched for everyone else — it can no longer wipe a shared vault out from under the other accounts.",
    ],
  },
  {
    version: "3.7.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Sharing a vault is now safe by construction: PM checks the destination folder can actually hold a shared vault before it moves anything, locks the folder down and confirms it can still open the vault there, and only then commits the move. If any of that fails, the move is cancelled and your vault stays exactly where it was — the earlier failure that could leave a vault stranded in a folder Windows then blocked can no longer happen.",
      "PM won't let you put a shared vault somewhere it can't work — a network drive, or a USB stick formatted as FAT32/exFAT (which can't store per-account permissions) — and says so up front instead of failing partway through.",
      "Making a shared vault private again now moves it back to your own private folder first, then decrypts — so your notes are never briefly written in readable form inside a folder other accounts can reach.",
      "When you add an account to a shared vault, PM now double-checks the permission actually took effect, so a silent “looked fine but didn't work” can't slip through.",
    ],
  },
  {
    version: "3.7.0-alpha",
    date: "2026-07-13",
    highlights: [
      "If Windows ever blocks PM from its own vault folder, PM now says exactly that — and offers a one-click “Repair access” that fixes the folder's permissions and opens your vault again. Before, a blocked folder showed up as a confusing wall of “the vault is locked” messages, a restart landed on a dead-end error screen, and rejoining looked like your passphrase had stopped working. Your data was never gone — now PM says so, in plain words, on every screen it could happen.",
      "Every vault problem now tells you what's actually wrong and what to do about it: a folder PM can't reach points to Repair access, a folder that's gone says so, a wrong passphrase says it's the passphrase (and only when it really is), and a damaged vault file is called damaged — four different problems that used to look identical.",
      "“Use a vault on this account instead” now explains what will happen before it does anything — whether you'll get back the vault that was set aside when you joined, or start with an empty one — and PM remembers the shared vault you left, so Settings can offer “Rejoin” with one click later. Nothing in the shared folder is ever deleted.",
      "Two protections against losing a shared vault by accident: “Remove PM data” on an account that's joined a shared vault now cleans up only that account's own data and leaves the shared vault untouched for everyone else, and the “start fresh” recovery can no longer delete a vault that's merely blocked by permissions.",
    ],
  },
  {
    version: "3.6.4-alpha",
    date: "2026-07-13",
    highlights: [
      "Finishing “Remove PM data” now actually closes the app. The “Close PM” and “Finish uninstall” buttons at the end of the removal flow used to do nothing when clicked, leaving PM open — they now quit it properly. And a full uninstall on Windows no longer strands a leftover “PM” folder: the bundled document-engine files it used to leave behind are now cleaned up with everything else.",
      "One more safeguard before you erase your data — when your vault and database are part of what you're removing, the confirmation screen now reminds you, in plain sight, that they can't be recovered, so you can back them up first if you haven't already.",
    ],
  },
  {
    version: "3.6.3-alpha",
    date: "2026-07-13",
    highlights: [
      "Windows updates no longer fail silently. If Windows Smart App Control is switched on, it blocks PM's update installer with no visible error — clicking “Restart now” would close PM and quietly reopen on the old version. Now PM checks for that first and, when it's the cause, tells you plainly and points to the one thing that fixes it (turning Smart App Control off), instead of trying the same broken update again and again on every launch.",
    ],
  },
  {
    version: "3.6.2-alpha",
    release: true,
    date: "2026-07-13",
    highlights: [
      "PM 3.6.2 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "Share your vault across Windows accounts on this PC — one guided flow moves the vault somewhere every account can reach, you pick who may open it from a list, and the other account joins from a single screen by typing the passphrase. Whatever was already on that account is kept safely aside, never deleted.",
      "PM now runs on Linux (x86_64) — an auto-updating AppImage (the recommended install) and an rpm for Fedora-family systems, both self-contained and built by the same signed pipeline as Windows and macOS.",
      "Index a folder but skip parts of it — when you choose what to index from a Google Drive, OneDrive, or a folder on this computer, uncheck any subfolder to leave it (and everything inside) out. Handy for a big archive or a noisy downloads folder.",
      "Mark a calendar as “Quiet” — it still shows on your Calendar tab, but its events stay out of everything PM brings to your attention: the daily briefing, “due soon” reminders, and chat.",
      "Tidy up after filing — change a document's project or importance once it's already filed, straight from the Documents tab, and clear a review one document at a time instead of committing the whole list.",
      "Sturdier and safer — “Remove PM data” can no longer leave you stuck on a vault that won't open, the “couldn't open your vault” screen now has a way forward, and finding the Proton Drive CLI for encrypted backups is far more reliable. Plus under-the-hood work: Linux keychain support, safer release tooling, and refreshed build dependencies.",
    ],
  },
  {
    version: "3.6.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Sharing your vault with another Windows account on this PC actually works now — and it's one guided flow. Before, “make shareable”, “move vault”, and “link account” were three separate steps that quietly left the vault somewhere other accounts could never reach. Now “Share with other accounts…” walks you through it: choose a passphrase, PM moves the vault to a spot every account can reach, and you pick who may open it from a list of this PC's accounts (no more hunting for SIDs — though you still can).",
      "Joining is now one screen. The first time PM opens on the other account, it spots the shared vault and offers it by name — enter the passphrase and everything shared is there: documents, chats, projects, calendars. Anything already on that account is kept safely aside, never deleted. There's also “Open an existing shared vault…” in Settings for joining by folder.",
      "After joining, PM explains what stays personal: your own AI key and your own cloud sign-ins (they never travel between Windows accounts). A note on the Connectors tab shows exactly what to reconnect, and everything already indexed stays findable meanwhile.",
      "Safety nets all around: PM refuses to move a vault onto a different vault (no more silent overwrites), linked accounts survive later moves, a shared vault that stops answering gets a clear explanation with a one-click way back to a vault of your own, and a passphrase changed on the other account simply asks you for the new one instead of acting broken.",
      "The changelog no longer pops up on a brand-new install pretending something changed — it now only appears after a real update.",
    ],
  },
  {
    version: "3.6.0-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now index a folder but skip parts of it. When you choose which folders to index from a Google Drive, OneDrive, or a folder on this computer, the folder tree lets you uncheck any subfolder to leave it (and everything inside it) out — handy for skipping a big archive or a noisy downloads folder while indexing the rest.",
      "Your choices take effect on that connection’s next sync. Anything already indexed inside a subfolder you’ve just excluded is removed from search then — but the files themselves are never touched on disk or in the cloud.",
    ],
  },
  {
    version: "3.5.0-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now change a document’s project and importance after it’s been filed, straight from the Documents tab. Filed something in the wrong place, or want to raise its importance? Click “Edit” on its row and adjust — nothing is re-processed.",
      "In Review, you can now approve documents one at a time: each row has its own “Approve” button, so you can clear the ones you’re sure about without committing the whole list.",
      "Fixed a small annoyance in Review — choosing an importance no longer re-shuffles the list and scrolls you away from the item you were working on. It stays put.",
      "When you preview how an indexed-only file (from a connected Drive/OneDrive/folder) was split into chunks, PM now shows the highlights correctly, or — if its saved breakdown is out of date — tells you plainly and offers a one-click “Re-index this item” to rebuild it, instead of a dead-end message.",
    ],
  },
  {
    version: "3.4.0-alpha",
    date: "2026-07-13",
    highlights: [
      "New: mark a calendar as “Quiet”. A quiet calendar still shows on your Calendar tab, but its events are kept out of everything PM brings to your attention — the daily briefing, “due soon” reminders, and chat. Ideal for a calendar you want to see but not be nudged about, like a recurring payments tracker. Toggle “Quiet” next to each calendar under Settings → Connectors.",
    ],
  },
  {
    version: "3.3.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Fixed the “Get the Proton Drive CLI” button, which had started leading to a missing page — it now opens Proton’s current guide.",
      "PM finds the Proton Drive CLI more reliably now. It also looks where the download usually lands (like your Downloads folder); if you keep it somewhere else you can point PM straight at it with “Locate manually…”; and a “Check again” button — plus an automatic re-check when you return to the window — picks it up right after you install it, with no restart.",
    ],
  },
  {
    version: "3.3.0-alpha",
    date: "2026-07-13",
    highlights: [
      "Fixed an important issue with “Remove PM data”: if the removal was interrupted — for example by a force-quit, or antivirus briefly locking the file — PM could end up unable to open your vault, stuck on the “couldn’t open your vault” screen with no way forward. Removal now works in a safe order and, if the vault file can’t be deleted, stops without changing anything so you can simply try again.",
      "Added a “Start fresh” option on the “couldn’t open your vault” screen. If the vault ever genuinely can’t be opened, you can now safely reset and set PM up again from there — you’re never stuck. (Your saved keys and sign-ins are kept.)",
      "Choosing to remove everything now uninstalls PM completely: after clearing your data it hands off to the Windows uninstaller and leaves nothing of PM behind on your computer.",
      "Clearer wording on the “Remove PM data” screen about what each choice deletes — your connected accounts and model preferences are stored in the database, so they’re removed with “Vault & database”, while “Saved keys & sign-ins” clears the keys and tokens themselves.",
    ],
  },
  {
    version: "3.2.1-alpha",
    date: "2026-07-12",
    highlights: [
      "Under-the-hood tidying: refreshed several of the development tools PM is built with to their latest maintenance releases. No change to how you use PM.",
    ],
  },
  {
    version: "3.2.0-alpha",
    date: "2026-07-10",
    highlights: [
      "PM now ships for Linux (x86_64): an auto-updating AppImage — the recommended install — and an rpm for Fedora-family systems. Both carry everything PM needs, including its own Python, so the document engine works out of the box with nothing to install.",
      "Linux releases are built, checked, and published by the same pipeline as Windows and macOS, and the AppImage auto-updates exactly like the Windows app.",
      'New in the README: a Linux install guide and a step-by-step "moving between computers" guide — your encrypted backup is the whole migration, on any OS pair.',
      "For the technically curious: the AppImage teaches the document engine to survive relaunches (AppImages mount at a random path each launch, so PM parks its bundled Python in your data folder), and a new no-secrets dry-run workflow proves the whole Linux packaging lane on every packaging change.",
    ],
  },
  {
    version: "3.1.0-alpha",
    date: "2026-07-10",
    highlights: [
      "Groundwork for Linux (first of a short series): on Linux, PM now keeps its secrets in your desktop's real keychain (KWallet or GNOME Keyring), so the key that protects your store survives a reboot — the load-bearing first step toward a supported Linux app. Nothing changes on Windows or macOS.",
      "The app-lock switch on Linux now says honestly that it isn't available yet, instead of offering a lock that any keypress would satisfy. A real Linux lock is on the roadmap.",
      "Fixed: restoring a backup made with the app lock turned on, onto a computer that can't run the verification (a Mac today, a Linux machine tomorrow), no longer shows a lock screen that can never pass — PM opens normally, tells you the lock is inactive on this device, and the setting re-arms on a machine that can verify.",
      "Shared vaults on Linux now get an OS-level folder lockdown (owner-only access, linked accounts re-admitted) — the same defence-in-depth Windows applies with file ACLs.",
    ],
  },
  {
    version: "3.0.3-alpha",
    date: "2026-07-10",
    highlights: [
      "Under-the-hood: our release tooling now double-checks that the notes shown on a version's download page match the version actually being shipped, so a release can't go out with the wrong notes attached. Nothing changes for you day to day.",
    ],
  },
  {
    version: "3.0.2-alpha",
    release: true,
    date: "2026-07-10",
    highlights: [
      "Fixed: a brand-new vault could freeze PM moments after its very first launch — the window went grey and “Not Responding” and never recovered. First starts now boot cleanly.",
      "Under-the-hood tidying across the whole app: a codebase-wide cleanup pass that removes duplicated code and trims wasted work. A few things get snappier along the way — dragging notes on the pinboard, panning the knowledge map, importing documents, and calendar sync with several accounts.",
    ],
  },
  {
    version: "3.0.1-alpha",
    date: "2026-07-08",
    highlights: [
      "Under-the-hood tidying: development builds no longer leave behind an ever-growing compiler cache that could quietly swallow hundreds of gigabytes of disk. Nothing changes for you day to day.",
    ],
  },
  {
    version: "3.0.0-alpha",
    release: true,
    date: "2026-07-08",
    highlights: [
      "PM 3.0.0 rolls up everything since the last public release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "Connect your clouds — Google Drive and OneDrive are indexed in place: what's in them turns up in your search, and nothing is copied out of your drive.",
      "Watch folders on this computer — point PM at a folder and it keeps itself current as you work; an edit is searchable within seconds.",
      "A real calendar — Google (several accounts), Outlook and iCal subscriptions together, in Month, Week, Day, Year and Agenda views.",
      "Chats are part of your memory — past conversations become searchable, answers cite the exact turn they drew from, chats name themselves, and each project keeps its own.",
      "A briefing that tracks instead of narrates — deadlines, today's events and prep-ahead nudges are real items you can mark done, or just tell it “the deck is done” in plain words.",
      "Projects with real milestones — several dated deadlines per project, priorities you set (or PM infers for projects other work waits on), and a sortable Focus view.",
      "Encrypted backups — your whole vault in one passphrase-protected file, on demand or on a schedule, to your own Proton Drive or Google Drive.",
      "A map of your knowledge — your documents arranged by meaning on a fast, navigable canvas.",
      "A built-in reader — click any document, or any source a chat answer cites, and read it right there; cloud items fetch their full text on demand.",
      "Photos and spreadsheets, done properly — screenshots read with on-device text recognition; spreadsheets indexed row by row, with one-click full import for Google Sheets.",
      "A Pinboard that keeps up — notes with real formatting, timelines linked to real projects, and one-click ingest of a note into your vault.",
      "Teach PM your world — merge and rename project names for good, and keep structured preferences that PM applies exactly where they fit.",
      "Sharper search — structure-aware chunking, a re-ranking second pass, multilingual vaults, and proper Chinese/Japanese/Korean keyword search.",
      "Quality of life — a calm new monochrome look with a sun-following Auto mode, a Storage tab to reclaim space, one-click Mac setup, a tidy uninstall, and a read-only Developer mode for the curious.",
    ],
  },
  {
    version: "2.92.57-alpha",
    date: "2026-07-07",
    highlights: [
      "Faster first-time indexing of large connected folders. Building the index for a big Google Drive, OneDrive, or local folder no longer slows down the further it gets — the encrypted index is now saved in batches instead of being fully rewritten after every single file, so a folder with thousands of items indexes in a fraction of the time. Nothing changes about what gets indexed or how it's sorted, and an interrupted sync still picks up cleanly where it left off.",
    ],
  },
  {
    version: "2.92.56-alpha",
    date: "2026-07-07",
    highlights: [
      "Your calendar agenda now follows your own day, not UTC's. An all-day event stays on the agenda for exactly the day you see it — no vanishing before midnight or lingering into tomorrow when your zone is far from UTC — and the same day-boundary now applies to the schedule your chat and project matching read. On the Focus view, an event that already finished today stays listed but greyed until your local midnight, so today still looks like a full day. No setup — it uses the time zone you've already set.",
    ],
  },
  {
    version: "2.92.55-alpha",
    date: "2026-07-07",
    highlights: [
      "A batch of reliability fixes. PM no longer churns files inside ignored folders (like node_modules or .git) in and out of a tracked local folder; a folder too large to list in one pass keeps its extra files instead of dropping them; and a folder whose sync hits an error now says so, instead of showing 'all good'. Preferences you state in a long chat are no longer skipped when there's a backlog. A spreadsheet column name containing a '|' no longer breaks its table, and 'you're prepared' no longer shows on the wrong occurrence of a repeating event.",
    ],
  },
  {
    version: "2.92.54-alpha",
    date: "2026-07-07",
    highlights: [
      "Deadlines and calendar dates respect your time zone at the day boundary. A calendar-linked milestone or a project's 'Due soon' countdown near midnight is now counted on the date you actually see it, in your own zone — so nothing reads a day early or late when your zone is far from UTC. No change to how anything looks.",
    ],
  },
  {
    version: "2.92.53-alpha",
    date: "2026-07-07",
    highlights: [
      "Glitch-free fast switching. Rapidly switching between conversations or documents no longer lets a slow background load land on the wrong one — the context meter, the chunk overlay, and the document chunk panel now ignore results for a view you've already moved on from. And after switching the chat to a larger-context model, the meter refreshes once the switch has actually taken effect. No change to how anything works.",
    ],
  },
  {
    version: "2.92.52-alpha",
    date: "2026-07-07",
    highlights: [
      "More robust vault changes and multi-computer coordination. When PM changes how your vault is protected — setting or changing a passphrase, or moving it to a shared folder — it now commits and cleans up in a safer order, so an interrupted change leaves no stray leftovers and can't briefly keep using the previous key. And when you run PM on more than one computer against a single shared vault, a crashed instance can no longer leave the other one waiting to take over. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.51-alpha",
    date: "2026-07-07",
    highlights: [
      "Sturdier handling of calendar feeds and pinboard notes. PM is now more resilient to unusual or malformed calendar (.ics) data, follows calendar-feed redirects more carefully, and is stricter about the internal filename it derives for a note. Normal feeds and notes are unaffected — no change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.50-alpha",
    date: "2026-07-07",
    highlights: [
      "An extra safeguard so updates never touch your saved work. PM now verifies automatically, on every build, that a database upgrade step can't delete or overwrite the projects, priorities and notes you've added — a belt-and-braces backstop behind the existing rule that updates only ever add to your data. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.49-alpha",
    date: "2026-07-07",
    highlights: [
      "Fixed a rare lockout when setting up a shareable vault. If your vault passphrase began or ended with a space, PM could store it one way but check it another and then refuse to unlock. PM now uses your passphrase exactly as typed — spaces and all — on every screen. Existing vaults are unaffected.",
    ],
  },
  {
    version: "2.92.48-alpha",
    date: "2026-07-07",
    highlights: [
      'Clearer about what leaves your device. The README and security policy now spell out the full, short list of network traffic PM makes — model calls, an update check, a one-time first-run download of its local models, and any calendar or encrypted backups you set up — so "local-first" is stated precisely. Internal test data also had a few sample names tidied up. No change to how anything works.',
    ],
  },
  {
    version: "2.92.47-alpha",
    date: "2026-07-07",
    highlights: [
      "Under-the-hood tidying. Each PM release now publishes a checksum file so you can verify an installer you downloaded by hand; the optional add-on components (photo text recognition, the knowledge-map reducer) are now covered by the same dependency security scanning as the core, and that scanning flags a wider range of advisories. No change to how anything works.",
    ],
  },
  {
    version: "2.92.46-alpha",
    date: "2026-07-07",
    highlights: [
      "Spreadsheet reading is hardened against booby-trapped files. When PM reads an Excel (.xlsx) spreadsheet, it now guards against one crafted to balloon memory or abuse its internal XML, on top of the existing size limits. Normal spreadsheets are unaffected.",
      "PM no longer reads legacy .xls spreadsheets. The decades-old Excel format needs a parser with a weak safety record on untrusted files, so PM now handles only modern .xlsx and .csv. Your .xls files stay on disk untouched — re-save one as .xlsx and PM will index it. Any .xls files already indexed drop out of search on the next sync.",
    ],
  },
  {
    version: "2.92.45-alpha",
    date: "2026-07-07",
    highlights: [
      "Calendar feed subscriptions are hardened against a network redirection trick. When PM fetches a calendar (.ics) feed, it now pins the exact address it already verified is safe — so a malicious feed can't quietly point PM at an address inside your own network between when you add the feed and when it's fetched. Normal feeds are unaffected.",
    ],
  },
  {
    version: "2.92.44-alpha",
    date: "2026-07-07",
    highlights: [
      "Disconnecting a Google account now fully cuts off PM's access. When you disconnect a Google Drive or Calendar account, PM tells Google to sever the connection — so PM drops off your account's connected-apps list right away, instead of lingering until the sign-in expires on its own. Microsoft accounts (which can't be revoked from inside an app) now show a one-click link to finish removing access yourself.",
      "Under-the-hood hardening. A release build of PM is stricter about where it looks for its bundled helper program, and it no longer follows a shortcut (symlink) that points outside a folder you're indexing — so indexing a folder can't quietly pull in files from elsewhere on your disk. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.43-alpha",
    date: "2026-07-07",
    highlights: [
      "Stronger protection for the passphrases that lock your data. When you set a passphrase for a shareable vault or an encrypted backup, PM now checks it's genuinely hard to guess — with a live strength meter as you type — and won't accept a weak one. Your existing passphrases still open everything exactly as before; this only applies when you set or change one.",
      "A shared vault can't be quietly downgraded. If a vault you share between computers has its 'keep notes encrypted at rest' setting changed behind your back, PM now notices when it opens the vault, switches encryption back on, and tells you — so your notes never silently start saving as plain text.",
    ],
  },
  {
    version: "2.92.42-alpha",
    date: "2026-07-07",
    highlights: [
      "Your documents and calendar entries can't dress themselves up as PM's own formatting. When PM cites sources in an answer, or lists your agenda, the text inside a document or a calendar event is now kept clearly separate from PM's own citation numbers and agenda lines — so nothing you've saved can be made to look like it came from PM itself.",
    ],
  },
  {
    version: "2.92.41-alpha",
    date: "2026-07-06",
    highlights: [
      "When editing a milestone doesn't go through, PM now tells you. Adding, renaming, moving, ticking off, or removing a milestone — on a project's Focus view or a pinboard timeline — used to fail silently if something went wrong behind the scenes, looking like it worked until the next refresh quietly undid it. Now you see the error instead.",
      "Clicking 'Use it' twice on an AI-suggested deadline no longer files it twice. The button now settles while the milestone is being added, and a deadline that's already on the project is skipped.",
      "Under-the-hood tidying. Added tests around the vault hand-off between two computers sharing one vault, the sidecar lookup, and the 'Remove all data' keychain sweep — pinning behaviour that had none. No change to how anything works.",
    ],
  },
  {
    version: "2.92.40-alpha",
    date: "2026-07-06",
    highlights: [
      "Your chats show their real name everywhere. When PM titles a conversation for you — or you rename one — that title now also updates in your Documents list and in the citations that point back to a chat, instead of keeping the short snippet it started life with.",
      "'Remove all data' is more thorough. If PM was interrupted partway through moving your vault to a new folder, a wipe now also clears the temporary backup it left behind, so removing everything never leaves a readable copy of your data on disk.",
      "Under-the-hood tidying. Renaming or merging a person or project no longer counts as fresh activity on every document it touches. No change to how anything looks.",
    ],
  },
  {
    version: "2.92.39-alpha",
    date: "2026-07-06",
    highlights: [
      "The knowledge map keeps up with your edits. When you change what's inside a document, PM now re-places it on the map to match its new meaning — previously a small edit that didn't change the document's length could leave it sitting in its old spot until something else on the map shifted.",
      "Under-the-hood tidying. PM now reuses a single connection when talking to your chat models instead of opening a fresh one for every request, and the progress bars for the optional downloads (the knowledge-map reducer, photo text recognition, and the Mac Python setup) all run through one shared piece of plumbing. No change to how any of it looks or works.",
    ],
  },
  {
    version: "2.92.38-alpha",
    date: "2026-07-06",
    highlights: [
      "Steadier cloud syncing and rebuilds. If a sync can't fully reach one of your connected accounts, PM now marks that account as needing another try instead of quietly treating the partial pass as complete — so nothing gets skipped over. And if rebuilding the search index can't restore your cloud-indexed items, it now says so plainly rather than finishing as if all was well.",
      "A little more careful with backup passphrases. The passphrase you type for an encrypted backup or restore is now wiped from memory as soon as PM is done with it, matching how PM already handles your vault passphrase.",
    ],
  },
  {
    version: "2.92.37-alpha",
    date: "2026-07-06",
    highlights: [
      "Behind the scenes: refreshed the project's README so its feature tour reflects what PM has grown into — indexing your cloud and local files without copying them, chats becoming searchable, the daily attention layer, the knowledge map, and encrypted restore-anywhere backups among them. No change to how you use PM.",
    ],
  },
  {
    version: "2.92.36-alpha",
    date: "2026-07-06",
    highlights: [
      "OneDrive files now remember their folder. When PM indexes a file from OneDrive, it notes which folder the file sits in — the same detail it already keeps for Google Drive files — so that folder is there as context when you look at where a document came from.",
    ],
  },
  {
    version: "2.92.35-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. The shared logic behind PM's cloud connectors — how a Drive or OneDrive sync starts, stops, resumes and reports, and how each account row is drawn — now has an automated test net, so a future change there can't quietly break it.",
    ],
  },
  {
    version: "2.92.34-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying (developer-side only). PM's commit-time checks now also catch a forgotten version bump, its secret scanner now sweeps the whole project history rather than just the current files, and there's a new command for measuring test coverage. Nothing you'll see changes.",
    ],
  },
  {
    version: "2.92.33-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. PM now has a test that upgrades a database created by a much older version all the way to the current one, checking your chats, documents and settings come through every step intact — so a future change to how the database evolves can't quietly lose data.",
    ],
  },
  {
    version: "2.92.32-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. The small helper process PM uses to read and convert your files now has automated tests covering how it talks to the rest of the app — so a future change to that plumbing can't quietly break the exchange.",
    ],
  },
  {
    version: "2.92.31-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. PM's code checks are stricter now: the linter understands types well enough to flag a background task whose errors would otherwise be silently dropped, and every such spot in the app was reviewed and made explicit. The type-checker also now covers PM's own build configuration, not just the app code.",
    ],
  },
  {
    version: "2.92.30-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. Two of PM's own safety checks now keep each other honest — one makes sure the two internal lists of allowed software licences stay in agreement, and another makes sure every check that runs while building PM also runs in its automated online checks, so neither can quietly fall out of coverage.",
    ],
  },
  {
    version: "2.92.29-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood release-safety tidying. PM's automated checks now compile the Windows build on every change, not only when a release is cut — so a Windows-only glitch is caught early instead of slipping into an update. And the release process now double-checks that each update was signed with the exact key your installed app trusts, so a signing mix-up can never quietly ship you an update your PM would refuse.",
    ],
  },
  {
    version: "2.92.28-alpha",
    date: "2026-07-06",
    highlights: [
      "Clearer backup progress. While a backup is uploading to the cloud, the progress bar now shows a gentle “working” shimmer instead of sitting frozen at 0% (uploads don’t report a percentage). And if a backup reaches some destinations but not others — say it saves to Proton Drive but Google Drive is offline — you’ll see a plain note saying which one failed, instead of it looking like everything worked.",
    ],
  },
  {
    version: "2.92.27-alpha",
    date: "2026-07-06",
    highlights: [
      "Small fixes across the app. Dropping files onto Documents now respects the “copy photos into the vault” checkbox even if you ticked it after opening the tab. A pinboard note you turn into a document now reads cleanly everywhere, not just on the board. Switching chats quickly no longer briefly shows the wrong conversation. Clearing every model from a role and saving now sticks (it falls back to the default as the screen says). And the microphone is always released if you leave the screen mid-recording.",
    ],
  },
  {
    version: "2.92.26-alpha",
    date: "2026-07-06",
    highlights: [
      "Snappier chat and Documents. A long chat no longer re-renders every earlier message as a reply streams in, and you can scroll up to re-read while a reply is still arriving (it only auto-follows when you're already at the bottom). Opening a document from a citation fetches just that one document instead of the whole list, and the Documents table skips drawing rows you've scrolled past — both stay quick as your library grows.",
    ],
  },
  {
    version: "2.92.25-alpha",
    date: "2026-07-06",
    highlights: [
      "Smoother calendar and pinboard. Editing a pinboard note no longer writes to disk on every keystroke — PM waits until you pause typing. Your calendar's background refresh now only rewrites its stored events when something has actually changed, instead of every 15 minutes regardless. And a timed event that runs until midnight (say 8pm–midnight) shows as a full block on the day grid, not a thin sliver.",
    ],
  },
  {
    version: "2.92.24-alpha",
    date: "2026-07-06",
    highlights: [
      "Dates land on the right day. Milestone, deadline and pinboard-timeline dates could show up one day early if your computer's clock is set to a timezone behind UTC — they now render on their correct calendar day everywhere. Alongside that, some internal tidy-up: the “last synced” timestamps route through one shared helper, and the overlay dimming behind dialogs is now a single named colour.",
    ],
  },
  {
    version: "2.92.23-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying: PM now runs an automated test safety-net over the small pieces of its interface that format your dates and clean up displayed text, so future changes to those can't quietly break them.",
    ],
  },
  {
    version: "2.92.22-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood security tidying. Your vault passphrase is now scrubbed from PM's working memory the moment it's finished with, instead of lingering there. And PM's interface layer can now only read the handful of look-and-feel settings it's meant to (theme, layout, and the like) — never other internal values.",
    ],
  },
  {
    version: "2.92.21-alpha",
    date: "2026-07-06",
    highlights: [
      "Calmer startup. When PM opens, several background catch-up jobs — re-indexing chats, refreshing running summaries and titles — used to all fire at the same moment right after unlock, making the first few seconds work harder than needed (especially waking the computer from sleep). They now take turns, so nothing piles up at once. There's also a behind-the-scenes build-tuning change whose effect we'll measure before the next release.",
    ],
  },
  {
    version: "2.92.20-alpha",
    date: "2026-07-06",
    highlights: [
      "Messier files import cleanly. A spreadsheet or CSV exported from Excel in the older Windows text encoding used to fail to import (accented names and symbols tripped it up) — PM now reads those correctly. If the optional read-text-in-images component is broken or missing, importing a photo now still records its date, location and dimensions instead of failing the whole photo. And an accidentally-enormous file (say a multi-hundred-megabyte text file) is now turned away with a clear message rather than risking PM running out of memory.",
    ],
  },
  {
    version: "2.92.19-alpha",
    date: "2026-07-06",
    highlights: [
      "More honest backups. If you back up to more than one place (say Proton Drive and Google Drive) and one of them quietly starts failing, PM no longer treats everything as fine just because the other succeeded — it now records each destination's last successful backup on its own, so a stale one can be spotted (a visible warning is coming next). And disconnecting your Google account now also switches off Google-Drive backups, instead of leaving them to fail silently against an account PM can no longer reach.",
    ],
  },
  {
    version: "2.92.18-alpha",
    date: "2026-07-06",
    highlights: [
      "Two fixes for what shows up in Focus. Reminders for recurring events now come back for each new occurrence: if you dismissed a “prepare ahead” nudge for, say, a weekly standup, PM used to read that as “never remind me about this event again” — it now correctly brings the nudge back for next week’s while staying quiet about the one you already handled. And a project that has deadlines but no documents yet no longer vanishes from Focus — its milestones surface, and earn their deadline reminders, just like a project you’ve already added files to.",
    ],
  },
  {
    version: "2.92.17-alpha",
    date: "2026-07-06",
    highlights: [
      "Snappier search on big pastes, and a sturdier index fingerprint. Dropping a very large block of text into a search or chat used to make PM briefly grind while it built a keyword query out of every single word — that keyword step is now bounded, so search stays quick no matter how much you paste. And under the hood, the fingerprint PM uses to decide when your search index needs rebuilding now covers two settings it was missing, so a future change to how results are ranked or text is tokenised will correctly offer a one-time rebuild instead of quietly drifting out of sync.",
    ],
  },
  {
    version: "2.92.16-alpha",
    date: "2026-07-06",
    highlights: [
      "Sturdier chat history. When PM saves a message you or it just sent, it writes that message to a file on disk first — that file is the real record everything else is rebuilt from. If that one write ever hiccups (a momentarily locked or full drive), the message still lived in PM's working memory, but a later rebuild of the search index — which trusts the on-disk files — could quietly drop it. PM now double-checks, before it indexes, that every recent message is actually in its file, and re-writes any that slipped through. Nothing said in a conversation can fall between the two.",
    ],
  },
  {
    version: "2.92.15-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood chat tidying. When you hit “Compress” to free up room in a long conversation at the same moment PM was already tidying that chat’s running summary in the background, the two could briefly step on each other and record the same stretch twice — that can no longer happen; whoever finishes first wins and the other steps aside cleanly. And if the document search that grounds an answer in your files ever fails, PM now leaves a note in its logs instead of quietly answering as if you had no documents at all.",
    ],
  },
  {
    version: "2.92.14-alpha",
    date: "2026-07-06",
    highlights: [
      "Steadier connector indexing. If a tracked folder ever changed so fast that the live watcher lost track of a burst of edits, PM used to quietly fall behind until you synced by hand — it now notices and re-checks that folder on its own. And when Google is briefly rate-limiting or having a hiccup, an indexed Google Sheet no longer gets stuck showing a “reconnect this account” note when nothing is actually wrong; it stays findable by name and fills its details back in on the next clean sync.",
    ],
  },
  {
    version: "2.92.13-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood responsiveness. Making a backup or exporting your data used to briefly tie up the app while it took a consistent snapshot of your library, and the very first sync after installing an update could do the same while it set up the document engine. Both now happen off to the side, so the app stays smooth and responsive even while a large backup is running.",
    ],
  },
  {
    version: "2.92.12-alpha",
    date: "2026-07-05",
    highlights: [
      "Tidier, more reliable data removal. Two fixes to \"Remove PM data\": if you ever forget your vault passphrase, the removal now still clears your saved keys and secrets instead of stopping short — so you're never locked out of erasing your own data. And when you preview a restored backup, the temporary copy it creates is now cleaned up automatically on the next start and fully removed when you erase PM, so an old backup's contents never quietly linger on disk.",
    ],
  },
  {
    version: "2.92.11-alpha",
    date: "2026-07-05",
    highlights: [
      'Sturdier re-indexing for cloud- and folder-indexed files: in two rare cases where PM was interrupted at exactly the wrong moment — a crash mid-way through indexing a cloud file, or right after turning an indexed file into a full local copy — a later "Re-index everything" could quietly drop that item or keep reporting it as failed. Both now self-heal on the next start, so nothing is silently lost. This is behind-the-scenes robustness; nothing you do changes.',
    ],
  },
  {
    version: "2.92.10-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder connector screens now share one set of building blocks — the account/folder list, the sync progress bar, the post-sync summary, and the Stop control — instead of three near-identical copies, so a fix or polish to one lands for all three. Nothing about how connecting or syncing works has changed; this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.9-alpha",
    date: "2026-07-05",
    highlights: [
      "Sturdier cloud sign-in and sync: a brief Google Drive rate-limit no longer makes PM treat a healthy account as disconnected, and two things refreshing an account's sign-in at the same moment can no longer leave it stuck (most likely to have affected Microsoft/OneDrive). Under the hood, the Google and Microsoft sign-in flows now share one piece of plumbing.",
    ],
  },
  {
    version: "2.92.8-alpha",
    date: "2026-07-05",
    highlights: [
      "Steadier cloud sync on very large accounts: when Google Drive or OneDrive is too big to list in a single pass, PM now keeps what it found and picks up the rest on the next sync — instead of, in rare cases, removing files it simply hadn't reached yet, or a very large OneDrive not syncing at all. Under the hood, Drive syncing and Drive backup now share one piece of Google plumbing.",
    ],
  },
  {
    version: "2.92.7-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: Google Drive and OneDrive now share a single sync engine instead of two near-identical copies, so a fix or improvement to cloud syncing lands once for both. Nothing about how syncing works has changed — this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.6-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder sync engines moved out of the big command file into their own modules, so the connector code is easier to find and change. Nothing about how syncing works has changed — this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.5-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder sync engines now share one copy of their common plumbing (the single-flight/crash-recovery wrapper and the indexing step) instead of three near-identical copies. Syncing behaves exactly as before — this just means future fixes land in one place.",
    ],
  },
  {
    version: "2.92.4-alpha",
    date: "2026-07-05",
    highlights: [
      "If a Google Drive or OneDrive sync hit a snag part-way through — a file it couldn't fetch or index — the connector could still show as fully synced and quietly leave those files out. Now a sync that runs into an error is flagged so you can see it needs another try, and the files it couldn't reach are picked up automatically on the next sync instead of being skipped.",
    ],
  },
  {
    version: "2.92.3-alpha",
    date: "2026-07-05",
    highlights: [
      "A cloud or local-folder sync that hit an unexpected error could quietly leave that connector unable to sync again until you restarted PM. Now it always recovers on its own — the next sync just runs normally.",
    ],
  },
  {
    version: "2.92.2-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive and OneDrive connectors now share a single piece of folder-sync reconciliation logic instead of keeping near-identical copies. Sync behaves exactly as before — this just means future fixes land in one place rather than needing to be applied twice.",
    ],
  },
  {
    version: "2.92.1-alpha",
    date: "2026-07-05",
    highlights: [
      "A project's chat now shows the same context meter as the main chat. When a scoped conversation starts filling the model's context window, you get the same at-a-glance gauge and the same Compress / Continue / switch-to-a-bigger-model options — instead of silently running into the limit.",
    ],
  },
  {
    version: "2.92.0-alpha",
    date: "2026-07-05",
    highlights: [
      "Search now works properly in Chinese, Japanese, Korean and other languages that don't put spaces between words. If your vault's search language is set to multilingual, keyword search finds these documents instead of quietly falling back to a weaker match — and PM offers a one-time re-index to bring your existing documents across. Vaults searching in English are unaffected.",
      "Short messages in any language are kept. A brief note in Chinese, Russian or another non-Latin script is no longer mistaken for small talk and skipped — it's indexed and learned from just like an English one.",
    ],
  },
  {
    version: "2.91.9-alpha",
    date: "2026-07-05",
    highlights: [
      "Renaming or merging a project now keeps everything attached to it. Before, a rename could quietly strip a project of its milestones, deadline, status, saved chats and activity history — now they all move across to the new name.",
      "PM stays out of your way while you work. Background housekeeping — indexing, summaries, backups and the daily catch-up — now waits for a genuine pause instead of kicking in while you're reading, scrolling, triaging or editing.",
      "The app opens faster. The first screen no longer waits on a calendar sync or a chain of startup checks before it appears — your projects show up right away, and the calendar fills in a moment later.",
      "Backups are sturdier. A large Google Drive backup now uploads in pieces — gentler on memory, and it resumes instead of restarting if the connection blips — and a Proton backup can no longer hang forever: it times out on its own, and Cancel now works mid-transfer.",
    ],
  },
  {
    version: "2.91.8-alpha",
    date: "2026-07-05",
    highlights: [
      "Startup is more forgiving. If PM can't open your vault the instant it launches — usually because antivirus or Windows Search is briefly scanning the file — it no longer closes with an error. You get a friendly “try again” screen instead, and your documents are untouched.",
      "Sharing a vault across two Windows profiles is safer: if one computer goes to sleep and the other takes over as the writer, the sleeping one now steps aside cleanly when it wakes, instead of both trying to write at once. And switching a vault back to private-to-this-device now recovers reliably if it's interrupted partway, rather than getting stuck on the next launch.",
    ],
  },
  {
    version: "2.91.7-alpha",
    date: "2026-07-05",
    highlights: [
      "Reliability fix for the document engine: a very large file — a big log or text dump — no longer jams PM's search, indexing, transcription and map. Previously one oversized file could quietly freeze all of them until you restarted; now the big file is handled in pieces and everything keeps working.",
      "If the document engine ever does get stuck — a first-time model download that stalls, an operation that hangs — PM now notices and recovers on its own instead of staying frozen until the next restart.",
    ],
  },
  {
    version: "2.91.6-alpha",
    date: "2026-07-05",
    highlights: [
      "Chat reliability: if a reply ever fails to come back — a dropped connection, a provider hiccup — that conversation is no longer stuck. Just send your next message and it carries on as normal; previously the only way out was deleting the whole chat.",
      "Long answers are no longer cut off. A reply that takes several minutes to write now streams to the end instead of stopping partway; PM only gives up if the connection actually goes silent.",
    ],
  },
  {
    version: "2.91.5-alpha",
    date: "2026-07-05",
    highlights: [
      "Safety fix for “Remove PM data”: if you had moved your vault into a folder that also held your own files, removing PM’s data now clears only PM’s files and leaves everything else untouched. Previously it could delete that whole folder. If PM had the folder to itself, the now-empty folder is tidied away as before.",
    ],
  },
  {
    version: "2.91.4-alpha",
    date: "2026-07-05",
    highlights: [
      "Reliability fix: the preferences you teach PM for a specific project are now always kept when PM refreshes its internal project index. Previously a rare internal rebuild could clear them. Nothing changes in how you use PM — your taught preferences just stay put.",
    ],
  },
  {
    version: "2.91.3-alpha",
    date: "2026-07-05",
    highlights: [
      "Behind the scenes: documented a rule for future data-format changes so your organised files and folders always stay put. No change to how you use PM.",
    ],
  },
  {
    version: "2.91.2-alpha",
    date: "2026-07-04",
    highlights: [
      "Behind the scenes: refreshed the contributor documentation that describes how PM is built. No change to how you use PM.",
    ],
  },
  {
    version: "2.91.1-alpha",
    date: "2026-07-04",
    highlights: [
      "Behind the scenes: routine maintenance updates to a batch of bundled dependencies, keeping the build tooling and app framework current. No change to the app itself.",
    ],
  },
  {
    version: "2.91.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Uninstalling PM now tidies up after itself. The regular uninstaller clears the large, re-downloadable components too — the document engine, the enhanced-map and photo-text add-ons, and the speech model — so removing PM no longer leaves gigabytes behind. Your vault, database and sign-ins are deliberately kept, so if you reinstall, PM picks up right where you left off.",
      "There's a new way to fully clear PM off your machine. Settings → Data & Security → “Remove PM data” lets you erase things piece by piece: the downloaded components, your vault and encrypted database, your saved keys and sign-ins, and this window's preferences. Removing your saved keys also revokes PM's access to your connected Google accounts (Microsoft is one click away, at account.live.com, since it can't be revoked from within an app). It's made hard to do by accident — you unlock the choices, review exactly what's selected and what each one means, pass a Windows Hello check if you use one, and type “Delete PM Data” to finish. Encrypted backups are left untouched on purpose, with a reminder to remove those yourself at Proton or Google Drive.",
    ],
  },
  {
    version: "2.90.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Ingesting a Pinboard note now saves it into your vault as a real Markdown document — the whole note, not a short summary. So it's fully readable and searchable offline, it survives you tidying the note away, and a full Rebuild reproduces it from disk like any other document. Notes you'd already ingested under the previous version are quietly upgraded to the full document the next time you re-ingest them, keeping wherever you'd filed them.",
      "Pinboard notes gained a formatting toolbar. Under a note you're editing there are buttons for bold, italic, a heading, and bullet / numbered / checklist lists — each toggles on and off, applies to the line(s) you've selected, and has a keyboard shortcut (shown when you hover). A note also renders itself whenever you're not editing it — lists, headings, checkboxes and all — and you click it to write again; the separate Edit/Preview toggle is gone.",
      "The Pinboard now fills the window. The board grows to match your screen — the notes and text stay exactly the same size, there's just more room — so a maximised window shows the whole board with no scrollbars, and shrinking the window scrolls it to its edges with proper draggable scrollbars you can grab. PM opens maximised now, too.",
      "You can resize and hide the side panels. Drag the edge of the left navigation sidebar, or a project's side panel, to set its width — and drag it all the way to the window edge to tuck it away behind a slim tab, then click the tab to bring it back. Your choice is remembered.",
      "The chat input is tidier again. The message box grows as you type a longer draft (up to a point, then it scrolls) and shrinks back when you send; the mic·box·Send cluster stays centred under your conversation with the context and Explain buttons tucked just outside it; and the context-window indicator is now a small ring that fills as a conversation grows — turning red when it's getting full — in place of a bare percentage.",
      "A handful of smaller fixes: pop-up dialogs now open below the title bar so the window's own minimise and close buttons stay reachable; in the Review queue you can click a document's title to read it in the panel while you file it; the Documents table no longer pushes the page sideways when a title is long (it trims instead); and a vertical mouse-wheel always scrolls vertically everywhere, while a wide table scrolls sideways as you'd expect.",
    ],
  },
  {
    version: "2.89.0-alpha",
    date: "2026-07-04",
    highlights: [
      "A Pinboard note can now become a real document. Hit Ingest on a note and PM takes it in the same way it takes any file — it's chunked, indexed and dropped into your Review queue to file to a project and set its importance, after which it turns up in Documents, Focus, chat and search like everything else. The note shows “In review” until you file it, then “Filed · project”. Edit the note later and one “Re-ingest” updates the same document while keeping where you filed it. The ingested document is its own copy from that point on — so if you tidy the note away, what you ingested stays put.",
    ],
  },
  {
    version: "2.88.0-alpha",
    date: "2026-07-04",
    highlights: [
      "A Pinboard timeline can now track a real project. Link one (pick an existing project or type a new name) and the card shows that project's actual milestones laid out on a line, earliest to latest — each a dot with its date and label. Add, rename, re-date, tick off or remove a milestone here and it writes straight through to the project, so the very same deadlines show up in your daily briefing and the project's Focus panel — and a milestone you add there appears on the timeline too (it refreshes when you come back to the window). Dates synced from a linked calendar event stay read-only. Unlink any time to go back to a plain scratch timeline; the project keeps its milestones. Timelines with nothing linked behave exactly as before.",
    ],
  },
  {
    version: "2.87.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Pinboard notes now do lists — and formatting. Start a line with . or - for a bullet, 1. for a numbered list, i. for roman numerals, > for an arrow/quote, or [] for a checkbox, and pressing Enter carries the list on (the next bullet, the next number, a fresh checkbox); Enter on an empty item ends the list. Each note has an Edit/Preview toggle: preview renders it properly — headings, bullets, numbered and roman lists, checkboxes and quotes — and double-clicking the preview drops you back into editing. A note is still just plain text underneath, so nothing is locked into a special format.",
      "The Timeline card on the Pinboard is now available on every density, not just Standard and Power — so a minimal setup can sketch dated plans too.",
    ],
  },
  {
    version: "2.86.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Help mode now explains a lot more of PM. Turn it on in Settings → General and hover — the whole Pinboard is covered for the first time (the board, notes, timelines and the tint colours), as is the Calendar tab (the view, how to move around it, and choosing which calendars show). A batch of spots that used to highlight but say nothing when hovered — the “say what you mean” box on your home screen, the project side panel and its milestones/files split, OneDrive and local-folder connectors and their sync results, encrypted backup, vault sharing, the Microsoft sign-in, and the idle/resumed markers in chat — now all explain themselves. Nothing about how PM works changed; there's just far less of it left unexplained.",
    ],
  },
  {
    version: "2.85.0-alpha",
    date: "2026-07-04",
    highlights: [
      "The chat input is tidier. The two strips that used to stack above the message box — the context-window meter and “Explain retrieval” — are now compact buttons on the input row itself, each opening the same panel just above it when you need it. Same information and controls, less space taken up, so more of the window is your conversation. The context button still turns red and nudges you when a conversation is filling the model's window, and Explain retrieval still only appears on the Standard and Power presets.",
    ],
  },
  {
    version: "2.84.0-alpha",
    date: "2026-07-04",
    highlights: [
      "PM has a new default look: a calm, monochrome dark theme. Its base is Eigengrau — the deep near-black your eyes settle on in true darkness — with a soft off-white text and highlights (eased back from pure white so it's easy on the eyes), and no colour tinting the buttons, borders or panels. The colours that actually mean something stay in colour: the memory map's project hues, and the status tags on your projects (Due soon, Blocked, and the rest). You'll find it in Settings → General → Appearance as the first Accent swatch under the Slate style (the dark circle with a white rim); the original coloured Slate accents are still right beside it, and your other styles (Editorial, Terminal) are untouched. If you'd already picked your own look, nothing changes — this is only the starting point for a fresh install.",
      "Mode now has four choices instead of two. Alongside Light and Dark there's System, which follows your computer's own light/dark setting and flips when it does, and Auto, which follows the sun where you are — light through the day, dark after sunset — and switches itself as the day turns. Auto works out sunrise and sunset entirely on your device from your timezone, with no location permission and nothing sent anywhere; if you'd like it exact, you can type your own coordinates under the Mode picker, and it shows you when the next switch is due.",
    ],
  },
  {
    version: "2.83.0-alpha",
    date: "2026-07-03",
    highlights: [
      "You can now read your documents right inside PM. Click any item in the Documents list — or a file in a project, or a source a chat answer cites — and it opens in a reading panel beside it, without hunting for it first. Your notes, spreadsheets and saved conversations render cleanly with headings, lists and tables, and a photo shows its original image (or, if you didn't save the image, the text PM read from it). For an item you keep in Google Drive, OneDrive or a folder on your own computer, the reader now fetches and shows the full text (falling back to the offline summary if the source can't be reached), plus one button to open the original at its source — opening the web link, or revealing the file in your file manager. The old “Open in Drive” link and the separate “Show full text” button have both folded into this panel. Drag the panel's left edge to widen it up to half the window, and it now sits below the title bar so the window's own controls stay within reach.",
      "Sources are now clickable everywhere they appear. When a chat answer lists the documents it drew from — in the main chat or a project's own chat — click one to open it in the reader and see exactly what PM was working from, then close it and carry on.",
      "For power users: turn the density up to Power and the reader gains a “Show chunks” view that paints the exact boundaries PM used to split a document for search, shading each chunk as its own band so you can see how a file was divided from top to bottom (for anything with full text, including your cloud and folder items). It's read-only, and (in developer mode) can show the raw source with the same boundaries. Nothing is re-indexed or changed by looking.",
    ],
  },
  {
    version: "2.82.0-alpha",
    date: "2026-07-03",
    highlights: [
      "You can now point PM at a folder on your own computer. Open Settings → Connectors → This device → Add a folder, pick one, and PM indexes the documents inside it — so what's in your files turns up in search, right alongside your Google Drive and OneDrive. As with those, nothing is copied into PM: it only reads each file to index it, and the file stays exactly where it is. Once a folder is added PM watches it and keeps it current as you work — edit a file and it re-indexes within seconds, rename or move one and it keeps its place, delete one and it drops out of your results (still findable by name). Each folder shows how many files it's indexed and when it last synced, with Sync now and Remove; removing a folder just stops the watching — the items you've already indexed stay findable. This completes local folders as a source.",
    ],
  },
  {
    version: "2.81.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): PM now watches the folders you've pointed it at and keeps them indexed on its own — save a file and its contents are searchable within a few seconds, rename or move one and it keeps its place, project and tags, delete one and it quietly drops out of your results (still findable if you search for it by name). It also catches up whenever you reopen PM, so anything you changed while it was closed is folded in. It never keeps a copy of your files — it only reads them to index, exactly as before. The button to add a folder is the last piece, coming next.",
    ],
  },
  {
    version: "2.80.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM can now index a folder on your own computer — the same way it already does for Google Drive and OneDrive, so what's inside your files turns up in search without copying anything into PM. This first step builds the engine: it walks a folder you point it at, notices what's new, changed, moved or deleted (a rename keeps a file's place and its project and tags, rather than reading as a brand-new file), and opens each file only to index it — never keeping a copy. Nothing on screen yet; the button to add a folder, and live watching as files change, land in the next updates.",
    ],
  },
  {
    version: "2.79.0-alpha",
    date: "2026-07-03",
    highlights: [
      "There's now a box right under your daily briefing where you can just say what you mean, in plain words. Tell it something's handled (“the deck is done”) and PM marks that item off for you; tell it how you'd like to be nudged (“stop reminding me so early”) and it remembers that as a lasting preference; or ask a question and it opens a chat to answer — no menus, no clicking the right item first. Anything that would cross something off asks you to confirm first, so nothing disappears by accident.",
      "Chat now shares the same “what needs your attention” layer as your briefing. Ask “am I ready for tomorrow?” or “what's pressing this week?” and it answers from your actual flagged deadlines, events and prep — and a project's own chat sees just that project's items. This wraps up the groundwork from the last few updates: one honest, shared picture of what matters, that the briefing, chat and that new box all read from.",
      "Marking a deadline done from your briefing now also ticks it off inside the project — before, the two could disagree, with the briefing showing it handled while the project still listed it as pending. They're the same fact now, so the project's status updates to match in one step. And if you got it wrong, un-ticking the milestone brings the reminder back to your briefing, so a mistaken “done” is easy to undo.",
      "The box under your briefing now keeps a suggestion you haven't acted on yet — switch to another tab and back and it's still waiting for your confirm, instead of vanishing.",
    ],
  },
  {
    version: "2.78.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): items in your “what needs your attention” layer can now be marked done — and PM treats that as a decision it remembers, not a line it erases. A resolved item drops out of the briefing, and a later re-scan can't quietly bring it back; if you'd already prepared for an event, its “happening today” note reads “you're prepared — file's here” with a link, instead of nagging you to prepare again. The button to mark things done straight from the briefing lands in the next update — this step puts the resolution rules and the honest record underneath them in place.",
    ],
  },
  {
    version: "2.77.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Your daily briefing now stands on a real “what needs your attention” layer: PM works out the pressing items first — an overdue or approaching milestone deadline, an event happening today, something to prepare for in the next few days — and the briefing describes those, instead of the model free-associating over your projects each morning. It stays honest across the day too, so an approaching deadline quietly becomes “overdue” once it passes, and refreshing the briefing won't reshuffle the facts underneath it.",
    ],
  },
  {
    version: "2.76.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM is starting to build a proper “what needs your attention” layer under the daily briefing — things like an approaching deadline or a presentation happening today become tracked items with their own memory, rather than a paragraph the model rewrites from scratch each day. Nothing changes on screen yet; this first step just lays the foundation so the briefing (and, later, chat) can point to the same stable to-dos and let you mark them done.",
    ],
  },
  {
    version: "2.75.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: move a chat into a project — or back to global. Hover any conversation in the sidebar and use the new 📁 button to file it under a project (its search then narrows to that project's files) or send it back to global (searches everything). Past messages stay put; only where the chat lives changes, and it takes effect on your next message.",
    ],
  },
  {
    version: "2.74.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Under the hood: the quiet activity signal now tidies itself — older entries are compacted into small daily summaries once a day while the app is idle, so it stays lean and never grows without bound. Nothing to see; it just keeps house.",
    ],
  },
  {
    version: "2.73.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): the same quiet activity signal now also notices when you file a document into a project or change a project's milestones. Still nothing new on screen — it's building the history that will power smarter surfacing (which projects are “hot” right now) in a later update.",
    ],
  },
  {
    version: "2.72.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM now quietly notes which projects you're actively working in — a message in a project's chat counts as engaging with it. Nothing changes on screen yet; this is the memory that will power smarter surfacing (which projects are “hot” right now) in a later update.",
    ],
  },
  {
    version: "2.71.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: “Import fully” for indexed Google Sheets. When a Sheet from your Drive is indexed by pointer only, its row in Documents now has an Import fully button — click it and PM downloads the whole spreadsheet (every tab), reads it as a proper local spreadsheet, and makes all of its cells searchable, keeping wherever you'd already filed it. The original stays linked in Drive, and PM won't re-add it as a duplicate on later syncs.",
    ],
  },
  {
    version: "2.70.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: Google Sheets from your connected Drive are now indexed the smart way — a lightweight pointer with the spreadsheet's tab names and column headers plus a link to open it in Drive, instead of dumping the whole grid into search. Sheets stay findable and their link always works. To enable the richer tab/header details on a Google account you connected earlier, use the new “Reconnect for Sheets” button in Settings → Connectors (a quick re-approval in your browser that adds read-only Sheets access) — until then those sheets are indexed by name only. Want a sheet's actual contents searchable? Importing it fully is coming next.",
    ],
  },
  {
    version: "2.69.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: spreadsheets are indexed properly now. Drop in an .xlsx, .xls, or .csv and PM reads it as a spreadsheet — not as one giant table mangled into search — with an overview of each sheet (its columns, their types, the row count, and any date range) plus each row indexed on its own with the column names folded in, so a search finds the exact row and still knows what every value means. Small sheets stay together as one entry; very large sheets are indexed up to a sensible limit (the overview always tells you the true total). Multi-sheet workbooks are handled sheet by sheet. Nothing to turn on — it just happens on ingest.",
    ],
  },
  {
    version: "2.68.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: projects you haven't set a priority for can now show one anyway. If other projects depend on a project — it's a parent of them, or they're blocked on it — the Focus view now works that out on its own and shows a priority tag marked “(auto)”, so the projects other work is waiting on stand out even before you've triaged them. Set a priority yourself in Triage at any time and it takes over, exactly as before.",
    ],
  },
  {
    version: "2.67.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: back up to Google Drive too, alongside Proton Drive. In Settings → Backup you can now grant PM permission to save its encrypted backups to your own Google Drive — a one-time approval in your browser that only lets PM manage its own “Personal Manager Backups” folder, never the rest of your Drive. Your automatic-backup schedule is now shared: pick a frequency once and turn on each destination (Proton, Google, or both) you want it to reach — every scheduled run fans out to all of them, and each keeps its own last-N history (older backups move to Google Drive’s Trash, so nothing is truly deleted). You can save to Google Drive on demand and restore any backup from it, exactly like Proton.",
    ],
  },
  {
    version: "2.66.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: automatic backups to Proton Drive, on a schedule. In Settings → Backup you can now set PM to back up to your Proton Drive every day, week, or month — no clicking required. Choose how many backups to keep (older ones move to your Proton Trash, so nothing is ever truly deleted). Because scheduled backups run unattended, PM stores your backup passphrase in your operating system’s keychain — this is an explicit opt-in, and you can turn it off (and forget the passphrase) any time. Automatic backups only run when your vault is unlocked, you’re not in the middle of something, and Proton Drive is connected.",
    ],
  },
  {
    version: "2.65.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: back up straight to your own Proton Drive. If you have Proton’s official Drive command-line tool installed, the Settings → Backup tab can now connect to your Proton account (sign-in happens in your browser — PM never sees your Proton password) and push the same encrypted, restore-anywhere backup to a “Personal Manager Backups” folder in your Drive. You can see what’s already backed up there and restore any of them right from PM. Don’t have the tool? PM detects that and links you to the official download — it never bundles or downloads it for you. (Automatic, scheduled backups are coming next.)",
    ],
  },
  {
    version: "2.64.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: encrypted backups you can actually restore anywhere. A new Settings → Backup tab saves your whole vault — notes, database, and settings — as a single, compressed, password-protected file. Unlike “Export all data” (which only opens on this computer), a backup is self-contained: move it to another machine, enter your passphrase, and restore. Restoring is safe by design — it unpacks and verifies everything into a new folder first, so your current vault is never touched until you choose to switch to the restored one. Keep your backup passphrase somewhere safe: there’s no way to recover a backup without it. (Backing up straight to Proton Drive, and on a schedule, is coming next.)",
    ],
  },
  {
    version: "2.63.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Setting up on a Mac is now truly one-click. If your Mac doesn’t already have a recent Python, PM downloads a private copy just for itself the first time you set up the document engine — no Terminal, no Homebrew, no “install Python first”. You’ll see a progress bar while it downloads, and it only happens once. (Windows already ships with everything built in; nothing changes there.)",
    ],
  },
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
    release: true,
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
    release: true,
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
