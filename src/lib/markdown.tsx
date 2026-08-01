// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// THE single sanitization boundary for rendering ingested content as Markdown.
//
// Until this component, PM rendered every model/ingested string as escaped plain text
// (`whitespace-pre-wrap`), so raw HTML in content was inert. Rendering vault content AS Markdown is the
// first untrusted-content render surface in the app, so every rendered-ingested-content flow MUST pass
// through here. It is deliberately strict and reusable: later HTML-heavy sources (Gmail/Outlook, Notion,
// web-article capture) inherit the same boundary by rendering through `<Markdown>` rather than rolling
// their own parser.
//
// Three layers of defence:
//   1. No raw-HTML passthrough — we do NOT add `rehype-raw`, so any literal HTML in the source is
//      dropped rather than parsed (react-markdown's default).
//   2. `rehype-sanitize` runs LAST with a GitHub-flavoured allowlist schema — it strips any unsafe
//      element/attribute/URL protocol that slipped through, including `javascript:`/`data:` hrefs.
//   3. `urlTransform` (`safeUrl`) gates every link/image URL to an http/https/mailto allowlist as the
//      hast tree is turned into React elements — i.e. AFTER layer 2, not before it.
//
// Rendered links carry `target="_blank"`, and the shared `useExternalLinks` hook — mounted by BOTH
// webview roots (App and PopoverRoot) — routes their clicks to the OS browser through the
// http(s)-guarded `open_url`. That hook only guards a real click on a link that HAS a scheme, though.
// `safeUrl` is what neutralises a hostile href: it runs LAST, after the sanitizer (react-markdown
// applies `urlTransform` post-`processor.run`, not before it), so it — not the sanitizer's protocol
// allowlist — is the final word on a URL, and it is the only layer that can see the SCHEMELESS case
// (`//host`), which a protocol allowlist passes for want of a colon.

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeExternalLinks from "rehype-external-links";

// Taken from react-markdown's own props rather than importing `unified` directly: `unified` is a
// transitive dependency, not a declared one, so a type imported from it rides on hoisting.
type RehypePlugins = NonNullable<React.ComponentProps<typeof ReactMarkdown>["rehypePlugins"]>;

// Extend the default (safe) schema only to let the external-links plugin's `target`/`rel` survive
// sanitization on anchors — everything else stays at the conservative default allowlist. Exported for
// the T-07 unit test, which locks the allowlist against a regression that widens it.
export const SCHEMA = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    a: [...(defaultSchema.attributes?.a ?? []), "target", "rel"],
  },
};

// Relative/in-page targets are safe; absolute URLs must match the protocol allowlist or are dropped.
// Exported for the T-07 unit test — this is the pure function that neutralises a hostile `javascript:`
// (or any non-allowlisted) href to an empty string.
const ABSOLUTE_ALLOWED = /^(https?:|mailto:)/i;
export function safeUrl(url: string): string {
  // A protocol-relative target carries no scheme, so neither the allowlist below nor the sanitizer's
  // protocol check (which bails out "allowed" when there is no colon) ever sees one — yet the browser
  // resolves it against the PAGE's protocol, i.e. straight off-origin: `//evil.example/x` from
  // `http://tauri.localhost` is `http://evil.example/x`. This MUST precede the `/`-prefix allowance
  // below, which would otherwise read it as same-origin-relative and hand it back verbatim.
  // `\` rides along as cheap defence because Chromium's URL parser treats `/\` exactly like `//` for
  // http(s) — it is NOT a case that can arrive from Markdown, since remark percent-encodes link
  // destinations, so `/\evil.example` reaches here as `/%5Cevil.example` and stays same-origin.
  if (/^[/\\][/\\]/.test(url)) return "";
  if (url.startsWith("#") || url.startsWith("/") || url.startsWith("./") || url.startsWith("../")) {
    return url;
  }
  return ABSOLUTE_ALLOWED.test(url) ? url : "";
}

// The rehype pipeline, hoisted out of the JSX so its SHAPE can be asserted rather than only its
// behaviour (H6). ORDER is the security property: `rehypeSanitize` must be LAST, after every plugin
// that can introduce nodes, or whatever a later plugin emits reaches the DOM unsanitised. Nothing
// here may parse raw HTML — adding `rehype-raw` is the one-line change this array exists to make
// visible. The unit test pins the length too, so adding a plugin is a deliberate act that has to
// state where it sits relative to the sanitizer.
export const REHYPE_PLUGINS: RehypePlugins = [
  [rehypeExternalLinks, { target: "_blank", rel: ["noreferrer"], protocols: ["http", "https"] }],
  [rehypeSanitize, SCHEMA],
];

/**
 * Render user-authored Markdown through the app's single sanitizing boundary. Element styling lives in
 * the `.pm-markdown` block in `src/index.css` (bound to design tokens — no typography plugin), so this
 * component stays purely about the parse+sanitize pipeline.
 */
export function Markdown({ children }: { children: string }) {
  return (
    <div className="pm-markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={REHYPE_PLUGINS}
        urlTransform={safeUrl}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
