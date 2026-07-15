PM desktop release — **v3.17.1-alpha**.

## Install

⬇️ **Download ONE file — the one for your system. Ignore everything else on this page.**

🪟 **Windows** — download the file ending in **`-setup.exe`** and run it.
🍎 **macOS** — download the **`.dmg`** file, then drag **PM** to **Applications**.
🐧 **Linux (x86_64)** — download the **`.AppImage`**, make it executable (`chmod +x`) and
run it; it auto-updates like the others. On Fedora-family systems you can instead install
the **`.rpm`** with `dnf` and update it later with `dnf upgrade`.

That's it. PM is **self-contained** — everything it needs (including a private Python
runtime for the document features) is inside that one file. Nothing else to install.

**🪟 Windows — if a blue "Windows protected your PC" box appears.** The installer isn't
code-signed yet, so SmartScreen is cautious. Click **More info → Run anyway**.

**🍎 macOS — first launch.** PM's alpha builds aren't signed or notarized by Apple yet, so
macOS blocks the first open (you may see "PM is damaged" or an "unidentified developer"
message). To open it once:

> 1. Move **PM** to **Applications** and try to open it.
> 2. Open **System Settings → Privacy & Security**, scroll to **Security**, and click
>    **Open Anyway** next to the PM message; confirm with your password or Touch ID.
>
> Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/PM.app`
>
> This is a one-time step per version and goes away once PM is notarized.

## Already have PM? You don't need this page

PM updates itself. Open your installed copy and it downloads this version quietly in
the background, then shows a small banner — click **Restart now** (or later, whenever
suits you) and you're on the new version. Your documents, projects, chats and settings
all stay exactly as they are.

**🍎 If the banner says the update couldn't apply** (possible on macOS while PM is
unsigned): download the `.dmg` above and drag **PM** to **Applications** again, choosing
**Replace** when asked. Everything you have is kept — just replace the old copy rather
than keeping two around.

**🐧 Linux.** The **AppImage** updates itself exactly like the Windows app. If you
installed the **`.rpm`**, update it with your package manager (`dnf upgrade`) when a new
release lands.

## What's new in 3.17.1-alpha

This release rolls up everything since v3.9.1 — here's the tour at a glance:

- **⚠ One thing to do after updating: open the Documents tab and click Rebuild, once.**
  Files you'd connected from **Google Drive**, **OneDrive** or a watched folder were only
  searchable by a short summary of themselves, so search, chat and the filing suggestions in
  Review could all miss what was deeper inside them. They're now read and indexed **in full**
  — and that one **Rebuild** is what brings the files you already have up to date. It re-reads
  your connected files from their source, one at a time, so you'll see each one processed.
  Anything offline at the time stays findable by its summary and catches up on its next sync.
- **A pinboard you can properly work on.** Folders are now made on purpose with a **+ Folder**
  button, they're **resizable**, and they stay until you ungroup them rather than evaporating
  when they're down to one card. A card only files into a folder if your **mouse** is over it
  when you let go — so a big note can finally sit beside a folder without being swallowed —
  and folders no longer swallow each other. Opening one as an overlay now gives you a real
  board to drag, resize and overlap on, not a list.
- **The pinboard has undo, and asks before it deletes.** **Ctrl+Z** (**⌘Z** on a Mac) takes
  back your last change and **Ctrl+Y** puts it back. Deleting a note or timeline now tells you
  what actually goes with it first — with a "Don't ask again" tick, and a switch in
  **Settings › General** to bring the asking back. Two things undo deliberately won't take
  back, because it can't do so honestly: **ingesting** a note, and **linking** a timeline to a
  project.
- **Your milestones and timelines now show on the calendar.** Any dated milestone appears as
  an all-day marker in its own colour across Month, Week, Day and Agenda — click it to jump
  straight to its project — and every dated entry on a pinboard timeline does the same. Hide
  either from the **Calendars** menu. The Milestones panel also **sorts** now: by deadline, by
  name, or by hand, from a **Manual / Deadline / Name** control in its header.
- **The calendar scrolls, and its timezones moved somewhere you'll find them.** Month and Year
  views now scroll smoothly through past and future instead of jumping a whole month or year
  at a time, settling neatly on a week or a row of months. Extra timezones are added from the
  **top-left corner of the Day or Week grid**, where the hour column meets the dates, and the
  list now reads as **Continent / Country / City** so you can search by any of them.
- **A far cheaper model out of the box.** PM now starts you on **Ling-2.6-flash** instead of
  Claude Sonnet 4.6 — a few hundred times less per word — which matters most for the work PM
  does when you're not watching it: naming your chats, summarising them, and proposing where
  each new document belongs. Sonnet is still there in **Settings › AI**. The honest trade: it
  reads a little less at once (262,000 tokens against a million), and it's served by a single
  provider, so if that provider has a bad day chat **waits** rather than quietly moving
  elsewhere — adding a second model and turning on **auto-switch** gives you that safety net
  back. If you've already picked your own models, nothing changes.
- **Settings is much quieter.** Every section now leads with what you came to change — the
  switches, the pickers, the connect buttons — and folds its explanation behind a caret
  underneath. Nothing was deleted, and the warnings you'd regret missing stay out in the open.

Every line above has its full story inside the app: open **What's New** from the
sidebar for the release-by-release detail.

> This is an **alpha**: expect rough edges, and please report anything that bites.

## Third-party

Windows and Linux builds bundle a relocatable **CPython** runtime from
[python-build-standalone](https://github.com/astral-sh/python-build-standalone) for the
document features. Python is distributed under the PSF License Agreement; the licence text
ships with the runtime inside the app (`python/LICENSE.txt`). PM's own third-party Rust
dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`, attached to each
release.
