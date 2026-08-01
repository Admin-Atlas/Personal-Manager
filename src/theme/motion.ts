// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Reduced-motion signal, for JavaScript.
//
// PM has TWO reduced-motion signals and CSS already honours both: the OS query (`index.css:336`
// gates every keyframe behind `prefers-reduced-motion: no-preference`) and the app's own
// Accessibility → Motion → "Reduced" setting, which `applyTheme` stamps onto <html> as
// `data-reduced-motion="on"` (`tokens.ts:202`) and `index.css:398-405` keys its override off.
//
// JS motion honoured NEITHER, and the override that looks like it covers this does not:
// `index.css:404`'s `scroll-behavior: auto !important` is only consulted when a scroll's `behavior`
// option is `"auto"` (the CSSOM-View default). An explicit `behavior: "smooth"` bypasses CSS
// entirely, so the `!important` is dead weight against every hard-coded smooth scroll. Nor does the
// engine rescue it: suppressing programmatic smooth scrolling under `prefers-reduced-motion` is a
// spec *should*, and Blink/WebView2 — PM's primary Windows engine — does not do it. And the in-app
// setting is not an OS media query, so no engine can ever know about it.
//
// READ THE STAMP, NOT `useTheme().reduceMotion`. One signal, three consumers (the CSS override, the
// `motion-reduce:` utilities, and this) — a second copy in React context is a copy that can drift,
// which is the entire failure this module exists to close. `tokens.test.ts:29/43/51` is what pins
// the stamp; those three assertions now protect JS behaviour as well as CSS.
//
// PLAIN FUNCTIONS, NOT HOOKS. Every call site reads at scroll time inside an event handler or an
// effect body, so a call-time read always sees the current value — including a change made while
// the component is mounted, with no dep array to keep in step and no stale closure to reason about.
// A hook would bolt a `useTheme()` subscription onto six components purely to compute a boolean at
// event time, and would couple to every test's context mock (`ChatView.test.tsx` mocks `../theme`
// with a partial `useTheme` that has no `reduceMotion`).
//
// INVARIANT: both functions read `document.documentElement`, so they must never be called at module
// scope or before the provider's first `applyTheme`. Every call site today is inside a handler or an
// effect, which holds that. Reading live — on every call, never cached at import — is also what
// makes a mid-session OS preference change take effect without a reload.

/**
 * Whether motion should be suppressed right now. True if EITHER signal asks for it: the in-app
 * "Reduced" setting (the `data-reduced-motion` stamp on <html>) or the OS preference.
 *
 * The in-app setting is checked first and is a one-way override — it is a deliberate "regardless of
 * the OS" promise made by the Accessibility tab's own copy.
 */
export function prefersReducedMotion(): boolean {
  if (typeof document !== "undefined" && document.documentElement.dataset.reducedMotion === "on") {
    return true;
  }
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false; // query unavailable (jsdom without a matchMedia stub) — keep today's behaviour
  }
}

/**
 * The `behavior` for a programmatic scroll. Pass `false` when the call site has its own reason to
 * jump instantly (a first paint, a non-user-initiated reposition) — that always wins, so a caller
 * can never accidentally re-introduce a glide.
 *
 * Usage keeps the rest of the options object untouched:
 * `el.scrollIntoView({ behavior: scrollBehavior(), block: "nearest" })` — `block` is load-bearing
 * against the whole-app-scrolls-out-of-view bug (see `ChatView.tsx`) and is none of this helper's
 * business.
 */
export function scrollBehavior(smooth = true): ScrollBehavior {
  return smooth && !prefersReducedMotion() ? "smooth" : "auto";
}
