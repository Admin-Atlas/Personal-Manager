PM desktop release — **v3.85.2-alpha**.

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

## What's new in 3.85.2-alpha

This release rolls up everything since v3.44.1 — here's the tour at a glance:

- **Nothing to do after updating.** Where this release repairs something, PM repairs it itself,
  quietly, the next time it opens your vault. No rebuild, no re-sync, no prompt.
- **PM has a proper Accessibility tab.** Scale every piece of text up or down, turn animations
  off regardless of what your device says, and switch the whole interface to **Atkinson
  Hyperlegible**, a typeface designed to make letters easy to tell apart. Alongside it: a
  **Density** control setting how large controls and their click targets are, a **Contrast**
  control (**AA**, meeting the recommended 4.5:1, or **High** for AAA), and a
  **colour-blind-safe palette** that re-colours the things carrying meaning — calendar sources,
  map nodes, status badges — and backs each calendar's dot with a distinct **shape** as well as
  a colour. Underneath, much more of PM now works from the keyboard and speaks properly to a
  screen reader.
- **Today's briefing can follow you around — and keeps itself up to date.** Put it at the bottom
  of the sidebar, in a floating panel you can drag, or in an **always-on-top** window that stays
  in view while you work elsewhere; PM can also sit in your **system tray, menu bar or panel**.
  The briefing re-checks itself when you open PM, once an hour, and within a minute of anything
  behind it changing — but only spends AI when something genuinely moved, so a quiet hour costs
  nothing.
- **The Focus tab is yours to arrange.** It now uses the width of your screen, with a divider you
  can drag and a **Panels** button to switch the briefing, focus box, Upcoming and projects list
  on or off. **Upcoming** can show a day-by-day hour grid instead of a list. **Chat** became
  **Chats**, with your projects and your global chats as two foldable lists.
- **Every calendar event now opens.** Click any event and a pop-up shows everything PM has
  synced — which calendar, busy or free, location, guests and organiser, a video-call link,
  whether it repeats, the full description — with buttons through to Google or Outlook, the
  linked project, or the Pinboard. Existing calendars fill in the new details on their next sync.
- **Local AI is much better at sizing your machine.** PM now reads the **real memory** on a
  dedicated AMD or Intel card on Windows instead of falling back to
  system RAM, looks up your card's actual memory bandwidth for its speed estimates, and sizes
  each model **two ways** where it helps. It also finds models **you've already downloaded** even
  when nothing is running them, and tells you when one of them would suit your machine better
  than what you have assigned.
- **Sorting a big import is quicker, cheaper, and no longer a queue.** Approve each document the
  moment its suggestion is ready; file the rest of a folder in one click; and PM works its
  suggestions out in the background as soon as a sync finishes, asks about several documents per
  request, and **remembers** them, so closing PM never means paying for the same answer twice.
  AI suggestions are now something you **turn on**, not a requirement.
- **Google Drive reaches the files people share with you.** Turn on **"Shared with me"** under a
  Google account in Connectors and pick the files or folders you want — a folder brings its
  contents, shortcuts are followed, and a file shared with two of your accounts is indexed once.
  You can also **back up on demand** to a connected Proton Drive or Google Drive.
- **Fixes worth naming.** Review's AI suggestions now actually fill the fields in. Filing a chat
  no longer strips the markers that say "this is a conversation" — and PM repairs any that were
  damaged. Changing your vault passphrase no longer loses how a cloud file was filed. Google
  Drive ingestion works again. Closing PM's window really does close PM. Connecting a Google
  account on **Windows** no longer hits a keychain size limit, and on a **Mac** PM asks for
  keychain permission once at startup rather than once per secret. **Linux** gets a
  Debian/Ubuntu **`.deb`** installer.

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
