PM desktop release.

## What's new in 2.0.1-alpha

The **first public release** of PM — a private, local-first personal manager for
your documents, your focus, and the moving parts of your day, in one calm place.

- **Yours, on your machine.** Everything stays local; your encrypted store stays
  encrypted, and PM never sends your data anywhere on its own.
- **A workspace that fits you.** A polished, fully themeable interface — light and
  dark, accent colours, and depth — with documents, a Focus briefing, usage
  tracking, model suggestions, and a Pinboard.
- **Optional app lock.** Keep a quick glance from opening your things (Windows
  Hello).
- **Self-contained on Windows.** Nothing extra to install, and from here on PM
  quietly keeps itself up to date.

> This is an **alpha**: expect rough edges, and please report anything that bites.

## Install

**Windows** — download and run the `*-setup.exe` installer. PM is **self-contained**:
everything it needs (including a private Python runtime for the document features)
ships inside the installer — no separate install required. The installer isn't yet
code-signed, so Windows SmartScreen may say "Windows protected your PC" — click
**More info → Run anyway**.

**macOS** — open the `.dmg` and drag **PM** to **Applications**.

> **First launch on macOS.** PM's alpha builds are **not yet signed or notarized by Apple**,
> so macOS blocks the first open (you may see "PM is damaged" or an "unidentified developer"
> message). To open it once:
>
> 1. Move **PM** to **Applications** and try to open it.
> 2. Open **System Settings → Privacy & Security**, scroll to **Security**, and click
>    **Open Anyway** next to the PM message; confirm with your password or Touch ID.
>
> Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/PM.app`
>
> This is a one-time step per version and goes away once PM is notarized.

Once you're on a release build, updates download and install from inside the app — no need
to revisit this page for each version.

## Third-party

Windows builds bundle a relocatable **CPython** runtime from
[python-build-standalone](https://github.com/astral-sh/python-build-standalone) for the
document features. Python is distributed under the PSF License Agreement; the licence text
ships with the runtime inside the app (`python/LICENSE.txt`). PM's own third-party Rust
dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`, attached to each
release.
