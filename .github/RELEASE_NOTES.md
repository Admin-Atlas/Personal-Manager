PM desktop release — **v3.9.1-alpha**.

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

## What's new in 3.9.1-alpha

This release rolls up everything since v3.6.2 — here's the tour at a glance:

- **See other timezones on the calendar, and set your own hours.** In Day and Week view,
  add up to two more timezones from the **Zones** button and they show as extra columns
  down the left beside your local time — handy for a call with another city. The **▾** on
  the Work and Day buttons lets you frame the view to your own hours (Work sits around a
  comfortable 9-to-5, Day follows your local sunrise-to-sunset), and events now show their
  **start and end** time (e.g. 09:30–10:45), not just the start.
- **A tidier pinboard.** Notes and timelines can now carry a **title**, the **Ingest**
  button moved up into the top bar so notes get their full height back, and dropping a note
  exactly on top of another the same size folds the two into a neat **folder** tile you can
  name, open in place, and drag cards back out of. Every colour also tells you its name on
  hover now — the note tint dots and every theme swatch in Settings.
- **Honest vault recovery when Windows blocks the folder.** If Windows ever blocks PM from
  its own vault folder, PM now says exactly that and offers a one-click **Repair access**,
  instead of a confusing wall of "the vault is locked" messages. Sharing a vault across
  Windows accounts is now safe by construction — PM checks the destination can hold it,
  locks it down and confirms it still opens there *before* committing the move — and you can
  properly **delete a shared vault**, with everyone who joined told plainly and moved back to
  a vault of their own. Your data was never at risk; now PM says so, on every screen.
- **Windows updates no longer fail silently.** If Windows **Smart App Control** is switched
  on, it blocks PM's update installer with no visible error — clicking "Restart now" used to
  quietly reopen on the old version. PM now spots that and tells you the one thing that fixes
  it, instead of retrying the same broken update on every launch.
- **A cleaner exit.** Finishing "Remove PM data" now actually closes the app, a full
  uninstall on Windows no longer strands a leftover "PM" folder, and you get a clear reminder
  before erasing a vault and database that can't be recovered.

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
