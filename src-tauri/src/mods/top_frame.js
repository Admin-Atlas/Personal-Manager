// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's top-frame init script (Mods spec §1.4, defence 3): stops any document that becomes a PM
// window's top document without being PM's own page, before its scripts run.
//
// Why: Tauri's init scripts, the per-launch invoke key among them, run in every top document with no
// URL allow-list, and Tauri counts the `pmmod` scheme as local. A mod's page at the top of a window
// would hold the key and reach every command. The navigation guard refuses such a navigation first;
// this is the wall behind it.
//
// Rust wraps this file in a function (engine.rs `top_frame_script`) whose one parameter,
// PM_TOP_FRAME, is `{ origins: [PM's own origin], dev: bool }`; in a dev build Vite's page on
// localhost is PM's too.
//
// The origin is read from the URL, not taken from `self.origin` alone, and must not be opaque
// either: a sandboxed copy of a PM URL is not PM's page. WebView2 injects even main-frame-only
// scripts into subframes, so a frame returns at once: frames are the sandbox's and webrtc_off.js's.
// `window.stop()` aborts the parser, so nothing after it in the document runs.
/* global PM_TOP_FRAME */

if (self === top) {
  var pmOrigin = location.protocol + "//" + location.host;
  var opaque = String(self.origin) === "null";
  var devServer =
    PM_TOP_FRAME.dev === true &&
    location.protocol === "http:" &&
    (location.hostname === "localhost" || location.hostname === "127.0.0.1");
  if (opaque || (PM_TOP_FRAME.origins.indexOf(pmOrigin) === -1 && !devServer)) {
    window.stop();
  }
}
