PM desktop release — **v3.130.10-alpha**.

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

## What's new in 3.130.10-alpha

This one finishes the job the last release started: **running AI on your own machine** — getting
a model onto it, and PM being straight with you about what it can hold and what it actually
answered. Plus the fixes and housekeeping since v3.129.1.

- **Downloading a model works now, start to finish.** PM has been showing you a list of models
  suited to your machine with no way to get any of them — the Download button existed and could
  never appear, because not one of the seventeen carried a name your model server would
  recognise. They all do now, and PM fetches the exact file it measured for the card rather than
  a differently-packaged copy that can be a third larger. The download also became something you
  can walk away from: it used to live inside the settings page, so switching tabs threw away the
  progress bar and re-armed the button, one click from starting the same multi-gigabyte fetch
  twice. It is now owned by the app — leave and come back and the progress is where you left it,
  a second copy is refused, and there is a **Cancel** button at last. And a download is no longer
  called a failure at the finish line: the silent minutes your server spends verifying a large
  file now get the time they need.
- **PM stopped guessing how much your model can hold — and the guess was hiding a real problem.**
  Every model is trained for a certain amount of conversation, but the server running it decides
  how much to really give, and those are different numbers. Ollama in particular hands over far
  less than the model can take and never mentions it. PM read the model's number, so the warning
  that a conversation is getting long, and the offer to compress it, both fire at 80% of a figure
  the conversation could never reach — while your older messages were being quietly dropped by
  the server. PM now asks the server what it is really serving, says plainly when a number has to
  be assumed, and re-checks within a minute rather than remembering the first answer forever. If
  PM told you to raise your context length and you did exactly that, it now notices.
- **The same honesty went into the work PM does in the background.** It was sending its sorting
  proposals and summaries in single lumps far larger than a local server can hold — and a server
  does not refuse those, it silently throws away the beginning and answers anyway. The beginning
  is where PM's instructions are. Work is now sized to the room your server really has before it
  is sent, PM marks off only the part it actually managed, and where something cannot be made to
  fit it stops and shows you the two numbers instead of pretending.
- **A reply that failed is no longer written down as an answer.** A cut-off or blank reply from a
  struggling local model was being recorded as a real result in several places, and in three of
  them permanently: a truncated summary was folded into your conversation's running summary and
  the messages behind it never read again; a cut-off pass over your history marked those messages
  as scanned, so anything you had said about how you like things done was lost with them; and the
  one-time import of your old profile notes could stamp itself complete having imported nothing.
  None of those count as an answer now — PM leaves the work where it is, tries again next time,
  and tells you which of the two happened. Billed calls whose replies were rejected also always
  reach the usage log, which they did not before.
- **Under the hood.** Two security boundaries that had never actually been run. PM boxes its
  document processor in so it can only reach the handful of folders it needs — but that
  confinement only exists on Linux and PM was built on Windows, so nothing had ever tested that
  it confines anything, and it fails silently if it stops working. It has now been run for real
  on a Linux machine, along with the on-device engine that reads text out of your photos, which
  runs inside the same box. Both work, and both now have tests that will say so from now on.

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
