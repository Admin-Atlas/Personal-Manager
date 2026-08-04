PM desktop release — **v3.123.2-alpha**.

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

## What's new in 3.123.2-alpha

This release rolls up everything since v3.114.9 — here's the tour at a glance:

- **A little tidying when you first open this one.** PM joins up files it can *prove* it was holding
  twice — the same Google Drive file reached two ways — into one document that knows both places it
  lives. Your filing is merged rather than chosen between, and nothing is deleted to do it. It happens
  on the first launch and on the next Drive sync, and needs nothing from you. If you have never opened
  the Documents table's **Columns** menu, that table also starts from a slightly different set of
  columns than before; if you have ever ticked anything, your choice is untouched.
- **One file is one document, however many places it lives.** If you own a file and a colleague has
  also shared it with you, PM used to hold it twice — two rows, two filings, and no way to tell them
  apart on screen. It now recognises them as the same file and keeps one document living in two
  places, both still checked, so the document stays readable as long as either one is there. It only
  ever joins two records when the provider's own id says they are the same file; two documents that
  merely look alike are still shown to you to decide on. The duplicate check has moved out of Settings
  onto the **Documents** tab, runs quietly after any sync that brought something new in, and now shows
  where each copy came from — which account, which shared drive, which folder — so you can tell the two
  sides apart. You can also say **keep both**, and PM remembers.
- **Your documents say who wrote them — and keep saying it truthfully.** Author, last editor, creation
  date and size now come through from Google Drive, OneDrive and your own machine. Word, PowerPoint,
  Excel and PDF files carry that information inside the file itself, so a document from a folder on
  your computer fills in the same columns as one from Drive. **Created** now means when the document
  was created, not when the copy on your disk was made — for anything emailed, downloaded or restored
  from a backup those are wildly different dates. And every sync brings it all up to date, including
  the folder a file has been moved into; PM used to ask once, when it first saw the file, and never
  again. Where a source says nothing, the field reads "Unknown" — and something PM was once told is
  never blanked just because the source has gone quiet.
- **A Documents table you can shape.** A **Columns** menu turns each column on or off; every column
  sorts, with the rows that have no answer always settling at the bottom so sorting by author never
  buries the files that have one. Two new facts join them: **Updated**, when the file itself last
  changed at its source, and **Last synced**, when PM last had something new to write down about it.
  Between them they separate "nobody has edited this since March" from "this connector stopped working
  in March", which until now looked identical from outside.
- **Sync you can stop, leave, and ask again from scratch.** **Stop indexing** now takes effect inside
  the file listing, so it lands in seconds rather than after a whole account. Press **Sync now** on a
  second account while the first is indexing and the "Queued" label stays put when you switch tabs. And
  a new **Re-index everything**, behind the small arrow next to Sync now, reads every file in an account
  again — for when PM and the account look like they have drifted apart. It asks first, because it takes
  a while; nothing is deleted, and files PM already has are recognised and left alone.
- **Re-tagging your library is something you can walk away from.** Both halves — choosing the vocabulary
  and labelling every document — show a real progress bar, and leaving the Teach tab and coming back
  rejoins the pass where it is instead of showing you nothing. The vocabulary step is a model call, and
  its result is now kept rather than thrown away when you look at something else. A pass that ends says
  so, however it ended, and PM refuses to start a second re-tag over a first.
- **Everything about where your data lives, in one place — and an export that says what it is.** The
  folder is written above the button that opens it, and on a vault you have moved or shared, PM names
  its own settings folder separately. Export asks what you actually want — everything or just your
  documents, plain or encrypted — with a sentence for each combination saying exactly what it produces
  and how private the result is. The export archive now also carries the vault's key file, your entity
  rules and every cloud pointer, which it had been leaving out: unzipped on another machine, the old one
  gave you a vault PM could not open.
- **Backups that show their work.** The progress bar now appears under the button you pressed rather
  than five sections up the page, and the first stage shimmers instead of sitting frozen at 0%. Your
  backups are listed by date and time rather than a 68-character filename with the date on the end. The
  "a destination failed" banner can be dismissed and stays dismissed — and most of the time it was never
  a failure at all: PM was reporting a tidy-up of older backups as the backup itself failing.
- **A missing library is reported, never quietly re-created.** If PM's store has been deleted or moved
  from outside PM, it now stops, names the file, and tells you it has deleted and re-created nothing, so
  you can put it back or restore a backup first. It used to start over silently, opening looking perfectly
  normal with nothing in it. "Start fresh" now also says that it deletes your notes, which it always did.
- **Things you can see.** Switches have a visible edge and a visible off state at every theme, mode,
  accent and contrast level — and PM's own contrast audit checks them now, so they cannot quietly fade
  again. Disabled buttons dim once instead of twice. The calendar opens on an ordinary Monday-to-Sunday
  week, and swiping sideways across it keeps working past the first day. The rebuild activity list keeps
  every file in the pass, scrolls, and survives the rebuild finishing.
- **And under the hood.** Approving a large pile of documents no longer locks up the app — PM was
  rewriting and re-encrypting its whole index file once per document, so two hundred approvals meant two
  hundred full rewrites. Security updates landed for three of the libraries PM builds on. A pre-release
  review also closed several faults in the work above, before any of it reached you: files in a connected
  account being reported as missing when they were fine, a duplicate merge quietly undoing your filing on
  the next launch, and an edit to a tracked file being skipped for good if PM could not read it the first
  time.

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
