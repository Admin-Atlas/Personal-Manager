// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Self-hosted fonts — NO CDN (privacy, offline, CSP `default-src 'self'`). Fontsource ships the
// woff2 inside node_modules; Vite bundles + fingerprints them into dist/assets, so nothing is
// ever fetched from fonts.googleapis.com / fonts.gstatic.com at runtime. The family names
// declared by these packages match design-system-docs exactly ('Newsreader' / 'Hanken Grotesk' /
// 'JetBrains Mono'), so themeVars' --head/--ui/--mono resolve to these faces. Weights are chosen
// from actual usage; Newsreader carries italic for the editorial status labels.

// Newsreader — editorial headings (--head); italic for status labels.
import "@fontsource/newsreader/400.css";
import "@fontsource/newsreader/600.css";
import "@fontsource/newsreader/400-italic.css";
import "@fontsource/newsreader/600-italic.css";

// Hanken Grotesk — UI/body (--ui everywhere; --head for slate).
import "@fontsource/hanken-grotesk/400.css";
import "@fontsource/hanken-grotesk/500.css";
import "@fontsource/hanken-grotesk/600.css";

// JetBrains Mono — numbers/ids/meta (--mono); everything in terminal.
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
