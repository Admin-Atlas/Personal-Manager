PM desktop release — **v3.114.9-alpha**.

## Install

⬇️ **Download ONE file — the one for your system. Ignore everything else on this page.**

🪟 **Windows** — download the file ending in **`-setup.exe`** and run it.
🍎 **macOS** — download the **`.dmg`** file, then drag **PM** to **Applications**.
🐧 **Linux (x86_64)** — the **`.AppImage`** runs on any distro and auto-updates like the
others; or install the native **`.rpm`** (Fedora/RHEL) or **`.deb`** (Debian/Ubuntu). One-line
commands and how to pick are in the **Linux guide** near the bottom of this page.

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

**🐧 Linux.** The **AppImage** updates itself exactly like the Windows app. A **package**
install (**`.rpm`** or **`.deb`**) can't self-update — PM only shows a small "reinstall to
update" note when a new version lands; download the new package (see the Linux guide below) and
run the same install command. Your notes, projects and settings are kept.

## What's new in 3.114.9-alpha

This release rolls up everything since v3.85.2 — here's the tour at a glance:

- **A short wait the first time you open this one.** Unlike the last few releases, this one does a
  little work on arrival. PM spends about a minute reinstalling its document engine so every
  component in it is checked against a known fingerprint before it runs, and your conversations move
  into their own **chats** folder inside the vault — a plain rename, nothing re-encrypted, nothing
  re-indexed. If older versions left deleted photos or spreadsheets behind, PM offers **once** to
  clear them out, shows you exactly what it found, and deletes nothing until you say so. Your files,
  vault and settings are untouched by all of it. If you use photo text recognition or the memory
  map's detailed layout, one may show as not installed afterwards — re-adding it from **Settings →
  Storage** is quick.
- **A document can belong to more than one project.** The Project field is a list you build like the
  To: line of an email. The first is the **primary** project — the one that owns the document,
  counts it as activity and places it on the Map — and the rest are links. You can see which is
  which wherever a document appears, and change it on the spot. A project's chat can now answer from
  documents linked into it, not just the ones filed there. Existing vaults carry over untouched.
- **Your tags are yours to edit — and PM stops inventing them.** **Teach → Tags** lists every label
  with how many documents carry it: rename one everywhere, fold near-duplicates together
  (tax/taxes, chair/chairs), or take one off every document. You can also re-tag your **whole
  library** from a single vocabulary chosen with everything in view. That last part fixes something
  quietly broken: tags proposed a few documents at a time produced labels like "ammun" and
  "placement" — fair descriptions of one file, and useless as tags, because a tag that lands on one
  document groups nothing. Nothing is written until you approve both the vocabulary and the
  per-document before-and-after. You can also point a chat at a tag: type **@** in the message box,
  pick one, and that single message searches it.
- **Projects you can merge, delete, and re-file into.** Merge one project into another and
  everything moves across before the old one is deleted. Delete a project and choose separately what
  becomes of its files, its chats and its name. Delete a single document from wherever you found it.
  Each asks you to type the name first and counts up exactly what it holds, because none of it can
  be undone from inside PM. **"Part of" has been removed** — it hid a project's own status behind a
  parent's name, and merging does honestly what it was being used for.
- **Sorting a new import happens as the files land.** Each file appears in Documents and in Review
  the moment it's stored, already carrying a proposed project, importance and tags — so you can
  approve the first handful while the rest are still arriving. Every file gets a suggestion however
  it reached PM, and PM never asks twice about one it has already suggested for. PM can also look
  through your library for **documents you have twice**, comparing both the opening text and what
  each document is about, so the same report saved as a Word file and a PDF still matches.
- **Every repeating calendar event now shows every time it happens.** PM was treating a whole
  repeating series as a single event, so a weekly meeting appeared exactly once. Your Month, Week
  and Year views and Upcoming on Focus will suddenly look a lot fuller — that's the honest picture.
  Editing a single occurrence no longer makes it vanish, and a sync that doesn't finish now adds
  what it found and removes nothing.
- **Your connected accounts keep themselves up to date — and a half-finished sync stops costing you
  files.** Google Drive, OneDrive and local folders are now checked when you open PM and every 15
  minutes after, the way your calendar already was ("Shared with me" gets its own hourly rhythm). A
  sync that only partly succeeded no longer removes anything: one unreadable folder, one file
  OneDrive won't hand over, or a drive that went away no longer takes the whole account down or
  quietly shortens your library. And a problem with the document engine no longer makes files
  disappear without a word — PM used to treat everything it couldn't read as unreadable forever and
  move its place-marker past it, so those files were never offered again.
- **"Remove PM data" now really removes everything — and says what it couldn't.** It reaches the
  caches, old installers and sandbox profiles PM had been leaving in folders you'd never think to
  look in, the Mac and Linux leftovers it never touched, and a vault you had moved elsewhere. It
  refuses while a backup is uploading, won't touch a vault another account owns, tells you when
  erasing a shared vault takes it away from other accounts too, and **lists what it left behind with
  the exact path** instead of claiming everything is gone.
