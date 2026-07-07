PM desktop release — **v3.0.0-alpha**.

## Install

⬇️ **Download ONE file — the one for your system. Ignore everything else on this page.**

🪟 **Windows** — download the file ending in **`-setup.exe`** and run it.
🍎 **macOS** — download the **`.dmg`** file, then drag **PM** to **Applications**.

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

## What's new in 3.0.0-alpha

This release rolls up everything since v2.1.2 — here's the tour at a glance:

- **Connect your clouds.** Google Drive and OneDrive are indexed in place: what's in
  them turns up in your search, and nothing is copied out of your drive.
- **Watch folders on this computer.** Point PM at a folder and it keeps itself current
  as you work — an edit is searchable within seconds.
- **A real calendar.** Google (several accounts), Outlook and iCal subscriptions
  together, in Month, Week, Day, Year and Agenda views.
- **Chats are part of your memory.** Past conversations become searchable, answers cite
  the exact turn they drew from, chats name themselves, and each project keeps its own.
- **A briefing that tracks instead of narrates.** Deadlines, today's events and
  prep-ahead nudges are real items you can mark done — or just tell it "the deck is
  done" in plain words.
- **Projects with real milestones.** Several dated deadlines per project, priorities you
  set (or PM infers for projects other work waits on), and a sortable Focus view.
- **Encrypted backups.** Your whole vault in one passphrase-protected file, on demand or
  on a schedule, to your own Proton Drive or Google Drive.
- **A map of your knowledge.** Your documents arranged by meaning on a fast, navigable
  canvas.
- **A built-in reader.** Click any document — or any source a chat answer cites — and
  read it right there; cloud items fetch their full text on demand.
- **Photos and spreadsheets, done properly.** Screenshots read with on-device text
  recognition; spreadsheets indexed row by row, with one-click full import for Google
  Sheets.
- **A Pinboard that keeps up.** Notes with real formatting, timelines linked to real
  projects, and one-click ingest of a note into your vault.
- **Teach PM your world.** Merge and rename project names for good, and keep structured
  preferences that PM applies exactly where they fit.
- **Sharper search.** Structure-aware chunking, a re-ranking second pass, multilingual
  vaults, and proper Chinese/Japanese/Korean keyword search.
- **Quality of life.** A calm new monochrome look with a sun-following Auto mode, a
  Storage tab to reclaim space, one-click Mac setup, a tidy uninstall, and a read-only
  Developer mode for the curious.

Every line above has its full story inside the app: open **What's New** from the
sidebar for the release-by-release detail.

> This is an **alpha**: expect rough edges, and please report anything that bites.

## Third-party

Windows builds bundle a relocatable **CPython** runtime from
[python-build-standalone](https://github.com/astral-sh/python-build-standalone) for the
document features. Python is distributed under the PSF License Agreement; the licence text
ships with the runtime inside the app (`python/LICENSE.txt`). PM's own third-party Rust
dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`, attached to each
release.
