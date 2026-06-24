PM desktop release.

## Install

⬇️ **Download ONE file — the one for your system. Ignore everything else on this page.**

🪟 **Windows** — download the file ending in **`-setup.exe`** and run it.
🍎 **macOS** — download the **`.dmg`** file, then drag **PM** to **Applications**.

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

Once you're on a release build, updates download and install from inside the app — no need
to revisit this page for each version.

## What's new in 2.1.2-alpha

**Shared & portable vaults.** PM can now protect your vault with a passphrase you
choose, instead of tying it to a single Windows account — so the same vault can be
opened from another profile on the machine.

- **Portable, never locked in.** A passphrase vault keeps its Markdown encrypted at
  rest, and you can export everything to plain Markdown anytime with your passphrase —
  encryption protects your notes, it doesn't lock you in.
- **Safe to share.** Only one profile writes at a time: if PM is open elsewhere, you
  get a calm "Continue here?" hand-off rather than two copies racing over your data.
- **Manage it in Settings → Vault.** Make a vault shareable or private again, change
  the passphrase, move it to a shared folder, link another account, or export to
  plaintext — each runs through one crash-safe migration.
- **Zero-friction by default.** Don't opt in and nothing changes: your vault stays
  private to this device, with its key in the OS keychain.

> This is an **alpha**: expect rough edges, and please report anything that bites.

## Third-party

Windows builds bundle a relocatable **CPython** runtime from
[python-build-standalone](https://github.com/astral-sh/python-build-standalone) for the
document features. Python is distributed under the PSF License Agreement; the licence text
ships with the runtime inside the app (`python/LICENSE.txt`). PM's own third-party Rust
dependencies and their licences are listed in `THIRD-PARTY-NOTICES.txt`, attached to each
release.
