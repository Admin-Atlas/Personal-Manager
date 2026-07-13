PM desktop release — **v3.6.2-alpha**.

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

## What's new in 3.6.2-alpha

This release rolls up everything since v3.0.2 — here's the tour at a glance:

- **Share your vault across Windows accounts.** On a PC with more than one account, a
  single guided flow moves your vault somewhere every account can reach, lets you pick who
  may open it, and the other account joins from one screen by typing the passphrase.
  Anything already on that account is kept safely aside, never deleted.
- **PM runs on Linux now.** An auto-updating AppImage (the recommended install) and an rpm
  for Fedora-family systems — both self-contained, built and signed by the same pipeline as
  Windows and macOS.
- **Index a folder but skip parts of it.** When you choose what to index from a Google
  Drive, OneDrive, or a folder on this computer, uncheck any subfolder to leave it — and
  everything inside it — out. Handy for a big archive or a noisy downloads folder.
- **Quiet calendars.** Mark a calendar "Quiet" and it still shows on your Calendar tab, but
  its events stay out of the daily briefing, "due soon" reminders, and chat.
- **Tidy up after filing.** Change a document's project or importance once it's already
  filed, and clear a review one document at a time instead of all at once.
- **Sturdier and safer.** "Remove PM data" can no longer leave you stuck on a vault that
  won't open, the "couldn't open your vault" screen has a clear way forward, and finding
  the Proton Drive CLI for encrypted backups is far more reliable.
- **Under the hood.** Linux keychain support, safer release tooling, and refreshed build
  dependencies.

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
