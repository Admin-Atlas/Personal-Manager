PM desktop release.

## Install

**Windows** — download and run the `*-setup.exe` installer. The installer isn't yet
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
