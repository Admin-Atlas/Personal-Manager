PM desktop release — **v3.44.1-alpha**.

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

## What's new in 3.44.1-alpha

This release rolls up everything since v3.17.1 — here's the tour at a glance:

- **One optional thing after updating: if you chatted with PM before this update, open the
  Documents tab and click Rebuild, once.** PM no longer treats its own past answers as source
  material when it searches your notes — that closes a subtle loop where an older, imperfect
  answer could quietly shape a new one — and that single **Rebuild** clears the older answers
  out of search. New to PM? There's nothing you need to do.
- **Run AI on your own machine — free, and private.** A new **Settings › Local AI** tab scans
  your computer, recommends on-device models it can actually run (and roughly how fast),
  connects to a local model server like **Ollama** or **LM Studio**, and lets you send your
  chats — or just the behind-the-scenes work — to it, falling back to the cloud only when the
  local model isn't reachable. A model on your own machine means **nothing leaves your
  device** — and PM now shows you **which model answered**, and says so honestly when a reply
  came from the cloud instead. If you use Ollama, PM can download a recommended model straight
  into it.
- **You can now start PM without an API key.** On first run, choose a cloud provider, a model
  on your own device, or **"set up AI later"** — PM works either way, and tells you plainly
  when a feature needs an AI provider you haven't set up yet. Already using PM? Nothing changes.
- **PM grounds its answers more carefully.** It now measures how strong the best match to your
  question really is, and when nothing fits well it **says so** and answers from general
  knowledge rather than dressing up a weak guess as a fact from your notes. It also weighs the
  **whole** shortlist of passages (not just a rough top few) and reads each one's section
  heading, so the passage you actually meant is likelier to surface — and there's a **"Save as
  note"** under each answer to keep a good reply as a real, searchable note in your vault.
- **The file-reader is now sealed off on every system.** The helper that opens and converts
  your files runs with **no way to reach the internet** and can only see the file it's working
  on — not your vault, not the rest of your computer — first on Windows, and now on **Linux and
  macOS** too. So even a booby-trapped document can't use it to phone home or snoop around.
  It's an extra wall, not a gate: if the sandbox can't fully start, PM keeps working and
  reports a short code (like `SBX-3101`) you can quote in a bug report.
- **Settings got a big tidy.** Changes now **save the moment you make them** — the Save button
  is gone — the tabs are grouped and icon-labelled down the side with their sub-sections listed
  to jump straight to, and a small **"Reset"** appears next to anything you've moved off its
  default (with a **"Reset to defaults"** on each tab). Your API keys, search language and time
  zone are deliberately left untouched.
- **Steadier under the hood.** A rebuild now **resumes** where it left off instead of starting
  over, and search keeps working the whole way through; your **passphrase** is taken exactly as
  you type it; photos saved into a vault survive a passphrase change and come out in a
  plain-files export; **"Remove PM data"** now clears every vault key PM had cached, not just
  the current one; one corrupt photo can no longer freeze the document engine; and a chat reply
  that fails partway through is reported as a **failure** instead of being saved as though it
  were the answer. Your calendars also keep their list in sync on every sync and decide whether
  something is **"today"** in your own timezone.

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
