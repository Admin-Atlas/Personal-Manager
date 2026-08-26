PM desktop release — **v3.129.1-alpha**.

## Install

⬇️ **Download ONE file — the one for your system. Ignore everything else on this page.**

🪟 **Windows** — download the file ending in **`-setup.exe`** and run it.
🍎 **macOS** — download the **`.dmg`** file, then drag **PM** to **Applications**.
🐧 **Linux (x86_64)** — the **`.AppImage`** runs on any distro and auto-updates like the
others; or install the native **`.rpm`** (Fedora/RHEL) or **`.deb`** (Debian/Ubuntu). One-line
commands and how to pick are in the **Linux guide** near the bottom of this page.

That's it — nothing else to install. On **Windows and Linux** that one file is
**self-contained**: everything PM needs, including a private Python runtime for the document
features, is inside it. On **macOS** the app fetches that runtime itself the first time you use
a document feature, so the first use needs a connection.

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

## What's new in 3.129.1-alpha

This one is about **running AI on your own machine** — setting it up, and PM being honest with
you about what your machine can actually manage. Plus the fixes and housekeeping since v3.128.7.

- **PM can now walk you through setting up local AI.** It works with three local servers —
  Ollama, LM Studio and llama-server — but until now it had instructions for only one of them,
  and a recommended model you hadn't downloaded yet gave you no way to get it. All three now
  have proper instructions for your operating system, alongside what each one is good at, how
  models get into it, and the thing most likely to rule it out for you. Every recommended model
  shows a command that fetches and runs it. Every one of those commands was checked against the
  makers' own documentation rather than written from memory. If you have ever wanted PM to keep
  working without an internet connection, this is the release to try it on.
- **The numbers PM showed about your machine are now true.** It worked out how much memory you
  had free once, when you opened the tab, then judged every model against that one reading for
  the rest of the session — so opening PM while your machine was busy could rule out a model
  that fits perfectly well. On Linux it never recognised your graphics card at all. And laptop
  graphics were borrowing the speed of the desktop card that shares their name, which overstated
  some laptops by around double. All three are fixed, seventeen laptop chips now carry their own
  verified figures, and a card PM has no real figure for says so rather than guessing. A model
  that only just fits now tells you that it only just fits.
- **Two things that made a perfectly good local model look broken.** If your machine has a proxy
  set up — common behind a work network, or with a VPN client running — PM was sending requests
  to your own computer through it, which cannot work; after three failures it concluded your
  model was dead and stopped using it for minutes at a time. And the Local AI status light was
  showing green from a guess rather than from anything it had actually seen, so it could sit
  there reassuring you while chat was failing. Requests to your own machine now go straight
  there, and the light reports what really happened.
- **Elsewhere.** A document containing a single em dash or accented name no longer becomes
  unreadable, and no longer holds up everything queued behind it. Transcribing a recording is
  back to full speed on a laptop with four processor cores or fewer. Help mode stopped
  explaining a project status PM removed a long time ago, and now checks itself against the
  statuses PM really has. The libraries behind reading documents, on-device search and voice
  notes all moved up to current versions, with every licence read again before it went in.

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
| Debian · Ubuntu · Mint · Pop!_OS | `PM_*.deb` | `sudo apt install ./PM_*.deb` | Manual — reinstall the newer `.deb` | Added for you |

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
`~/.local/share/Personal Manager`. Clear it from **Settings → Storage**, or from **Settings →
Data & Security → Remove PM data**, before uninstalling — or delete that folder by hand.

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
the binary: `python/licenses/` inside the installed app holds one file per component,
CPython's own among them as `LICENSE.cpython.txt`. The licence files come from the exact build
PM pins, so they cannot drift from what is actually linked.

PM's own third-party dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`,
attached to each release: the Rust crates it is built from, and the npm packages compiled
into its interface — including the typefaces it self-hosts.

The document features also install a small set of **Python packages from PyPI** the first time you
use them — onto your machine, not inside the installer, which is why they are not in that file.
Every one of them is recorded with its licence in `sidecar/licences.json` in the source repository,
and a check refuses a package whose terms nobody has read.
