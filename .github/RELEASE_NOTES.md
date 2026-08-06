PM desktop release — **v3.128.3-alpha**.

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

## What's new in 3.128.3-alpha

This release rolls up everything since v3.123.2 — here's the tour at a glance:

- **A folder on the pinboard can pick your next task for you.** Put the jobs you're dithering between
  into one folder, press the dice in its top bar, and choose how it should decide: a roulette wheel, a
  fist of straws, a box of folded slips, a coin toss, or rock-paper-scissors against PM. The first
  three pick one card out of all of them; the last two put a single card to you and let you play for
  it — lose and it's yours, win and you're off the hook. Whatever it lands on is what you do next.
  Nothing checks up on you afterwards: it is there to be argued with, not obeyed.
- **Each game is properly played, not just announced.** The wheel turns several times and slows into
  its wedge, the straws are pulled from a fist to reveal the long one, the box is shaken with the slips
  jostling inside before one is lifted out, the coin is thrown in an arc and turns over as it goes, and
  a throw is counted out with both fists before either opens. Every piece carries the name of the card
  it stands for — along its wedge, down its straw, on the slip that comes out — and a folder opened as
  an overlay draws all of it larger, with room for longer names. Motion turned down in Settings skips
  straight to the answer.
- **A round that remembers.** A card the folder has picked greys out and waits its turn, and isn't
  offered again until every other card has had one — then the round loops back and starts over. That
  round survives closing PM, so you can come back in the afternoon to a folder that still knows what it
  has already handed you. If you'd rather have no memory between plays at all — every card in every
  draw, the same one twice running — there's a switch for that, beside the one that sends a chosen card
  straight to your board. On the wheel, cards can also be given a bigger or smaller share, and the
  wedges are cut from exactly those shares, so it can never show you one thing and pick by another.
- **Every document says where it actually is.** The folders it sits in, the way Google Drive shows
  them — “My Drive › Projects › PM › documentation”, or “Shared with you › crisis › study guide”. That
  replaces a web address which could tell you how to open a file and never where it lived, so two files
  called “Notes” looked identical. A file PM has found in more than one place shows a separate trail
  for each, which is the difference that matters on the screen asking whether they're duplicates. Where
  PM cannot see the whole trail it shows what it can rather than guessing — the folders above something
  shared with you belong to whoever shared it — and the address is still one click away, with a Copy
  button.
- **A Documents table that stops wasting space and keeps the shape you give it.** Each column is the
  width of what's in it rather than reserving room for its worst case, and all that slack used to
  collect in one gap between a title and the buttons beside it. Sorting by size and going to look at
  something else no longer drops you back to newest-first when you return, and the order survives
  closing PM. A new **Source** column, off until you switch it on, says what PM is holding and what
  it's only pointing at — and clicking its heading gathers a whole library's worth of trouble at one
  end.
- **A note can tell a bullet from a dash.** Starting a line with `.` gives a round bullet and `-` now
  gives an en dash — two kinds of point instead of one, so a list and the asides hanging off it don't
  have to look the same. Both stay proper list items, so they nest, and one long enough to wrap
  continues under its own text. A checklist sits flush against the note's edge, the bullets sharing a
  list with it keep their dots, and code pasted into a note is left exactly as pasted — a line
  beginning with a dash inside a code block stays one, which in a diff is the difference between a line
  being removed and a line being added.
- **A new line in a note is brought into view before you type into it.** When PM continues a list for
  you it places the cursor itself, and a browser only follows a cursor it moved on its own — so on a
  note card a few lines tall you were typing into a line just below the bottom edge. Tab, the
  formatting buttons and undo all place the cursor the same way and are fixed with it, in both
  directions: the note no longer scrolls itself when the line was already showing.
- **Smaller things you'll see.** The badges in the Documents list line up down the right of the title
  column again, so a file with something wrong with it is findable by scanning one column instead of
  reading every row. The **Edit** button on a row stays out of the way until you hover it, like
  **Delete** beside it. And sorting by a column most documents have no answer for keeps the blanks at
  the bottom whichever way you sort.
- **Under the hood**, a review of everything in this release closed fourteen faults before any of it
  reached you — among them a folder game that moved a card onto your board after telling you you'd won
  it, a tick-list that had quietly lost its alignment, notes that scrolled themselves whenever you
  pressed Enter near the top, and a document's folder trail that could be saved wrong and stay wrong if
  Google happened to be busy at the moment PM asked.

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