- **Your vault is harder to tamper with and safer to interrupt.** Notes, chat transcripts and saved
  photos are now written and swapped into place in one step, so a crash mid-save leaves you the old
  version rather than half a file. A settings file edited to claim your notes aren't encrypted is
  refused rather than re-signed. Changing the passphrase on a vault shared between two accounts asks
  you to confirm you're taking it over. And an operation that can't read part of your vault — a
  passphrase change, an export, a backup — now stops and says so instead of finishing and reporting
  success. You can also **keep typing while PM is still answering**: press Enter and your message
  waits its turn, up to three at a time.
- **Reading PM is easier, and it speaks properly now.** Every status colour clears the readability
  floor in light mode (Take a look was as low as 3.1 against a required 4.5), text you're meant to
  read is out of PM's faintest grey, every dialog has a name a screen reader can announce, and
  errors and warnings are announced rather than failing in silence. Reduced motion is honoured
  everywhere, including PM's own setting.
- **Licences, properly — and a smaller install.** PM's own licence now installs alongside the app;
  the credits file covers the interface's open-source work and its typefaces too; and every local
  model PM suggests says what its weights are licensed under, with the seven under publisher terms
  showing you those terms before the download rather than after. PM also got smaller: it had been
  installing 82 MB of debugging files it never needed, so the bundled Python drops from 150 MB to
  69 MB.

Every line above has its full story inside the app: open **What's New** from the
sidebar for the release-by-release detail.

> This is an **alpha**: expect rough edges, and please report anything that bites.

## 🐧 Linux — the detailed install & update guide

**Download ONE file that matches your system**, then follow its row. Not sure which? Pick the
**AppImage** if you want automatic updates; pick the native **rpm/deb** if you'd rather PM just
land in your app menu with no fuss.

| Your distro | Download | Install & run | Updates | App-menu icon |
| --- | --- | --- | --- | --- |
| **Any distro** | `PM_*.AppImage` | `chmod +x PM_*.AppImage`, then `./PM_*.AppImage` | **Automatic**, in-app (like Windows/macOS) | Add it yourself |
| Fedora · RHEL · openSUSE | `PM-*.rpm` | `sudo dnf install ./PM-*.rpm` | Manual — reinstall the newer `.rpm` | Added for you |
| Debian · Ubuntu · Mint · Pop!_OS | `pm_*.deb` | `sudo apt install ./pm_*.deb` | Manual — reinstall the newer `.deb` | Added for you |

Every file is **self-contained** — the private Python runtime for the document features is inside,
so there's nothing else to install. (Tip: to type a filename, press **Tab** to auto-complete it.)

**AppImage — the self-updating one.** The AppImage *is* the whole app in a single file; there's no
installer step. Keep it somewhere you own, e.g. `~/Applications/`, so it can replace itself in place
when it updates — **don't delete it.** It won't add its own menu entry: run it from the file, or use
a helper like [Gear Lever](https://flathub.org/apps/it.mijorus.gearlever) to give it an icon. If it
won't start and mentions **FUSE**, install the FUSE 2 runtime (`sudo dnf install fuse` on Fedora,
`sudo apt install libfuse2` on Debian/Ubuntu) or run it with `./PM_*.AppImage --appimage-extract-and-run`.

**rpm / deb — the native packages.** These are real installers: your package manager copies PM into
place, adds the menu entry and icon, and records it — so you can **delete the downloaded package
afterwards** (unlike the AppImage). The leading `./` matters — it tells the package manager the file
is local, and dependencies are pulled in for you. If the package is refused for being unsigned, allow
it once: `sudo dnf install --nogpgcheck ./PM-*.rpm`, or confirm the prompt on Debian/Ubuntu. To
update later, download the new release's package and run the same install command; your data is kept.

**Your data outlives any uninstall.** Removing PM (either format, or deleting the AppImage) leaves
your vault and the regenerable `runtime/` (Python venv + models) under
`~/.local/share/Personal Manager`. Clear it from **Settings → Storage** or **Settings → Remove PM
data** before uninstalling, or delete that folder by hand.

**Secrets need a running keychain.** PM stores its encryption keys in the freedesktop Secret Service
— KWallet (KDE) or GNOME Keyring — over D-Bus. Every mainstream desktop provides one, so expect a
one-time wallet/keyring prompt on first launch. On a minimal window-manager setup you must run one
yourself, or PM can't store the key that protects its database.

## Third-party

Windows and Linux builds bundle a relocatable **CPython** runtime from
[python-build-standalone](https://github.com/astral-sh/python-build-standalone) for the
document features. Python itself is distributed under the PSF License Agreement.

That runtime is not only CPython: it links **OpenSSL, SQLite, libffi, liblzma, mpdecimal,
bzip2, expat and zlib**, each under its own terms. PM therefore ships the licence-bearing
build rather than the smaller `install_only` one, so every component's licence travels with
the binary — `python/LICENSE.txt` for CPython and `python/licenses/` for the rest, both
inside the installed app. The licence files come from the exact build PM pins, so they cannot
drift from what is actually linked.

PM's own third-party dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`,
attached to each release: the Rust crates it is built from, and the npm packages compiled
into its interface — including the typefaces it self-hosts.

The document features also install a small set of **Python packages from PyPI** the first time you
use them — onto your machine, not inside the installer, which is why they are not in that file.
Every one of them is recorded with its licence in `sidecar/licences.json` in the source repository,
and a check refuses a package whose terms nobody has read.
