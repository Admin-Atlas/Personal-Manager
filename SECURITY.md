# Security Policy

PM is alpha software built by a tiny team. We take security seriously — it's a
local-first app that holds sensitive personal data — but we're small, so please
read this for what to expect: best-effort timelines, honest communication, and
no bug bounty.

## Threat model (the short version)

PM is **local-first**: your data lives encrypted on your own machine, and the
only traffic that leaves is the model API call (and, if you connect one, a
read-only calendar fetch). PM treats **all ingested content as untrusted data,
never as instructions** — documents, calendar feeds, and model output can't make
PM act on your behalf. The areas where a bug would matter most are: the
**encrypted local store** (SQLCipher), **secret handling** (the OS keychain
holds your API key, the database key, and any OAuth tokens), and the **signed
release / auto-updater pipeline** (a bad update is the highest-leverage way to
reach a user's machine). Reports that touch those areas are the most valuable.

## Supported versions

PM auto-updates, and only the **most recent release** receives security fixes.
Older alpha builds are unsupported — if you're on one, update to the latest
release. There are no long-term support branches.

| Version              | Supported          |
| -------------------- | ------------------ |
| Latest release       | :white_check_mark: |
| Any earlier build    | :x:                |

(Versioning note: pre-release builds are tagged `1.x` now and sort below the
first public `2.0.0-alpha`. Whatever the number, only the newest release is
supported.)

## Reporting a vulnerability

**Please do not open a public issue for security bugs.** Public issues are
visible to everyone before there's a fix.

Report privately through GitHub's private vulnerability reporting:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Fill in the form — this opens a private advisory only the maintainers can see.

<!-- TODO(Bobby): if you want a fallback contact email for people who can't use
     GitHub's private reporting, add it here. Left blank deliberately rather
     than guessing one. -->

Please include:

- **Steps to reproduce** (a proof of concept if you have one).
- The **affected version** (see Settings → What's New, or the title bar).
- The **impact** as you understand it — what an attacker could do.

What to expect:

- A **best-effort acknowledgement within a few days**. We're a small team, so
  this isn't a guaranteed SLA.
- **Status updates** as we investigate and work on a fix.
- **Coordinated disclosure** — please give us reasonable time to ship a fix
  before disclosing publicly. We'll credit you if you'd like once it's resolved.

## Scope

**In scope:**

- The **PM desktop app** (Tauri/Rust backend + the web frontend).
- The **Python sidecar** (document conversion, embeddings, on-device transcription).
- The **build / release / signing pipeline** (CI, the signed update feed, the updater).

**Out of scope:**

- **Third-party providers** — OpenRouter and the AI models themselves. Report
  those to the relevant vendor.
- Issues that **require an already-compromised host or OS** (e.g. malware or an
  attacker who already has your account, your keychain, or filesystem access).
- **Social engineering**, phishing, or physical access.

Thanks for helping keep PM and its users safe.
